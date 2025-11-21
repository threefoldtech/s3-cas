```markdown
# S3-CAS Codebase Function Map and Call Graph

## Module Structure

### cas/fs.rs - CasFS (Content Addressable Storage FileSystem)

**Key Functions:**

**`CasFS::new(root, meta_path, metrics, storage_engine, inlined_metadata_size, durability)`**
- Creates CasFS instance with storage backend initialization
- Sets up metastore, multipart tree, and block storage root

**`CasFS::get_object_paths(&self, bucket_name: &str, key: &str)`**
- Returns Object metadata and filesystem paths to blocks
- Handles both inlined and block-stored objects
- Returns (Object, Vec<(PathBuf, usize)>)

**`CasFS::bucket_delete(&self, bucket_name: &str)` (async)**
- Deletes bucket and all contained objects
- Cascades deletion to all object blocks

**`CasFS::delete_object(&self, bucket: &str, key: &str)` (async)**
- Deletes object and its blocks from disk
- Decrements/removes block refcounts
- Removes path mappings

**`CasFS::store_single_object_and_meta(&self, bucket_name, key, data)` (async)**
- Convenience wrapper combining store_object + create_object_meta

**`CasFS::store_object(&self, bucket_name, key, data)` (async) [CRITICAL]**
- Streams bytes, chunks into BLOCK_SIZE (1MiB) chunks
- MD5 hashes each chunk and full stream
- Writes blocks to disk with concurrent writes (up to 5 concurrent)
- Handles refcount management for duplicate blocks via transactions
- Returns (block_ids, content_hash, total_size)
- **KEY LOGIC**: Checks if old object exists, captures old_blocks, calls handle_key_replacement after storing new blocks

**`CasFS::store_inlined_object(&self, bucket_name, key, data)`**
- Stores small object data directly in metadata (bypasses block storage)

---

### metastore/meta_store.rs - MetaStore

**Key Functions:**

**`MetaStore::new(store, inlined_metadata_size)`**
- Creates MetaStore with storage backend
- Default inlined metadata size: 1 byte (effectively disabled)

**Tree Access (returns specialized metadata trees):**
- `get_allbuckets_tree()` - "_BUCKETS"
- `get_bucket_ext(name)` - Bucket tree with range filtering
- `get_block_tree()` - "_BLOCKS"
- `get_path_tree()` - "_PATHS"
- `get_tree(name)` - Arbitrary tree (for multipart)

**Bucket Operations:**
- `bucket_exists(name)`, `drop_bucket(name)`, `insert_bucket(name, raw_bucket)`, `list_buckets()`

**Object Metadata:**
- `insert_meta(bucket, key, raw_obj)`, `get_meta(bucket, key)`

**`MetaStore::delete_object(&self, bucket, key)` [CRITICAL]**
- Removes object from bucket
- Decrements/removes block refcounts
- Returns blocks to physically delete (refcount reached 0)
- **REFCOUNT LOGIC**: Decrements rc, deletes if rc==1

**`MetaStore::handle_key_replacement(&self, bucket, key, new_blocks)` [CRITICAL - NEW]**
- Called after storing new object with same key
- Decrements refcount on blocks no longer used
- Returns blocks to physically delete
- **REFCOUNT LOGIC**: Only touches old blocks not in new_blocks list

**Other Methods:**
- `begin_transaction()`, `num_keys()`, `disk_space()`, `get_underlying_store()`

---

### metastore/meta_store.rs - Transaction

**`Transaction::write_block(&mut self, block_hash, data_len, key_has_block)` [CRITICAL]**
- Gets or creates block metadata
- If block exists and key doesn't have it: increments refcount
- If block exists and key has it: returns without incrementing
- If block doesn't exist: creates with refcount=1, allocates path, creates path entry
- Returns (is_new_block, block_metadata)
- **REFCOUNT LOGIC**: Increments refcount if !key_has_block

**Other Methods:**
- `commit()`, `rollback()`

---

### metastore/block.rs - Block

**`Block::new(size, path)`** - Creates block with refcount=1

**Refcount Methods:**
- `increment_refcount()`, `decrement_refcount()`, `rc()` - Refcount management

**Other Methods:**
- `disk_path(root)` - Constructs hierarchical filesystem path from path bytes
- Standard getters: `size()`, `path()`, `to_vec()`

---

### metastore/object.rs - Object

**`Object::new(size, hash, object_data)`** - Creates object with current timestamp

**Key Methods:**
- `format_e_tag()` - S3 ETag (includes part count for multipart)
- `has_block(block)` - Checks if object contains specific block
- `is_inlined()`, `inlined()` - Inline data accessors
- `blocks()` - Returns block IDs (empty for inline)

**Standard Accessors:**
- `size()`, `hash()`, `last_modified()`, `touch()`, `to_vec()`, `minimum_inline_metadata_size()`

---

### s3fs.rs - S3FS

**`S3FS::new(casfs, metrics)`** - Wraps CasFS for S3 API

**`calculate_multipart_hash(&self, blocks)`** - Calculates S3 multipart ETag (MD5 of MD5s)

**S3 Trait Methods (all async):**
- `complete_multipart_upload()` - Collects parts, creates final object, removes part metadata
- `create_multipart_upload()` - Generates UUID upload_id
- `upload_part()` - Stores part, inserts multipart metadata
- `put_object()` - Inline storage for small objects, otherwise calls store_single_object_and_meta
- `get_object()` - Returns inline data or BlockStream, supports range requests
- `delete_object()`, `delete_objects()` - Object deletion (single/batch)
- `create_bucket()`, `delete_bucket()`, `list_buckets()` - Bucket management
- `list_objects()`, `list_objects_v2()` - Object listing with pagination
- `head_bucket()`, `head_object()`, `get_bucket_location()` - Metadata queries
- `copy_object()` - Not implemented

---

### cas/block_stream.rs - BlockStream

**`BlockStream::new(paths, size, range, metrics)`**
- Creates stream over multiple block files
- Handles range requests and seeks

**`poll_next()`** - Implements Stream trait, manages file I/O, applies range filtering

---

### cas/range_request.rs - RangeRequest

**Enum:** `All`, `Range(u64, u64)`, `ToBytes(u64)`, `FromBytes(u64)`

**`parse_range_request(input)`** - Parses S3 Range header (e.g., "bytes=0-1023")

---

### stores/fjall.rs & fjall_notx.rs - Storage Backends

**FjallStore (transactional):**
- `new(path, inlined_metadata_size, durability)` - Opens transactional keyspace
- `get_partition(name)`, `commit_persist(tx)` - Partition and transaction management

**FjallStoreNotx (non-transactional):**
- `new(path, inlined_metadata_size)` - Opens non-transactional keyspace
- `rollback()` - Simulates rollback by removing inserted keys

---

### metrics.rs - Metrics & MetricFs

**Prometheus Metrics:**
- Method invocations, bucket count, data bytes (received/sent/written)
- Block operations (written/ignored/pending/errors/dropped)

**`MetricFs<T>`** - Wraps S3 implementation to record metrics

---

## Multi-User Authentication Architecture

### Overview
- **Dual credentials**: Separate UI (login/password) and S3 (access_key/secret_key)
- **Storage**: User data in Fjall partitions (_USERS, _USERS_BY_LOGIN, _USERS_BY_S3_KEY)
- **Session management**: In-memory sessions with 24-hour lifetime
- **Per-user routing**: S3UserRouter extracts access_key and routes to user's S3FS
- **First-time setup**: When no users exist, /login shows setup form to create admin account
- **Lazy initialization**: CasFS instances created on-demand (not at startup)
- **Dynamic authentication**: S3 credentials validated by querying UserStore on each request

### auth/user_store.rs - UserStore

**UserRecord struct:**
```rust
pub struct UserRecord {
    pub user_id: String,           // Primary key
    pub ui_login: String,          // HTTP UI username
    pub ui_password_hash: String,  // bcrypt DEFAULT_COST (12)
    pub s3_access_key: String,     // S3 access key (20 chars)
    pub s3_secret_key: String,     // S3 secret key (40 chars)
    pub is_admin: bool,            // Admin privileges
    pub created_at: u64,           // UNIX timestamp
}
```

**Stored in Fjall partitions:**
- `_USERS` - Primary storage (key: user_id)
- `_USERS_BY_LOGIN` - Index (ui_login → user_id)
- `_USERS_BY_S3_KEY` - Index (s3_access_key → user_id)

**Key Methods:**
- `UserStore::new(store)` - Opens three partitions
- `create_user(user)` - Validates uniqueness, inserts with indices
- `get_user_by_id/ui_login/s3_key(...)` - Retrieves via primary key or indices
- `delete_user(user_id)` - Removes from all partitions/indices
- `update_password(user_id, new_password)` - Rehashes with bcrypt (caller must invalidate sessions)
- `authenticate(ui_login, password)` - Lookup + bcrypt verification
- `list_users()`, `count_users()` - Enumeration

### auth/session.rs - SessionStore

**SessionData struct:**
```rust
pub struct SessionData {
    pub user_id: String,
    pub created_at: Instant,
}
```

**Storage:** In-memory `HashMap<String, SessionData>`, lost on restart

**Key Methods:**
- `create_session(user_id)` - Generates 32-byte random session_id (64 hex chars)
- `get_session(session_id)` - Returns user_id if valid and not expired
- `delete_session(session_id)` - Logout
- `delete_user_sessions(user_id)` - Removes all sessions for user
- `cleanup_expired()` - Periodic cleanup

### auth/router.rs - UserRouter

**Purpose:** Manages per-user CasFS instances with lazy initialization

**`UserRouter::new(shared_block_store, fs_root, meta_root, metrics, storage_engine, inlined_metadata_size, durability)`**
- Creates router with shared block metadata store
- Initializes empty CasFS cache (no users loaded at startup)
- Stores CasFS configuration parameters

**`UserRouter::get_casfs_by_user_id(&self, user_id)`** [Core routing logic]
- Uses double-checked locking pattern for thread-safe lazy initialization
- Checks read lock for cached CasFS
- If not found: acquires write lock, creates CasFS for user, caches it
- Returns Arc<CasFS> for user

**`UserRouter::create_casfs_for_user(&self, user_id)`** [Internal]
- Creates user-specific CasFS instance with shared block store
- Uses per-user metadata partition (meta_root/user_id)
- All users share same block metadata but have separate object metadata

### s3_wrapper.rs - DynamicS3Auth & S3UserRouter

**`DynamicS3Auth`** - Dynamic S3 credential validation

**`DynamicS3Auth::new(user_store)`**
- Creates authenticator with reference to UserStore

**`DynamicS3Auth::get_secret_key(&self, access_key)` (async)** [S3Auth trait]
- Queries UserStore::get_user_by_s3_key() on each request
- Returns user's secret_key if found
- Returns InvalidAccessKeyId error if not found
- Uses constant-time comparison (subtle crate) for secret validation

**`S3UserRouter`** - Per-request user routing

**`S3UserRouter::get_s3fs_for_request<T>(&self, req)`** [Core routing logic]
- Extracts access_key from req.credentials
- Looks up user via UserStore::get_user_by_s3_key()
- Gets user's CasFS from UserRouter::get_casfs_by_user_id() (lazy initialization)
- Returns S3FS instance for this request

**S3 Trait Methods:** All follow pattern: call get_s3fs_for_request() → forward to user's S3FS → return result
- Provides complete per-user isolation for all S3 operations

### http_ui/middleware.rs - SessionAuth

**AuthContext:** `{ user_id: String, is_admin: bool }`

**`SessionAuth::authenticate(&self, req)`** [Core auth logic]
- Extracts session_id from cookie
- Validates via SessionStore, retrieves user from UserStore
- Returns `Option<AuthContext>`

**Cookie Management:**
- `create_session_cookie(session_id)` - HttpOnly, SameSite=Strict, Max-Age=24h
- `clear_session_cookie()` - Max-Age=0

**Path Guards:**
- `is_public_path(path)` - /login, /setup-admin, /health
- `is_admin_path(path)` - /admin/*

**Response Helpers:**
- `login_redirect_response(original_path)`, `forbidden_response()`

### http_ui/login.rs - Login Handlers

**First-Time Setup Flow:**
- `handle_login_page()` - Checks user count; if 0, shows setup form instead of login form
- `handle_setup_admin()` - Creates first admin user:
  - Validates no users exist (prevents duplicate setup)
  - Validates password confirmation and 8-character minimum
  - Auto-generates S3 credentials (access_key: 20 chars, secret_key: 40 chars)
  - Creates UserRecord with is_admin=true
  - Creates session and redirects to /profile?setup=1 with credentials in query params
  - Credentials shown once with warning to save them

**Normal Login Flow:**
- `handle_login_page()` - Returns HTML login form
- `handle_login_submit()` - Authenticates, creates session, sets cookie, redirects
- `handle_logout()` - Deletes session, clears cookie, redirects to /login

**Helpers:** `generate_access_key()` (20 chars), `generate_secret_key()` (40 chars)

### http_ui/admin.rs - Admin Panel

**User Management:**
- `handle_list_users()` - Shows all users with details
- `handle_new_user_form()` - User creation form
- `handle_create_user()` - Auto-generates credentials if not provided (password: 16, access_key: 20, secret_key: 40)
- `handle_delete_user()` - Deletes user + all sessions
- `handle_reset_password_form()`, `handle_update_password()` - Password reset flow

**Helpers:** `generate_access_key()`, `generate_secret_key()`, `generate_password()`

### http_ui/mod.rs - HTTP UI Services

**`HttpUiServiceMultiUser::route_request(&self, req)`**
- Checks public paths → authenticates → checks admin paths → routes to handlers

**`HttpUiServiceEnum`** - Wrapper supporting both single-user (Basic Auth) and multi-user (Session Auth) modes

---

## Call Graph
```

