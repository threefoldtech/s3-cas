# CAS Storage Library API Documentation

## Overview

The `cas-storage` library provides a generic content-addressable storage layer with block-level deduplication, reference counting, and multi-user support. It can be used as the foundation for various storage protocols (S3, RESP, gRPC, etc.).

## Core Concepts

### Content-Addressable Storage (CAS)
- Objects are chunked into 1 MiB blocks
- Each block is identified by its MD5 hash (BlockID)
- Duplicate blocks are automatically deduplicated
- Reference counting tracks block usage across objects

### Multi-User Architecture
- **Shared block storage**: All users share physical blocks (deduplication across users)
- **Isolated metadata**: Each user has separate buckets and object namespaces
- **Lazy initialization**: Per-user storage instances created on-demand

## Public API

### Core Types

```rust
use cas_storage::{
    // Metadata structures
    Block, BlockID, Object, ObjectData, ObjectType, BucketMeta,

    // Storage interfaces
    CasFS, SharedBlockStore, MetaStore, BlockTree,

    // Traits for custom backends
    Store, BaseMetaTree, MetaTreeExt, Transaction,

    // Storage engines
    StorageEngine, FjallStore, FjallStoreNotx, Durability,

    // Streaming and utilities
    BlockStream, RangeRequest, MultiPart,

    // Errors
    MetaError,
};
```

### Single-User API

For simple, standalone storage where one instance manages all data:

```rust
use cas_storage::{CasFS, StorageEngine, Durability, SharedMetrics};
use std::path::PathBuf;

// Create single-user storage instance
let casfs = CasFS::new(
    PathBuf::from("./data/blocks"),   // Block storage directory
    PathBuf::from("./data/meta"),     // Metadata database directory
    SharedMetrics::new(),              // Metrics collector
    StorageEngine::Fjall,              // Storage backend (Fjall or FjallNotx)
    None,                              // inlined_metadata_size (None = default 1 byte)
    Some(Durability::Immediate),       // Durability level
);

// Database location: ./data/meta/db/
// Block files: ./data/blocks/00/01/02/...
```

**Database Structure (Single-User):**
```
./data/
├── blocks/              # Physical block files
│   ├── 00/01/02/...    # Hierarchical storage by hash
│   └── [block files]
└── meta/
    └── db/              # Single Fjall database
        ├── _BUCKETS     # Bucket list
        ├── _BLOCKS      # Block metadata with refcounts
        ├── _PATHS       # Path allocation
        ├── _MULTIPART_PARTS  # Multipart upload state
        └── [bucket-name]     # Object metadata per bucket
```

### Multi-User API

For scenarios with multiple isolated users sharing block storage:

#### Step 1: Create Shared Block Store (Once)

```rust
use cas_storage::{SharedBlockStore, StorageEngine, Durability};
use std::sync::Arc;

// Create shared block metadata store (singleton)
let shared_block_store = Arc::new(SharedBlockStore::new(
    PathBuf::from("./data/meta"),     // Shared metadata root
    StorageEngine::Fjall,
    None,                              // inlined_metadata_size
    Some(Durability::Immediate),
)?);

// Database location: ./data/meta/blocks/db/
// Contains: _BLOCKS, _PATHS, _MULTIPART_PARTS (shared across all users)
```

#### Step 2: Create Per-User CasFS Instances

```rust
// Create CasFS for user "alice"
let alice_casfs = CasFS::new_multi_user(
    PathBuf::from("./data/blocks"),           // Shared block storage
    PathBuf::from("./data/meta/user_alice"),  // Alice's metadata root
    shared_block_store.block_tree(),          // Shared block refcounts
    shared_block_store.path_tree(),           // Shared path allocation
    shared_block_store.multipart_tree(),      // Shared multipart state
    shared_block_store.meta_store(),          // Shared metadata store
    metrics.clone(),
    StorageEngine::Fjall,
    None,
    Some(Durability::Immediate),
);

// Database location: ./data/meta/user_alice/db/
// Contains: _BUCKETS (alice's buckets), bucket trees (alice's objects)

// Create CasFS for user "bob"
let bob_casfs = CasFS::new_multi_user(
    PathBuf::from("./data/blocks"),           // Same shared blocks
    PathBuf::from("./data/meta/user_bob"),    // Bob's metadata root
    shared_block_store.block_tree(),          // Same shared refcounts
    // ... same shared trees as alice
);

// Database location: ./data/meta/user_bob/db/
```

