# PRD: Multi-User Support for S3-CAS

## Executive Summary

Add multi-user support to s3-cas by creating per-user metadata stores while sharing the existing content-addressable block storage across all users.

## Goals

1. **User Isolation**: Each user has their own buckets and object namespace
2. **Deduplication**: Blocks shared across all users (global CAS)
3. **No Storage Code Changes**: Keep existing block storage logic (adaptive depth, _PATHS, etc.)
4. **Backward Compatible**: Single-user mode continues to work

## Non-Goals

- Per-user metrics (separate PRD)
- Per-user HTTP UI views (separate PRD)
- Dynamic user management (separate PRD - will replace TOML with DB)
- Quotas/limits (separate PRD)
- Changing existing block storage implementation

---

## Architecture

### Current (Single User)
```
/meta_root/db/
  _BUCKETS/
  _BLOCKS/
  _PATHS/
  bucket1/
  bucket2/

/fs_root/blocks/
  {adaptive depth structure}
```

### Proposed (Multi-User)
```
/meta_root/
  blocks/db/               ← RENAMED: Was /meta_root/db/ (shared block metadata)
    _BLOCKS/               (refcounts, sizes, paths)
    _PATHS/                (prefix reservations)

  user_alice/db/           ← NEW: Per-user metadata
    _BUCKETS/
    _MULTIPART_PARTS/
    bucket1/
    bucket2/

  user_bob/db/             ← NEW: Per-user metadata
    _BUCKETS/
    _MULTIPART_PARTS/
    bucket1/

/fs_root/blocks/           ← UNCHANGED: Shared CAS
  {adaptive depth structure - existing code}
```

---

## Key-Value Storage Details

### Per-User Metadata (`/meta_root/user_{id}/db/`)

#### Partition: `_BUCKETS`
```
Key: bucket_name (bytes)
Value: BucketMeta {
  name: String,
  creation_time: SystemTime
}
```

#### Partition: `_MULTIPART_PARTS`
```
Key: "{bucket}/{key}/{upload_id}/{part_number}"
Value: MultiPart {
  size: usize,
  hash: BlockID,
  blocks: Vec<BlockID>
}
```

#### Partition: `{bucket_name}`
```
Key: object_key (bytes)
Value: Object {
  size: u64,
  hash: BlockID,              // MD5 of entire object
  object_type: ObjectType,    // Single/Multipart/Inline
  creation_time: SystemTime,
  object_data: ObjectData     // Inline | SinglePart | MultiPart
}
```

### Shared Metadata (`/meta_root/blocks/db/`)

#### Partition: `_BLOCKS`
```
Key: block_hash (BlockID = [u8; 16])
Value: Block {
  size: usize,     // Block size in bytes
  rc: usize,       // Reference count (how many objects use this block)
  path: Vec<u8>    // Adaptive path prefix (see _PATHS)
}
```

#### Partition: `_PATHS`
```
Key: prefix (variable length byte slice)
Value: block_hash (BlockID)

Purpose: Reserves path prefixes for adaptive directory depth
- Tracks which prefixes are in use
- Enables shortest unique prefix allocation
- Allows directory tree to grow as needed
```

---

## Components

### 1. User Authentication

**Config File**: `users.toml`
```toml
[users.alice]
access_key = "AKIA..."
secret_key = "..."

[users.bob]
access_key = "AKIB..."
secret_key = "..."
```

**Authentication Flow**:
1. S3 request arrives with access_key in signature
2. Auth layer maps access_key → user_id
3. Request routed to user's CasFS instance

### 2. SharedBlockStore (New Component)

**Purpose**: Singleton managing shared block metadata

**Responsibilities**:
- Opens fjall DB at `/meta_root/blocks/db/`
- Provides `BlockTree` and `PathTree` to all CasFS instances
- **Does NOT change existing block write/delete logic**

**Interface**:
```rust
pub struct SharedBlockStore {
    meta_store: Arc<MetaStore>,
    block_tree: Arc<BlockTree>,
    path_tree: Arc<dyn BaseMetaTree>,
}

impl SharedBlockStore {
    pub fn new(
        path: PathBuf,
        engine: StorageEngine,
        inlined_metadata_size: Option<usize>,
        durability: Option<Durability>,
    ) -> Result<Self, MetaError>;

    pub fn block_tree(&self) -> Arc<BlockTree>;
    pub fn path_tree(&self) -> Arc<dyn BaseMetaTree>;
}
```

### 3. CasFS (Modified)