main() [server startup]
└─ CasFS::new()
├─ FjallStore::new() or FjallStoreNotx::new()
└─ MetaStore::new()
└─ S3FS::new()
└─ MetricFs::new()

S3 PUT OBJECT
└─ S3FS::put_object()
└─ CasFS::store_single_object_and_meta()
├─ CasFS::store_object() [MAIN WRITE PATH]
│ ├─ BufferedByteStream [chunking into 1MiB]
│ ├─ Md5::digest [full stream hash]
│ ├─ For each chunk (concurrent):
│ │ ├─ Md5::digest [chunk hash = BlockID]
│ │ ├─ MetaStore::begin_transaction()
│ │ ├─ Transaction::write_block() [KEY REFCOUNT LOGIC]
│ │ │ ├─ Check if block exists
│ │ │ ├─ If exists & !key_has_block: increment_refcount()
│ │ │ └─ If new: create with rc=1, allocate path
│ │ ├─ Write block to disk (async_fs::write)
│ │ └─ Transaction::commit/rollback()
│ └─ MetaStore::handle_key_replacement() [NEW - CLEANUP OLD BLOCKS]
│ └─ For old blocks not in new set:
│ └─ Block::decrement_refcount() or delete
└─ CasFS::create_object_meta()
└─ MetaStore::insert_meta()