**Database Structure (Multi-User):**
```
./data/
├── blocks/                      # Shared physical blocks (all users)
│   ├── 00/01/02/...
│   └── [deduplicated block files]
│
└── meta/
    ├── blocks/                  # Shared metadata (SINGLETON)
    │   └── db/                  # Fjall database
    │       ├── _BLOCKS          # Block refcounts (shared)
    │       ├── _PATHS           # Path allocation (shared)
    │       └── _MULTIPART_PARTS # Multipart state (shared)
    │
    ├── user_alice/              # Alice's isolated metadata
    │   └── db/                  # Fjall database
    │       ├── _BUCKETS         # Alice's bucket list
    │       └── [bucket-name]    # Alice's objects
    │
    └── user_bob/                # Bob's isolated metadata
        └── db/                  # Fjall database
            ├── _BUCKETS         # Bob's bucket list
            └── [bucket-name]    # Bob's objects
```

### Storage Operations

All operations are the same for single-user and multi-user `CasFS` instances:

```rust
// Create bucket
casfs.create_bucket("my-bucket")?;

// Check bucket exists
let exists = casfs.bucket_exists("my-bucket")?;

// List all buckets
let buckets = casfs.list_buckets()?;
for bucket in buckets {
    println!("Bucket: {} (created: {:?})", bucket.name, bucket.creation_date);
}

// Store object (async)
let (block_ids, content_hash, size) = casfs.store_object(
    "my-bucket",
    "path/to/object.jpg",
    data_stream,  // ByteStream
).await?;

// Store object with metadata (convenience method)
let object = casfs.store_single_object_and_meta(
    "my-bucket",
    "path/to/object.jpg",
    data_stream,
).await?;

// Get object metadata
let object = casfs.get_object_meta("my-bucket", "path/to/object.jpg")?;
if let Some(obj) = object {
    println!("Size: {} bytes", obj.size());
    println!("Hash: {:?}", obj.hash());
    println!("Blocks: {:?}", obj.blocks());
}

// Get object metadata + block paths for reading
let paths = casfs.get_object_paths("my-bucket", "path/to/object.jpg")?;
if let Some((object, block_paths)) = paths {
    // block_paths: Vec<(PathBuf, usize)> - (path to block file, block size)
    for (path, size) in block_paths {
        // Read block file...
    }
}

// Delete object (async)
casfs.delete_object("my-bucket", "path/to/object.jpg").await?;

// Delete bucket (async) - deletes all objects and their blocks
casfs.bucket_delete("my-bucket").await?;

// Multipart upload support
casfs.insert_multipart_part(
    bucket, key, size, part_number, upload_id,
    content_hash, block_ids
)?;

let part = casfs.get_multipart_part(bucket, key, upload_id, part_number)?;

casfs.remove_multipart_part(bucket, key, upload_id, part_number)?;
```

## Multi-User Deduplication Example

```rust
// Setup: Create shared block store
let shared = Arc::new(SharedBlockStore::new(meta_root, ...)?);

// Alice uploads a file
let alice_casfs = CasFS::new_multi_user(..., shared.clone(), ...);
alice_casfs.create_bucket("photos")?;
alice_casfs.store_single_object_and_meta(
    "photos",
    "vacation.jpg",
    vacation_data,  // 10 MB file → 10 blocks
).await?;

// Bob uploads the SAME file (different key)
let bob_casfs = CasFS::new_multi_user(..., shared.clone(), ...);
bob_casfs.create_bucket("backup")?;
bob_casfs.store_single_object_and_meta(
    "backup",
    "trip.jpg",     // Different name!
    vacation_data,  // Same 10 MB content
).await?;

// Result:
// - Physical storage: 10 MB (blocks stored once)
// - Block refcount: 2 for each of the 10 blocks
// - Alice sees: photos/vacation.jpg
// - Bob sees: backup/trip.jpg
// - Both objects point to same physical blocks

// Alice deletes her copy
alice_casfs.delete_object("photos", "vacation.jpg").await?;

// Result:
// - Block refcount: 1 for each block (decremented)
// - Blocks still exist (Bob still references them)
// - Physical storage: 10 MB (unchanged)

// Bob deletes his copy
bob_casfs.delete_object("backup", "trip.jpg").await?;

// Result:
// - Block refcount: 0 for each block
// - Blocks deleted from disk
// - Physical storage: 0 MB
```

## Storage Engines

### Fjall (Recommended for Production)

```rust
use cas_storage::{StorageEngine, Durability};

let casfs = CasFS::new(
    ...,
    StorageEngine::Fjall,
    None,
    Some(Durability::Immediate),  // Sync to disk immediately
);
```