**Current Constructor**: Single-user mode
```rust
CasFS::new(root, meta_path, ...) -> Self
```

**New Constructor**: Multi-user mode
```rust
CasFS::new_multi_user(
    root: PathBuf,                 // Shared: /fs_root/blocks/
    user_meta_path: PathBuf,       // Per-user: /meta_root/user_X/db/
    shared_block_store: &SharedBlockStore,
    metrics: SharedMetrics,
    storage_engine: StorageEngine,
    inlined_metadata_size: Option<usize>,
    durability: Option<Durability>,
) -> Self
```

**Struct Changes**:
```rust
pub struct CasFS {
    async_fs: Box<dyn AsyncFileSystem>,
    user_meta_store: MetaStore,              // Per-user (was: meta_store)
    root: PathBuf,                           // Shared blocks/
    metrics: SharedMetrics,
    multipart_tree: Arc<MultiPartTree>,      // Per-user
    shared_block_tree: Arc<BlockTree>,       // Shared (was: block_tree)
    shared_path_tree: Arc<dyn BaseMetaTree>, // Shared (NEW)
}
```

**Method Changes**:
- `path_tree()`: Return `Arc::clone(&self.shared_path_tree)` instead of calling meta_store
- All other methods remain unchanged (use shared trees transparently)

### 4. Request Routing (New Component)

**Purpose**: Map S3 requests to user CasFS instances

```rust
pub struct UserRouter {
    auth: UserAuth,                              // access_key → user_id
    casfs_instances: HashMap<String, Arc<CasFS>>, // Pre-created instances
}

impl UserRouter {
    pub fn new(users_config: UsersConfig, shared_block_store: &SharedBlockStore, ...) -> Self {
        let mut casfs_instances = HashMap::new();

        // Create CasFS instance for each user at startup
        for (user_id, user) in users_config.users {
            let casfs = CasFS::new_multi_user(
                fs_root.clone(),
                meta_root.join(&user_id),
                shared_block_store,
                metrics.clone(),
                storage_engine,
                inlined_metadata_size,
                durability,
            );
            casfs_instances.insert(user_id, Arc::new(casfs));
        }

        Self { auth, casfs_instances }
    }

    pub fn get_casfs(&self, access_key: &str) -> Result<Arc<CasFS>, Error> {
        let user_id = self.auth.get_user_id(access_key)?;
        self.casfs_instances.get(user_id)
            .cloned()
            .ok_or_else(|| Error::UnknownUser)
    }
}
```

---

## Data Flow Examples

### Example 1: Upload Object (User Alice)

1. S3 PUT request arrives with alice's access_key in signature
2. Router: `access_key → user_id="alice"`
3. Router: Get/create CasFS for alice
4. **Existing code**: `casfs.store_object()` chunks data into 1MB blocks, MD5 hashes each
5. For each block:
   - `shared_block_tree.write_block()` - increment refcount or create with rc=1
   - `shared_path_tree` - allocate shortest unique prefix (if new block)
   - Write block file to `/fs_root/blocks/{adaptive_path}/{hash}`
6. Object metadata saved to `/meta_root/user_alice/db/bucket1/photo.jpg`
   - Contains: `Object{blocks: [hash1, hash2, ...], size, hash}`

### Example 2: Upload Same Object (User Bob)

1. Router maps to bob's CasFS
2. **Existing code**: `casfs.store_object()` chunks, hashes
3. Block hashes match alice's blocks exactly
4. For each block:
   - `shared_block_tree.write_block()` - increment refcount (rc: 1→2)
   - Block already exists on disk - skip write
5. Object metadata saved to `/meta_root/user_bob/db/bucket1/photo.jpg`
   - Contains: `Object{blocks: [hash1, hash2, ...], size, hash}` (same hashes!)

**Result**: Only one copy of blocks on disk, shared by both users

### Example 3: Delete Object (User Alice)

1. Router maps to alice's CasFS
2. Read object from `/meta_root/user_alice/db/bucket1/photo.jpg`
3. Get block list: `[hash1, hash2, ...]`
4. Delete object metadata from alice's DB
5. **Existing code**: `casfs.delete_object()`
   - For each block: decrement refcount in `shared_block_tree` (rc: 2→1)
   - Blocks with rc>0 NOT deleted from disk
6. Bob's object still works (blocks remain on disk with rc=1)

### Example 4: Delete Object (User Bob)

