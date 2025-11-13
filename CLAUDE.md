```markdown
# S3-CAS Codebase Function Map and Call Graph

## Module Structure

### cas/fs.rs - CasFS (Content Addressable Storage FileSystem)

#### Core Functions

**`CasFS::new(root: PathBuf, meta_path: PathBuf, metrics: SharedMetrics, storage_engine: StorageEngine, inlined_metadata_size: Option<usize>, durability: Option<Durability>) -> Self`**

- Creates a new CasFS instance with storage backend initialization
- Sets up metastore and multipart tree
- Initializes block storage root directory

**`CasFS::path_tree(&self) -> Result<Box<dyn BaseMetaTree>, MetaError>`**

- Returns path metadata tree for tracking disk block locations
- Used internally for managing block paths

**`CasFS::fs_root(&self) -> &PathBuf`**

- Returns reference to filesystem root for block storage

**`CasFS::max_inlined_data_length(&self) -> usize`**

- Returns maximum size for inlining object data in metadata
- Returns 0 if inlining disabled

**`CasFS::get_bucket(&self, bucket_name: &str) -> Result<Box<dyn MetaTreeExt + Send + Sync>, MetaError>`**

- Retrieves extended tree interface for a specific bucket
- Enables range filtering and listing operations

**`CasFS::block_tree(&self) -> Result<BlockTree, MetaError>`**

- Returns specialized block metadata tree
- Used for block refcount and metadata operations

**`CasFS::bucket_exists(&self, bucket_name: &str) -> Result<bool, MetaError>`**

- Checks if bucket exists in metastore

**`CasFS::create_object_meta(&self, bucket_name: &str, key: &str, size: u64, hash: BlockID, object_data: ObjectData) -> Result<Object, MetaError>`**

- Creates Object metadata and inserts into metastore
- Returns constructed Object

**`CasFS::get_object_meta(&self, bucket_name: &str, key: &str) -> Result<Option<Object>, MetaError>`**

- Retrieves deserialized Object metadata from bucket

**`CasFS::get_object_paths(&self, bucket_name: &str, key: &str) -> Result<Option<ObjectPaths>, MetaError>`**

- Returns Object metadata and filesystem paths to blocks
- Handles both inlined and block-stored objects
- Returns tuple of (Object, Vec<(PathBuf, usize)>)

**`CasFS::create_bucket(&self, bucket_name: &str) -> Result<(), MetaError>`**

- Creates new bucket and initializes metadata

**`CasFS::bucket_delete(&self, bucket_name: &str) -> Result<(), MetaError>` (async)**

- Deletes bucket and all contained objects
- Cascades deletion to all object blocks

**`CasFS::insert_multipart_part(&self, bucket: String, key: String, size: usize, part_number: i64, upload_id: String, hash: BlockID, blocks: Vec<BlockID>) -> Result<(), MetaError>`**

- Stores multipart upload part metadata
- Maps part number to MultiPart struct

**`CasFS::get_multipart_part(&self, bucket: &str, key: &str, upload_id: &str, part_number: i64) -> Result<Option<MultiPart>, MetaError>`**

- Retrieves MultiPart metadata for specific part

**`CasFS::remove_multipart_part(&self, bucket: &str, key: &str, upload_id: &str, part_number: i64) -> Result<(), MetaError>`**

- Removes MultiPart metadata after upload completion

**`CasFS::key_exists(&self, bucket: &str, key: &str) -> Result<bool, MetaError>`**

- Checks if object key exists in bucket

**`CasFS::list_buckets(&self) -> Result<Vec<BucketMeta>, MetaError>`**

- Returns all buckets with metadata

**`CasFS::delete_object(&self, bucket: &str, key: &str) -> Result<(), MetaError>` (async)**

- Deletes object and its blocks from disk
- Decrements/removes block refcounts
- Removes path mappings

**`CasFS::store_single_object_and_meta(&self, bucket_name: &str, key: &str, data: ByteStream) -> Result<Object, io::Result>` (async)**

- Convenience function combining store_object + create_object_meta
- Returns constructed Object

**`CasFS::store_object(&self, bucket_name: &str, key: &str, data: ByteStream) -> Result<(Vec<BlockID>, BlockID, u64), io::Result>` (async)**

- Streams bytes, chunks into BLOCK_SIZE (1MiB) chunks
- MD5 hashes each chunk and full stream
- Writes blocks to disk with concurrent writes (up to 5 concurrent)
- Handles refcount management for duplicate blocks
- Performs transaction commit/rollback
- Returns (block_ids, content_hash, total_size)
- **KEY LOGIC**: Checks if old object exists, captures old_blocks, calls handle_key_replacement after storing new blocks

**`CasFS::store_inlined_object(&self, bucket_name: &str, key: &str, data: Vec<u8>) -> Result<Object, MetaError>`**

- Stores small object data directly in metadata
- MD5 hashes data
- Returns Object with Inline data variant

---

### metastore/meta_store.rs - MetaStore

**`MetaStore::new(store: impl Store + 'static, inlined_metadata_size: Option<usize>) -> Self`**

- Creates MetaStore with storage backend
- Sets inlined metadata size (default 1 byte, effectively disabled)

**`MetaStore::max_inlined_data_length(&self) -> usize`**

- Returns max inlined data length accounting for Object overhead

**`MetaStore::get_allbuckets_tree(&self) -> Result<Box<dyn MetaTreeExt + Send + Sync>, MetaError>`**

- Returns bucket list tree (DEFAULT_BUCKET_TREE = "\_BUCKETS")

**`MetaStore::get_bucket_ext(&self, name: &str) -> Result<Box<dyn MetaTreeExt + Send + Sync>, MetaError>`**

- Returns extended tree for bucket with range filtering

**`MetaStore::get_block_tree(&self) -> Result<BlockTree, MetaError>`**

- Returns block metadata tree (DEFAULT_BLOCK_TREE = "\_BLOCKS")

**`MetaStore::get_tree(&self, name: &str) -> Result<Box<dyn BaseMetaTree>, MetaError>`**

- Returns arbitrary named tree (used for multipart)

**`MetaStore::get_path_tree(&self) -> Result<Box<dyn BaseMetaTree>, MetaError>`**

- Returns path tracking tree (DEFAULT_PATH_TREE = "\_PATHS")

**`MetaStore::bucket_exists(&self, bucket_name: &str) -> Result<bool, MetaError>`**

- Checks if bucket tree exists

**`MetaStore::drop_bucket(&self, name: &str) -> Result<(), MetaError>`**

- Deletes bucket tree if exists

**`MetaStore::insert_bucket(&self, bucket_name: &str, raw_bucket: Vec<u8>) -> Result<(), MetaError>`**

- Inserts bucket metadata into buckets tree
- Creates bucket tree

**`MetaStore::list_buckets(&self) -> Result<Vec<BucketMeta>, MetaError>`**

- Deserializes all buckets from buckets tree

**`MetaStore::insert_meta(&self, bucket_name: &str, key: &str, raw_obj: Vec<u8>) -> Result<(), MetaError>`**

- Inserts Object metadata into bucket tree

**`MetaStore::get_meta(&self, bucket_name: &str, key: &str) -> Result<Option<Object>, MetaError>`**

- Retrieves and deserializes Object from bucket

**`MetaStore::delete_object(&self, bucket: &str, key: &str) -> Result<Vec<Block>, MetaError>`**

- Removes object from bucket
- Decrements/removes block refcounts
- Returns blocks to physically delete (refcount reached 0)
- **REFCOUNT LOGIC**: Decrements rc, deletes if rc==1

**`MetaStore::handle_key_replacement(&self, bucket: &str, key: &str, new_blocks: &[BlockID]) -> Result<Vec<Block>, MetaError>`** (NEW)

- Called after storing new object with same key
- Decrements refcount on blocks no longer used
- Returns blocks to physically delete
- **REFCOUNT LOGIC**: Only touches old blocks not in new_blocks list

**`MetaStore::begin_transaction(&self) -> Transaction`**

- Creates transaction for atomic metadata operations

**`MetaStore::num_keys(&self) -> usize`**

- Returns count of keys in bucket tree

**`MetaStore::disk_space(&self) -> u64`**

- Returns total disk space used by metastore

---

### metastore/meta_store.rs - Transaction

**`Transaction::new(backend: Box<dyn TransactionBackend>) -> Self`**

- Creates transaction wrapper around backend

**`Transaction::commit(mut self) -> Result<(), MetaError>`**

- Commits transaction, making changes permanent

**`Transaction::rollback(mut self)`**

- Rolls back transaction, discards all changes

**`Transaction::write_block(&mut self, block_hash: BlockID, data_len: usize, key_has_block: bool) -> Result<(bool, Block), MetaError>`**

- Gets or creates block metadata
- If block exists and key doesn't have it: increments refcount
- If block exists and key has it: returns without incrementing
- If block doesn't exist: creates with refcount=1, allocates path, creates path entry
- Returns (is_new_block, block_metadata)
- **REFCOUNT LOGIC**: Increments refcount if !key_has_block

---

### metastore/block.rs - Block

**`Block::new(size: usize, path: Vec<u8>) -> Self`**

- Creates block with refcount=1

**`Block::size(&self) -> usize`**

- Returns block data size

**`Block::path(&self) -> &[u8]`**

- Returns internal path bytes

**`Block::disk_path(&self, root: PathBuf) -> PathBuf`**

- Constructs filesystem path from path bytes
- Creates hierarchical directory structure

**`Block::rc(&self) -> usize`**

- Returns current refcount

**`Block::increment_refcount(&mut self)`**

- Increments refcount by 1

**`Block::decrement_refcount(&mut self)`**

- Decrements refcount by 1

**`Block::to_vec(&self) -> Vec<u8>`**

- Serializes block to bytes

---

### metastore/object.rs - Object

**`Object::new(size: u64, hash: BlockID, object_data: ObjectData) -> Self`**

- Creates object with current timestamp
- Infers object_type from ObjectData variant

**`Object::minimum_inline_metadata_size() -> usize`**

- Returns minimum metadata size for inline objects

**`Object::to_vec(&self) -> Vec<u8>`**

- Serializes object to bytes

**`Object::format_e_tag(&self) -> String`**

- Formats S3 ETag (includes part count for multipart)

**`Object::hash(&self) -> &BlockID`**

- Returns object's content hash

**`Object::touch(&mut self)`**

- Updates creation time to now

**`Object::size(&self) -> u64`**

- Returns object size

**`Object::blocks(&self) -> &[BlockID]`**

- Returns slice of block IDs (empty for inline)

**`Object::has_block(&self, block: &BlockID) -> bool`**

- Checks if object contains specific block

**`Object::last_modified(&self) -> SystemTime`**

- Returns creation time as SystemTime

**`Object::is_inlined(&self) -> bool`**

- Checks if object is inline

**`Object::inlined(&self) -> Option<&Vec<u8>>`**

- Returns inline data if present

---

### s3fs.rs - S3FS

**`S3FS::new(casfs: CasFS, metrics: SharedMetrics) -> Self`**

- Wraps CasFS for S3 API

**`S3FS::calculate_multipart_hash(&self, blocks: &[BlockID]) -> Result<([u8; 16], usize), io::Result>`**

- Calculates S3 multipart ETag (MD5 of MD5s)
- Returns (hash, total_size)

**S3 Trait Methods (all async, return S3Result):**

**`complete_multipart_upload(&self, req: S3Request<CompleteMultipartUploadInput>)`**

- Collects all parts, validates part numbers sequential
- Creates final object with MultiPart data
- Removes part metadata
- Returns ETag

**`copy_object(&self, req: S3Request<CopyObjectInput>)`**

- Not implemented (returns NotImplemented error)

**`create_multipart_upload(&self, req: S3Request<CreateMultipartUploadInput>)`**

- Generates UUID upload_id
- No actual storage (metadata only on complete)

**`create_bucket(&self, req: S3Request<CreateBucketInput>)`**

- Checks bucket doesn't exist
- Creates bucket
- Increments bucket count metric

**`delete_bucket(&self, req: S3Request<DeleteBucketInput>)`**

- Deletes bucket and all objects
- Decrements bucket count

**`delete_object(&self, req: S3Request<DeleteObjectInput>)`**

- Checks key exists
- Calls casfs.delete_object
- Handles block deletion

**`delete_objects(&self, req: S3Request<DeleteObjectsInput>)`**

- Batch delete, continues on errors
- Returns list of deleted and errors

**`get_bucket_location(&self, req: S3Request<GetBucketLocationInput>)`**

- Checks bucket exists
- Returns empty location

**`get_object(&self, req: S3Request<GetObjectInput>)`**

- Checks bucket exists
- Gets object paths
- Returns inlined data or BlockStream
- Supports range requests

**`head_bucket(&self, req: S3Request<HeadBucketInput>)`**

- Checks bucket exists

**`head_object(&self, req: S3Request<HeadObjectInput>)`**

- Checks bucket exists
- Returns object size and metadata

**`list_buckets(&self, req: S3Request<ListBucketsInput>)`**

- Returns all buckets with creation dates

**`list_objects(&self, req: S3Request<ListObjectsInput>)`**

- Lists objects with prefix/marker/max_keys
- Returns pagination marker

**`list_objects_v2(&self, req: S3Request<ListObjectsV2Input>)`**

- Lists objects with prefix/start_after/continuation_token
- Encodes continuation token as hex

**`put_object(&self, req: S3Request<PutObjectInput>)`**

- Validates storage class
- Checks bucket exists
- If content_length <= max_inlined: stores inline
- Otherwise: calls store_single_object_and_meta
- Returns ETag

**`upload_part(&self, req: S3Request<UploadPartInput>)`**

- Validates content_length present
- Stores object (not metadata)
- Inserts multipart part metadata
- Returns ETag

---

### cas/block_stream.rs - BlockStream

**`BlockStream::new(paths: Vec<(PathBuf, usize)>, size: usize, range: RangeRequest, metrics: SharedMetrics) -> Self`**

- Creates stream over multiple block files
- Handles range requests and seeks

**`BlockStream::poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<io::Result<Bytes>>>`**

- Implements Stream trait
- Manages file opening/seeking/reading
- Applies range filtering
- Reports metrics

---

### cas/range_request.rs - RangeRequest

**`RangeRequest` enum variants:**

- `All` - entire file
- `Range(u64, u64)` - inclusive range
- `ToBytes(u64)` - from start to position
- `FromBytes(u64)` - from position to end

**`parse_range_request(input: &Option<String>) -> RangeRequest`**

- Parses S3 Range header (e.g., "bytes=0-1023")
- Returns RangeRequest enum

**`RangeRequest::size(&self, file_size: u64) -> u64`**

- Calculates byte count for range

---

### stores/fjall.rs - FjallStore (with transactions)

**`FjallStore::new(path: PathBuf, inlined_metadata_size: Option<usize>, durability: Option<Durability>) -> Self`**

- Opens transactional fjall keyspace
- Configures durability level

**`FjallStore::get_partition(&self, name: &str) -> Result<fjall::TxPartitionHandle, MetaError>`**

- Opens or creates partition

**`FjallStore::commit_persist(&self, tx: fjall::WriteTransaction) -> Result<(), MetaError>`**

- Commits transaction and persists

**Store trait implementation**

---

### stores/fjall_notx.rs - FjallStoreNotx (non-transactional)

**`FjallStoreNotx::new(path: PathBuf, inlined_metadata_size: Option<usize>) -> Self`**

- Opens non-transactional fjall keyspace

**`FjallNoTransaction::rollback(&mut self)`**

- Removes all inserted keys (simulates rollback)

**Store trait implementation**

---

### metrics.rs - Metrics & MetricFs

**`SharedMetrics::new() -> Self`**

- Creates shared metrics wrapper

**`Metrics::new() -> Self`**

- Initializes prometheus metrics:
  - s3_api_method_invocations
  - s3_bucket_count
  - s3_data_bytes_received/sent/written
  - s3_data_blocks_written/ignored/pending_write/write_errors/dropped

**Metrics recording methods:**

- `add_method_call(call_name: &str)`
- `set/inc/dec_bucket_count()`
- `bytes_received/sent(amount: usize)`
- `block_pending/written/write_error/ignored()`
- `blocks_dropped(amount: u64)`

**`MetricFs<T>` struct:**

- Wraps S3 implementation
- Records metrics for each S3 operation
- Implements S3 trait, forwarding to inner storage

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

### ObjectData Enum
```rust
Inline { data: Vec<u8> }           // Small object data
SinglePart { blocks: Vec<BlockID> } // Regular upload
MultiPart { blocks: Vec<BlockID>, parts: usize } // Multipart upload
````

### ObjectType Enum

```rust
Single = 0,
Multipart = 1,
Inline = 2
```

### RangeRequest Enum

```rust
All,
Range(u64, u64),
ToBytes(u64),
FromBytes(u64)
```

### BlockID

```rust
[u8; 16]  // MD5 hash
```

---

## Key Constants

- `BLOCK_SIZE = 1 << 20` = 1 MiB (chunk size for streaming)
- `BLOCKID_SIZE = 16` (MD5 hash size)
- `PTR_SIZE = usize` size (typically 8 bytes)
- `DEFAULT_BUCKET_TREE = "_BUCKETS"`
- `DEFAULT_BLOCK_TREE = "_BLOCKS"`
- `DEFAULT_PATH_TREE = "_PATHS"`
- `DEFAULT_INLINED_METADATA_SIZE = 1` (effectively disabled)

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