S3 GET OBJECT
└─ S3FS::get_object()
└─ CasFS::get_object_paths()
├─ CasFS::get_object_meta()
│ └─ MetaStore::get_meta()
└─ BlockTree::get_block() [for each block]
└─ If inline:
└─ Return data directly
└─ Else:
└─ BlockStream::new()
└─ BlockStream::poll_next() [streaming reads]

S3 DELETE OBJECT
└─ S3FS::delete_object()
└─ CasFS::delete_object() [async]
├─ MetaStore::delete_object() [KEY REFCOUNT LOGIC]
│ └─ For each block in object:
│ ├─ Block::decrement_refcount()
│ └─ If rc==0: delete block from metastore
└─ For each deletable block:
└─ async_fs::remove_file()

S3 MULTIPART UPLOAD
└─ S3FS::create_multipart_upload()
└─ Generate UUID
└─ S3FS::upload_part() [per part, async]
├─ CasFS::store_object() [stores part, doesn't create object metadata]
└─ CasFS::insert_multipart_part()
└─ MultiPartTree::insert()
└─ S3FS::complete_multipart_upload()
├─ For each part:
│ └─ CasFS::get_multipart_part()
│ └─ MultiPartTree::get_multipart_part()
├─ S3FS::calculate_multipart_hash()
├─ CasFS::create_object_meta() [creates final object]
└─ For each part:
└─ CasFS::remove_multipart_part()
└─ MultiPartTree::remove()

BUCKET OPERATIONS
└─ S3FS::create_bucket()
└─ CasFS::create_bucket()
└─ MetaStore::insert_bucket()
└─ S3FS::delete_bucket()
└─ CasFS::bucket_delete() [async]
├─ MetaStore::get_allbuckets_tree()
├─ MetaStore::get_bucket_ext()
├─ For each object in bucket:
│ └─ CasFS::delete_object() [cascades]
└─ MetaStore::drop_bucket()
└─ S3FS::list_buckets()
└─ MetaStore::list_buckets()

LIST OPERATIONS
└─ S3FS::list_objects() / list_objects_v2()
└─ CasFS::get_bucket()
└─ MetaStore::get_bucket_ext()
└─ MetaTreeExt::range_filter() [with prefix/start_after/token]

MULTI-USER AUTHENTICATION FLOWS

HTTP UI LOGIN
└─ login::handle_login_submit()
   ├─ UserStore::authenticate(ui_login, password)
   │  ├─ get_user_by_ui_login()
   │  └─ UserRecord::verify_password() [bcrypt]
   ├─ SessionStore::create_session(user_id)
   │  └─ Generate random session_id (32 bytes = 64 hex chars)
   └─ Set session cookie (HttpOnly, SameSite=Strict, Max-Age=24h)

HTTP UI AUTHENTICATED REQUEST
└─ HttpUiServiceMultiUser::route_request()
   ├─ SessionAuth::authenticate(req)
   │  ├─ Extract session_id from cookie
   │  ├─ SessionStore::get_session()
   │  └─ UserStore::get_user_by_id()
   ├─ Check path guards (is_admin_path?)
   ├─ UserRouter::get_casfs(access_key)
   └─ Handle request with user's CasFS

S3 API MULTI-USER REQUEST
└─ S3UserRouter::{s3_method}()
   └─ get_s3fs_for_request()
      ├─ Extract access_key from req.credentials
      ├─ UserStore::get_user_by_s3_key(access_key)
      ├─ UserRouter::get_casfs(access_key)
      ├─ S3FS::new(casfs, metrics)
      └─ s3fs.{s3_method}(req)

HTTP UI LOGOUT
└─ login::handle_logout()
   ├─ Extract session_id from cookie
   ├─ SessionStore::delete_session(session_id)
   ├─ Clear session cookie (Max-Age=0)
   └─ Redirect to /login

ADMIN USER CREATION
└─ admin::handle_create_user()
   ├─ Generate random password (16 chars) if not provided
   ├─ Generate S3 keys (access: 20 chars, secret: 40 chars) if not provided
   ├─ UserRecord::new() [hashes password with bcrypt]
   └─ UserStore::create_user()
      ├─ Insert into _USERS partition
      ├─ Index in _USERS_BY_LOGIN
      └─ Index in _USERS_BY_S3_KEY

ADMIN PASSWORD RESET
└─ admin::handle_update_password()
   ├─ UserStore::update_password(user_id, new_password)
   │  └─ Hash new password with bcrypt
   ├─ SessionStore::delete_user_sessions(user_id)
   └─ Redirect to user list

ADMIN USER DELETION
└─ admin::handle_delete_user()
   ├─ SessionStore::delete_user_sessions(user_id)
   ├─ UserStore::delete_user(user_id)
   │  ├─ Remove from _USERS partition
   │  ├─ Remove from _USERS_BY_LOGIN index
   │  └─ Remove from _USERS_BY_S3_KEY index
   └─ Redirect to user list

MULTI-USER MODE STARTUP (main.rs)
└─ run_multi_user()
   ├─ SharedBlockStore::new() [shared block metadata]
   ├─ UserStore::new(shared_store) [user database]
   ├─ SessionStore::new() [in-memory sessions]
   ├─ UserRouter::new(shared_block_store, ...) [empty CasFS cache]
   ├─ Check user_store.count_users()
   │  └─ If 0: Log "First user will be created through HTTP UI setup"
   ├─ DynamicS3Auth::new(user_store) [dynamic credential validation]
   ├─ S3UserRouter::new(user_router, user_store) [S3 per-request routing]
   ├─ HttpUiServiceMultiUser::new(user_router, user_store, session_store, metrics) [session-based HTTP UI]
   ├─ S3ServiceBuilder::new(s3_user_router).set_auth(dynamic_auth).build()
   └─ run_server()

````

---

## Reference Counting Flow

### New Block Creation
1. `Transaction::write_block()` called with `key_has_block=false`
2. Block doesn't exist in DB
3. Creates `Block::new(size, path)` → `rc=1`
4. Stores in block tree

### Duplicate Block (Same Key)
1. `Transaction::write_block()` called with `key_has_block=true`
2. Block exists in DB
3. **Does NOT** increment refcount
4. Returns existing block

### Duplicate Block (Different Key)
1. First object stored with block → `rc=1`
2. Second object stored with same block hash
3. `Transaction::write_block()` called with `key_has_block=false` (different key)
4. Block exists in DB, `!key_has_block=true`
5. **Increments** refcount → `rc=2`

### Key Replacement (OLD FIX NEEDED)
1. Key already has object with blocks [A, B]
2. New content hashes to blocks [B, C]
3. `store_object()` completes with new blocks [B, C]
4. **FIX**: Call `MetaStore::handle_key_replacement()` with new_blocks=[B,C]
5. For block A (not in new_blocks):
   - If `rc==1`: delete block, delete from disk
   - If `rc>1`: decrement rc, update block

### Object Deletion
1. `MetaStore::delete_object()` called
2. Gets object, loops through blocks
3. For each block:
   - If `rc==1`: remove from block tree, add to delete_list
   - If `rc>1`: decrement rc, update in block tree
4. Caller deletes files from disk for delete_list

---

## Data Structures

```rust
// Core data types
type BlockID = [u8; 16];  // MD5 hash

enum ObjectData {
    Inline { data: Vec<u8> },                           // Small object data
    SinglePart { blocks: Vec<BlockID> },                // Regular upload
    MultiPart { blocks: Vec<BlockID>, parts: usize },   // Multipart upload
}

enum RangeRequest {
    All,
    Range(u64, u64),      // Inclusive range
    ToBytes(u64),         // From start
    FromBytes(u64),       // To end
}

// Multi-user authentication (detailed in module docs above)
struct UserRecord { user_id, ui_login, ui_password_hash, s3_access_key, s3_secret_key, is_admin, created_at }
struct SessionData { user_id, created_at }
struct AuthContext { user_id, is_admin }
```

---

## Recent Major Changes

- **ADR 003**: Extracted CAS storage layer as reusable library (`cas-storage` crate)
- **Multi-user authentication**: Session-based auth with admin panel and per-user isolation
- **HTTP UI enhancements**: Infinite scroll pagination for object listings
- **S3s upgrade**: Migrated to v0.11.1 tagged release for stability
- **Performance**: Optimized path handling to eliminate getcwd() syscalls in async operations

## Architecture Decisions

- **Library extraction**: Core CAS/metadata logic separated into `cas-storage` crate for reusability across different protocols
- **Metrics abstraction**: Trait-based `MetricsCollector` system allows pluggable implementations (Prometheus, StatsD, etc.)
- **Multi-user isolation**: Per-user CasFS instances with shared block-level deduplication
- **Storage backends**: Transactional (fjall) vs non-transactional (fjall_notx) modes for different durability/performance tradeoffs

---

## Key Constants

### Storage and Block Management
- `BLOCK_SIZE = 1 << 20` = 1 MiB (chunk size for streaming)
- `BLOCKID_SIZE = 16` (MD5 hash size)
- `PTR_SIZE = usize` size (typically 8 bytes)
- `DEFAULT_BUCKET_TREE = "_BUCKETS"`
- `DEFAULT_BLOCK_TREE = "_BLOCKS"`
- `DEFAULT_PATH_TREE = "_PATHS"`
- `DEFAULT_INLINED_METADATA_SIZE = 1` (effectively disabled)

### Authentication (Multi-User Mode)
- `SESSION_COOKIE_NAME = "session_id"`
- `SESSION_ID_BYTES = 32` (generates 64 hex characters)
- `DEFAULT_SESSION_LIFETIME = 24 hours` (86400 seconds)
- `COOKIE_MAX_AGE = 24 * 60 * 60` seconds
- `USERS_TREE = "_USERS"` (primary user storage partition)
- `USERS_BY_LOGIN_TREE = "_USERS_BY_LOGIN"` (ui_login → user_id index)
- `USERS_BY_S3_KEY_TREE = "_USERS_BY_S3_KEY"` (s3_access_key → user_id index)
- `bcrypt DEFAULT_COST = 12` (password hashing cost)
- `RANDOM_PASSWORD_LENGTH = 16` (for initial user creation)

---

## Important Notes for Implementation

1. **Refcount Bug Fix**: `CasFS::store_object()` must call `MetaStore::handle_key_replacement()` after getting new_blocks to handle old block cleanup

2. **Transaction Safety**: `Transaction::write_block()` MUST always increment refcount on duplicate blocks from different keys, never fail to increment (data loss risk)

3. **Failure Handling**: Data leakage acceptable (not decrementing refcount on delete), but never data loss (must increment on new references)

4. **Async Operations**: `store_object()`, `delete_object()`, `bucket_delete()` are async and use concurrent operations (limit 5 concurrent block writes)

5. **Streaming**: Objects read via `BlockStream` which manages multiple file handles and range requests

6. **Inlining**: Objects ≤ `max_inlined_data_length()` stored directly in Object metadata instead of separate blocks

7. **Multipart**: Parts stored separately, only combined into final object on complete_multipart_upload

```

```