1. Router maps to bob's CasFS
2. Read object, get blocks: `[hash1, hash2, ...]`
3. Delete object metadata from bob's DB
4. For each block: decrement refcount (rc: 1→0)
5. **rc=0**: Blocks ARE deleted from disk
   - Remove files from `/fs_root/blocks/`
   - Remove entries from `_PATHS` (free the prefix)

**Result**: Blocks cleaned up when last reference removed

---

## Implementation Phases

### Phase 1: Core Multi-User

**Files to Create**:
- `src/cas/shared_block_store.rs` - SharedBlockStore struct
- `src/auth/mod.rs` - Auth module
- `src/auth/user_config.rs` - Parse users.toml
- `src/auth/router.rs` - UserRouter

**Files to Modify**:
- `src/cas/fs.rs` - Add new_multi_user(), change fields to Arc
- `src/main.rs` - Create SharedBlockStore, use UserRouter
- `Cargo.toml` - Add TOML parsing dependency

**Implementation Steps**:
1. Create `SharedBlockStore` struct
2. Add `CasFS::new_multi_user()` constructor
3. Update `CasFS` struct fields (block_tree → Arc, add shared_path_tree)
4. Update `path_tree()` method to use shared reference
5. Create `UserAuth` for config parsing
6. Create `UserRouter` for request routing
7. Update `main.rs` to use router
8. Keep `CasFS::new()` for backward compatibility

### Phase 2: Testing

**Test Cases**:
1. Multi-user deduplication
   - Alice uploads file → check block created with rc=1
   - Bob uploads same file → check rc=2, no new blocks
2. Concurrent operations
   - Multiple users upload/delete simultaneously
   - Verify no deadlocks, correct refcounts
3. Delete refcount
   - Alice deletes → rc decrements
   - Bob's copy still works
   - Bob deletes → blocks removed from disk
4. Backward compatibility
   - Single-user mode (no users.toml) still works

### Phase 3: Documentation

1. Update README with multi-user setup
2. Document users.toml format
3. Migration guide (single → multi-user)

---

## Transaction Safety

**Critical**: The existing short transaction pattern MUST be preserved to avoid deadlocks.

### Current Pattern (Preserved)
```rust
// In store_object() - per block
let mut tx = shared_block_tree.begin_transaction();  // ← Lock acquired
tx.write_block(hash, size, key_has_block);           // ← Fast metadata update
tx.commit();                                         // ← Lock released (microseconds)
```

**Why This Works for Multi-User**:
- Transaction holds lock for microseconds only
- No I/O operations inside transaction
- Multiple users can update refcounts in parallel with minimal contention
- Each user's bucket/object metadata is in separate DB (no contention)

---

## Configuration

### Command Line (Unchanged)
```bash
s3-cas server \
  --fs-root /data/blocks \
  --meta-root /data/meta \
  --access-key KEY \
  --secret-key SECRET \
  --users-config users.toml   # NEW: optional
```

### users.toml Format
```toml
[users.alice]
access_key = "AKIAIOSFODNN7EXAMPLE"
secret_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"

[users.bob]
access_key = "AKIAI44QH8DHBEXAMPLE"
secret_key = "je7MtGbClwBF/2Zp9Utk/h3yCo8nvbEXAMPLEKEY"
```

**Behavior**:
- If `--users-config` provided → multi-user mode
- If not provided → single-user mode (backward compatible)

---

## Success Criteria

1. ✅ Multiple users can upload/delete independently
2. ✅ Same content uploaded by multiple users → only one copy on disk
3. ✅ User deletes object → other users' copies unaffected
4. ✅ No deadlocks with concurrent operations
5. ✅ Existing single-user deployments continue to work unchanged
6. ✅ Refcounts correctly track block usage across all users

---

## Design Decisions

1. **Block metadata path**: `/meta_root/blocks/` (minimal change from current `/meta_root/db/`)
2. **CasFS instances**: Pre-created at startup for all users in config
3. **Metrics**: Global counters (per-user metrics deferred to future PRD)
4. **HTTP UI**: Admin view showing all users (per-user views deferred to future PRD)
5. **Config reload**: Not supported initially (requires restart to add/remove users)

---

## Future Enhancements (Separate PRDs)

- **Dynamic user management**: Replace TOML with DB-based user storage, runtime user add/remove
- **Per-user metrics**: Track storage/bandwidth per user
- **Per-user HTTP UI views**: Login as specific user, see only their buckets
- **Quotas**: Per-user limits on buckets/objects/bytes
- **Audit logging**: Track which user performed which operation