**Characteristics:**
- ✅ True ACID transactions
- ✅ Atomic commits (blocks not visible until commit)
- ✅ True rollback on error
- ✅ Configurable durability levels
- ⚠️ Slightly slower than FjallNotx

**Durability Levels:**
- `Durability::Immediate` - Sync to disk on every commit (safest, slowest)
- `Durability::Eventual` - Async writes, periodic sync (faster, risk of data loss on crash)

### FjallNotx (Single-User or Testing)

```rust
let casfs = CasFS::new(
    ...,
    StorageEngine::FjallNotx,
    None,   // Durability ignored for FjallNotx
);
```

**Characteristics:**
- ✅ Faster writes (no transaction overhead)
- ⚠️ **Not atomic**: Blocks visible before "commit"
- ⚠️ **Best-effort rollback**: Uses cleanup list, not true rollback
- ⚠️ **Not recommended for multi-user**: Race conditions possible

**Use Cases:**
- Single-user deployments where performance is critical
- Development/testing environments
- Scenarios where eventual consistency is acceptable

## Inline Data Optimization

Small objects can be stored directly in metadata instead of separate block files:

```rust
// Configure inline threshold (default: 1 byte = effectively disabled)
let casfs = CasFS::new(
    ...,
    inlined_metadata_size: Some(4096),  // Inline objects ≤ 4 KB
);

// Objects ≤ 4 KB are stored inline
casfs.store_single_object_and_meta(
    "bucket",
    "small.txt",
    small_data,  // 2 KB
).await?;

// Object metadata:
// ObjectData::Inline { data: Vec<u8> }  ← data embedded in metadata

// Objects > 4 KB use block storage
casfs.store_single_object_and_meta(
    "bucket",
    "large.bin",
    large_data,  // 10 MB
).await?;

// Object metadata:
// ObjectData::SinglePart { blocks: Vec<BlockID> }  ← references to block files
```

**Benefits:**
- Faster access (no disk I/O for small objects)
- Reduced block metadata overhead
- Useful for many small files (logs, configs, thumbnails)

**Tradeoffs:**
- Increases metadata database size
- No deduplication for inline objects
- Recommended threshold: 1-8 KB

## Reference Counting Details

### How Refcounting Works

Every block has a refcount in the shared `_BLOCKS` tree:

```rust
pub struct Block {
    size: usize,         // Block data size
    path: Vec<u8>,       // Internal path (e.g., "00/01/02/03...")
    refcount: usize,     // Number of objects referencing this block
}
```

### Write Path

```rust
// store_object() for each block:
transaction.write_block(block_hash, size, key_has_block)?;

// Inside write_block():
if block_exists {
    if !key_has_block {
        block.increment_refcount();  // Different object references this block
    }
    // else: Same object re-uploaded, don't increment
} else {
    block = Block::new(size, path);  // rc = 1
    allocate_path();
}
```

**Key Rule**: Only increment refcount if a **different object** references the block.

### Delete Path

```rust
// delete_object():
for each block in object.blocks() {
    if block.refcount == 1 {
        // Last reference - delete block from metadata and disk
        delete_from_block_tree(block_hash);
        delete_list.push(block);
    } else {
        // Other objects still reference this block
        block.decrement_refcount();
        update_in_block_tree(block_hash, block);
    }
}

// After metadata transaction commits:
for block in delete_list {
    async_fs::remove_file(block.disk_path(fs_root)).await?;
}
```

### Key Replacement

When overwriting an object (same bucket + key):

```rust
// store_object() after writing new blocks:
MetaStore::handle_key_replacement(bucket, key, new_blocks)?;

// Inside handle_key_replacement():
let old_object = get_meta(bucket, key)?;
for old_block in old_object.blocks() {
    if !new_blocks.contains(old_block) {
        // Old block no longer needed
        if block.refcount == 1 {
            delete_from_block_tree(old_block);
        } else {
            block.decrement_refcount();
        }
    }
}
```

**Important**: Only decrements refcount for blocks **not in the new object**.

## Application Layer Responsibilities

The library provides storage primitives. Applications must implement:

### User Routing (Multi-User)

```rust
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub struct UserRouter {
    shared_block_store: Arc<SharedBlockStore>,
    casfs_cache: Arc<RwLock<HashMap<String, Arc<CasFS>>>>,
    // ... configuration
}

impl UserRouter {
    pub fn get_casfs_by_user_id(&self, user_id: &str) -> Result<Arc<CasFS>> {
        // Check cache
        if let Some(casfs) = self.casfs_cache.read().unwrap().get(user_id) {
            return Ok(casfs.clone());
        }

        // Create new CasFS for this user (lazy initialization)
        let casfs = Arc::new(CasFS::new_multi_user(
            self.fs_root.clone(),
            self.meta_root.join(format!("user_{}", user_id)),
            self.shared_block_store.block_tree(),
            // ... shared trees
        ));

        self.casfs_cache.write().unwrap().insert(user_id.to_string(), casfs.clone());
        Ok(casfs)
    }
}
```

### Authentication

```rust
// Application maps protocol credentials → user_id
fn authenticate_s3(access_key: &str, secret_key: &str) -> Option<String> {
    // Validate credentials, return user_id
}

fn authenticate_resp(username: &str, password: &str) -> Option<String> {
    // Validate credentials, return user_id
}

// Then route to correct CasFS
let user_id = authenticate_s3(req.access_key, req.secret_key)?;
let casfs = user_router.get_casfs_by_user_id(&user_id)?;
```

### Protocol Adaptation

```rust
// Example: S3 protocol adapter
impl s3s::S3 for S3FS {
    async fn put_object(&self, req: S3Request<PutObjectInput>) -> S3Result<...> {
        // 1. Extract parameters from S3 request
        let bucket = req.input.bucket;
        let key = req.input.key;
        let body = req.input.body;

        // 2. Call library
        let object = self.casfs.store_single_object_and_meta(
            &bucket, &key, body
        ).await?;

        // 3. Format S3 response
        Ok(S3Response::new(PutObjectOutput {
            e_tag: Some(object.format_e_tag()),
            ...
        }))
    }
}

// Example: RESP protocol adapter
async fn handle_resp_set(key: &str, value: &[u8]) -> Result<()> {
    // RESP "SET mykey myvalue"
    // Map to: bucket="data", key="mykey"
    casfs.store_single_object_and_meta("data", key, value).await?;
    Ok(())
}
```

## Performance Characteristics

### Deduplication Efficiency

- **Block size**: 1 MiB (configurable via `BLOCK_SIZE` constant)
- **Smaller blocks**: More deduplication, more metadata overhead
- **Larger blocks**: Less deduplication, less metadata overhead
- **1 MiB default**: Good balance for typical file workloads

### Concurrent Writes

```rust
// store_object() writes up to 5 blocks concurrently
const MAX_CONCURRENT_BLOCK_WRITES: usize = 5;
```

### Metadata Overhead

Per object:
- Bucket metadata: ~100 bytes
- Object metadata: ~100-200 bytes + (num_blocks × 16 bytes)
- Block metadata (shared): ~50 bytes per unique block

Per block file:
- Filesystem overhead: ~4 KB (typical filesystem block size)
- Actual block size: up to 1 MiB

### Recommended Configurations

**High-throughput, multi-user:**
```rust
StorageEngine::Fjall
Durability::Eventual  // Faster, acceptable for most use cases
inlined_metadata_size: Some(4096)  // Inline small objects
```

**Maximum safety, single-user:**
```rust
StorageEngine::Fjall
Durability::Immediate  // Sync every write
inlined_metadata_size: None  // Disable inlining
```

**Development/testing:**
```rust
StorageEngine::FjallNotx  // Faster, no transaction overhead
inlined_metadata_size: Some(8192)
```

## Error Handling

```rust
use cas_storage::MetaError;

match casfs.get_object_meta("bucket", "key") {
    Ok(Some(object)) => { /* found */ },
    Ok(None) => { /* not found */ },
    Err(MetaError::BucketNotFound(bucket)) => { /* bucket doesn't exist */ },
    Err(MetaError::TreeNotFound(name)) => { /* internal tree missing */ },
    Err(e) => { /* other error */ },
}
```

## Migration Guide

### From Monolithic s3-cas to Library

See [ADR 003](adr/003-cas-storage-library-extraction.md) for detailed migration plan.

**Summary:**
1. Add `cas-storage` dependency to your application
2. Update imports: `use cas_storage::CasFS;`
3. Keep protocol-specific code (S3, auth, UI) in application layer
4. Use library for all storage operations

## References

- [ADR 001: Multi-User Authentication](adr/001-multi-user-authentication.md)
- [ADR 003: CAS Storage Library Extraction](adr/003-cas-storage-library-extraction.md)
- [Reference Counting Documentation](refcount.md)
- [CLAUDE.md](../CLAUDE.md) - Complete codebase function map
