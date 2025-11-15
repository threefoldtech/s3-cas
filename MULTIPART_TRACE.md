# Multipart Upload Flow in Multi-User Mode

## Overview
Multipart uploads use a **shared** `_MULTIPART_PARTS` tree stored at `/meta_root/blocks/db/_MULTIPART_PARTS` that is accessed by all users.

---

## 1. INITIALIZATION (Server Startup)

### Single-User Mode
**File**: `src/main.rs:188-202` (`run_single_user()`)
```rust
let casfs = CasFS::new(
    args.fs_root.clone(),
    args.meta_root.clone(),
    metrics.clone(),
    storage_engine,
    args.inline_metadata_size,
    Some(args.durability),
);
```

**File**: `src/cas/fs.rs:126-163` (`CasFS::new()`)
```rust
// Line 151-152: Creates user-specific multipart tree
let tree = meta_store.get_tree("_MULTIPART_PARTS").unwrap();
let multipart_tree = MultiPartTree::new(tree);
// Line 159: Stores as Arc
multipart_tree: Arc::new(multipart_tree),
```

### Multi-User Mode
**File**: `src/main.rs:253-275` (`run_multi_user()`)
```rust
// Lines 269-274: Create SharedBlockStore with shared multipart tree
let shared_block_store = SharedBlockStore::new(
    args.meta_root.join("blocks"),  // Path: /meta_root/blocks/db/
    storage_engine,
    args.inline_metadata_size,
    Some(args.durability),
)?;
```

**File**: `src/cas/shared_block_store.rs:28-59` (`SharedBlockStore::new()`)
```rust
// Line 48-51: Initialize shared multipart tree
let block_tree = meta_store.get_block_tree()?;
let path_tree = meta_store.get_path_tree()?;
let multipart_tree_base = meta_store.get_tree("_MULTIPART_PARTS")?;
let multipart_tree = MultiPartTree::new(multipart_tree_base);

// Line 53-58: Store as Arc in SharedBlockStore
Ok(Self {
    meta_store: Arc::new(meta_store),
    block_tree: Arc::new(block_tree),
    path_tree,
    multipart_tree: Arc::new(multipart_tree),
})
```

**File**: `src/main.rs:301-311` (Create CasFS for first user)
```rust
let s3_casfs = CasFS::new_multi_user(
    args.fs_root.clone(),
    args.meta_root.join(format!("user_{}", first_user_id)),
    shared_block_store.block_tree(),
    shared_block_store.path_tree(),
    shared_block_store.multipart_tree(),  // Line 306: Pass shared tree
    metrics.clone(),
    storage_engine,
    args.inline_metadata_size,
    Some(args.durability),
);
```

**File**: `src/cas/fs.rs:177-211` (`CasFS::new_multi_user()`)
```rust
pub fn new_multi_user(
    mut root: PathBuf,
    mut user_meta_path: PathBuf,
    shared_block_tree: Arc<BlockTree>,
    shared_path_tree: Arc<dyn BaseMetaTree>,
    shared_multipart_tree: Arc<MultiPartTree>,  // Line 182: Accept shared tree
    metrics: SharedMetrics,
    storage_engine: StorageEngine,
    inlined_metadata_size: Option<usize>,
    durability: Option<Durability>,
) -> Self {
    // ... (lines 188-200: initialize user metadata store)

    // Line 202-210: Use shared multipart tree
    Self {
        async_fs: Box::new(RealAsyncFs),
        user_meta_store,
        root,
        metrics,
        multipart_tree: shared_multipart_tree,  // Line 207: NO per-user tree
        block_tree: shared_block_tree,
        shared_path_tree: Some(shared_path_tree),
    }
}
```

---

## 2. STEP 1: CREATE MULTIPART UPLOAD

### S3 Client Request
```bash
POST /bucket/my-large-file?uploads HTTP/1.1
```

### Flow
**File**: `src/s3fs.rs:203-223` (`create_multipart_upload()`)
```rust
async fn create_multipart_upload(
    &self,
    req: S3Request<CreateMultipartUploadInput>,
) -> S3Result<S3Response<CreateMultipartUploadOutput>> {
    let CreateMultipartUploadInput { bucket, key, .. } = req.input;

    // Line 209-211: Check bucket exists
    if !try_!(self.casfs.bucket_exists(&bucket)) {
        return Err(s3_error!(NoSuchBucket, "Bucket does not exist"));
    }

    // Line 213: Generate unique upload ID
    let upload_id = Uuid::new_v4().to_string();

    // Line 215-220: Return upload_id to client
    let output = CreateMultipartUploadOutput {
        bucket: Some(bucket),
        key: Some(key),
        upload_id: Some(upload_id.to_string()),
        ..Default::default()
    };

    Ok(S3Response::new(output))
}
```

**Note**: No metadata is stored yet, just returns a UUID to the client.

---

## 3. STEP 2: UPLOAD PARTS (Called once per part)

### S3 Client Request
```bash
PUT /bucket/my-large-file?partNumber=1&uploadId=<uuid> HTTP/1.1
Content-Length: 5242880
[5MB binary data]
```

### Flow
**File**: `src/s3fs.rs:662-721` (`upload_part()`)
```rust
async fn upload_part(
    &self,
    req: S3Request<UploadPartInput>,
) -> S3Result<S3Response<UploadPartOutput>> {
    // Lines 666-675: Extract parameters
    let UploadPartInput {
        body,
        bucket,
        content_length,
        content_md5: _,
        key,
        part_number,
        upload_id,
        ..
    } = req.input;

    // Lines 677-686: Validate request
    let Some(body) = body else {
        return Err(s3_error!(IncompleteBody));
    };
    let content_length = content_length.ok_or_else(|| {
        s3_error!(MissingContentLength, "...")
    })?;

    // Lines 688-689: Convert to ByteStream
    let converted_stream = convert_stream_error(body);
    let byte_stream = ByteStream::new_with_size(converted_stream, content_length as usize);

    // Line 695: Store blocks to disk (chunks data, writes to /fs_root/blocks/)
    let (blocks, hash, size) = try_!(self.casfs.store_object(&bucket, &key, byte_stream).await);

    // Lines 697-702: Validate size
    if size != content_length as u64 {
        return Err(s3_error!(InvalidRequest, "..."));
    }

    // Lines 704-712: *** STORE MULTIPART METADATA ***
    try_!(self.casfs.insert_multipart_part(
        bucket,
        key,
        size as usize,
        part_number as i64,
        upload_id,
        hash,
        blocks  // List of block IDs that make up this part
    ));

    // Lines 714-720: Return ETag to client
    let e_tag = format!("\"{}\"", hex_string(&hash));
    let output = UploadPartOutput {
        e_tag: Some(e_tag),
        ..Default::default()
    };
    Ok(S3Response::new(output))
}
```

**File**: `src/cas/fs.rs:332-350` (`insert_multipart_part()`)
```rust
pub fn insert_multipart_part(
    &self,
    bucket: String,
    key: String,
    size: usize,
    part_number: i64,
    upload_id: String,
    hash: BlockID,
    blocks: Vec<BlockID>,
) -> Result<(), MetaError> {
    // Line 342: Get shared multipart tree
    let mp_map = self.multipart_tree.clone();  // Arc<MultiPartTree>

    // Line 344: Generate storage key
    let storage_key = self.part_key(&bucket, &key, &upload_id, part_number);
    // Format: "{bucket}-{key}-{upload_id}-{part_number}"

    // Line 346: Create MultiPart struct
    let mp = MultiPart::new(size, part_number, bucket, key, upload_id, hash, blocks);

    // Line 348: *** INSERT INTO SHARED TREE ***
    mp_map.insert(storage_key.as_bytes(), mp)?;
    Ok(())
}
```

**File**: `src/cas/fs.rs:327-329` (`part_key()`)
```rust
fn part_key(&self, bucket: &str, key: &str, upload_id: &str, part_number: i64) -> String {
    format!("{bucket}-{key}-{upload_id}-{part_number}")
}
```

**File**: `src/cas/multipart.rs:179-186` (`MultiPartTree::insert()`)
```rust
pub fn insert(&self, key: &[u8], mp: MultiPart) -> Result<(), MetaError> {
    self.tree.insert(key, mp.to_vec())
}
```

**Storage Location**:
- Metadata: `/meta_root/blocks/db/_MULTIPART_PARTS`
- Key: `"mybucket-myfile.bin-<uuid>-1"`
- Value: Serialized `MultiPart` struct containing:
  - size, part_number, bucket, key, upload_id
  - hash (MD5 of this part)
  - blocks (Vec of BlockIDs)

---

## 4. STEP 3: COMPLETE MULTIPART UPLOAD

### S3 Client Request
```bash
POST /bucket/my-large-file?uploadId=<uuid> HTTP/1.1
<CompleteMultipartUpload>
  <Part><PartNumber>1</PartNumber><ETag>"..."</ETag></Part>
  <Part><PartNumber>2</PartNumber><ETag>"..."</ETag></Part>
</CompleteMultipartUpload>
```

### Flow
**File**: `src/s3fs.rs:79-168` (`complete_multipart_upload()`)
```rust
async fn complete_multipart_upload(
    &self,
    req: S3Request<CompleteMultipartUploadInput>,
) -> S3Result<S3Response<CompleteMultipartUploadOutput>> {
    // Lines 83-96: Extract parameters
    let CompleteMultipartUploadInput {
        multipart_upload,
        bucket,
        key,
        upload_id,
        ..
    } = req.input;

    let multipart_upload = if let Some(multipart_upload) = multipart_upload {
        multipart_upload
    } else {
        return Err(s3_error!(InvalidPart, "Missing multipart_upload"));
    };

    // Lines 98-134: *** COLLECT ALL PARTS FROM SHARED TREE ***
    let mut blocks = vec![];
    let mut cnt: i32 = 0;
    for part in multipart_upload.parts.iter().flatten() {
        // Lines 101-111: Validate part numbers are sequential
        let part_number = try_!(part.part_number.ok_or_else(|| { ... }));
        cnt = cnt.wrapping_add(1);
        if part_number != cnt {
            try_!(Err(io::Error::new(io::ErrorKind::Other, "InvalidPartOrder")));
        }

        // Lines 113-115: *** RETRIEVE PART FROM SHARED TREE ***
        let result = self.casfs.get_multipart_part(
            &bucket,
            &key,
            &upload_id,
            part_number as i64
        );

        // Lines 116-132: Handle errors
        let mp = match result {
            Ok(Some(mp)) => mp,
            Ok(None) => {
                error!("Missing part \"{}\" in multipart upload: part not found", part_number);
                return Err(s3_error!(InvalidArgument, "Part not uploaded"));
            }
            Err(e) => {
                error!("Missing part \"{}\" in multipart upload: {}", part_number, e);
                return Err(s3_error!(InvalidArgument, "Part not uploaded"));
            }
        };

        // Line 133: Collect all blocks from all parts
        blocks.extend_from_slice(mp.blocks());
    }

    // Line 136: Calculate multipart hash (MD5 of MD5s)
    let (content_hash, size) = try_!(self.calculate_multipart_hash(&blocks));

    // Lines 138-147: *** CREATE FINAL OBJECT METADATA ***
    let object_meta = try_!(self.casfs.create_object_meta(
        &bucket,
        &key,
        size as u64,
        content_hash,
        ObjectData::MultiPart {
            blocks,
            parts: cnt as usize
        },
    ));

    // Lines 149-159: *** CLEANUP: REMOVE PARTS FROM SHARED TREE ***
    for part in multipart_upload.parts.into_iter().flatten() {
        if let Err(e) = self.casfs.remove_multipart_part(
            &bucket,
            &key,
            &upload_id,
            part.part_number.unwrap() as i64,
        ) {
            error!("Could not remove part: {}", e);
        };
    }

    // Lines 161-167: Return response
    let output = CompleteMultipartUploadOutput {
        bucket: Some(bucket),
        key: Some(key),
        e_tag: Some(object_meta.format_e_tag()),
        ..Default::default()
    };
    Ok(S3Response::new(output))
}
```

**File**: `src/cas/fs.rs:352-362` (`get_multipart_part()`)
```rust
pub fn get_multipart_part(
    &self,
    bucket: &str,
    key: &str,
    upload_id: &str,
    part_number: i64,
) -> Result<Option<MultiPart>, MetaError> {
    // Line 359: Get shared multipart tree
    let mp_map = self.multipart_tree.clone();  // Arc<MultiPartTree>

    // Line 360-361: *** RETRIEVE FROM SHARED TREE ***
    let part_key = self.part_key(bucket, key, upload_id, part_number);
    mp_map.get_multipart_part(part_key.as_bytes())
}
```

**File**: `src/cas/multipart.rs:192-200` (`MultiPartTree::get_multipart_part()`)
```rust
pub fn get_multipart_part(&self, key: &[u8]) -> Result<Option<MultiPart>, MetaError> {
    let value = match self.tree.get(key) {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(None),
        Err(e) => return Err(e),
    };
    let mp = MultiPart::try_from(value.as_ref()).expect("Corrupted multipart data");
    Ok(Some(mp))
}
```

**File**: `src/cas/fs.rs:364-374` (`remove_multipart_part()`)
```rust
pub fn remove_multipart_part(
    &self,
    bucket: &str,
    key: &str,
    upload_id: &str,
    part_number: i64,
) -> Result<(), MetaError> {
    // Line 371: Get shared multipart tree
    let mp_map = self.multipart_tree.clone();

    // Line 372-373: *** REMOVE FROM SHARED TREE ***
    let part_key = self.part_key(bucket, key, upload_id, part_number);
    mp_map.remove(part_key.as_bytes())
}
```

**File**: `src/cas/multipart.rs:188-190` (`MultiPartTree::remove()`)
```rust
pub fn remove(&self, key: &[u8]) -> Result<(), MetaError> {
    self.tree.remove(key)
}
```

---

## 5. KEY DIFFERENCES: Single vs Multi-User Mode

### Single-User Mode
- **Multipart Tree Location**: `/meta_root/db/_MULTIPART_PARTS`
- **Initialization**: Created per CasFS instance (src/cas/fs.rs:151-152)
- **Isolation**: Each CasFS has its own multipart tree (but there's only one CasFS)

### Multi-User Mode (CURRENT IMPLEMENTATION)
- **Multipart Tree Location**: `/meta_root/blocks/db/_MULTIPART_PARTS` (SHARED)
- **Initialization**: Created once in SharedBlockStore (src/cas/shared_block_store.rs:50-51)
- **Sharing**: All users access the same `Arc<MultiPartTree>`
  - User A's upload_part → writes to shared tree
  - User B's upload_part → writes to same shared tree
  - No conflicts because keys include bucket+key+upload_id (which is UUID)
- **Why This Works**:
  - Each upload has a unique UUID (upload_id)
  - Part keys are formatted as: `"{bucket}-{key}-{upload_id}-{part_number}"`
  - Two users can't have the same upload_id (UUID collision virtually impossible)
  - Multipart metadata is temporary (deleted after completion)

---

## 6. COMPLETE DATA FLOW

### User A uploads 10MB file (2 parts):

1. **CREATE**:
   - Client → S3FS → generates UUID "abc-123"
   - Returns upload_id to client

2. **UPLOAD PART 1**:
   - Client sends 5MB → S3FS:662
   - store_object() → writes blocks to `/fs_root/blocks/...`
   - insert_multipart_part() → writes to shared tree:
     - Key: `"mybucket-file.bin-abc-123-1"`
     - Value: `{size: 5MB, blocks: [block1, block2, ...], ...}`

3. **UPLOAD PART 2**:
   - Client sends 5MB → S3FS:662
   - store_object() → writes blocks to `/fs_root/blocks/...`
   - insert_multipart_part() → writes to shared tree:
     - Key: `"mybucket-file.bin-abc-123-2"`
     - Value: `{size: 5MB, blocks: [block3, block4, ...], ...}`

4. **COMPLETE**:
   - Client → S3FS:79
   - get_multipart_part("mybucket-file.bin-abc-123-1") → reads from shared tree
   - get_multipart_part("mybucket-file.bin-abc-123-2") → reads from shared tree
   - Collects all blocks: [block1, block2, block3, block4, ...]
   - create_object_meta() → writes to user's metadata:
     - Location: `/meta_root/user_A/db/mybucket/file.bin`
     - Value: `Object{blocks: [block1-4], type: MultiPart, ...}`
   - remove_multipart_part() → deletes both keys from shared tree

### User B uploads simultaneously:
- Uses different upload_id "def-456"
- Part keys: `"mybucket-file2.bin-def-456-1"`, etc.
- **NO CONFLICT** with User A's parts

---

## 7. STORAGE LOCATIONS

### Multi-User Mode Storage Layout:
```
/meta_root/
  ├── blocks/db/                    # SHARED METADATA
  │   ├── _BLOCKS/                  # Block refcounts (shared)
  │   ├── _PATHS/                   # Block path allocation (shared)
  │   └── _MULTIPART_PARTS/         # Multipart temp metadata (shared) ← HERE
  │       ├── "mybucket-file.bin-abc-123-1" → MultiPart{...}
  │       └── "mybucket-file.bin-abc-123-2" → MultiPart{...}
  │
  ├── user_delandtj/db/             # USER-SPECIFIC METADATA
  │   ├── _BUCKETS/                 # User's buckets
  │   └── mybucket/                 # User's objects
  │       └── "file.bin" → Object{blocks: [...], type: MultiPart}
  │
  └── user_smetlee/db/              # USER-SPECIFIC METADATA
      └── ...

/fs_root/
  └── blocks/                       # SHARED BLOCK STORAGE
      └── XX/YY/ZZZZ...             # Actual block files (deduplicated)
```

### Why Shared Multipart Tree is Correct:
1. **Temporary data**: Parts are deleted after completion
2. **Globally unique keys**: UUID ensures no collisions
3. **Simpler implementation**: No need for per-request routing
4. **Consistent with design**: Matches shared blocks and paths
5. **Performance**: Single tree lookup instead of per-user routing

---

## 8. CRITICAL FILES

| File | Lines | Purpose |
|------|-------|---------|
| `src/main.rs` | 269-274 | Create SharedBlockStore with shared multipart tree |
| `src/main.rs` | 306 | Pass shared_multipart_tree to CasFS |
| `src/cas/shared_block_store.rs` | 50-51 | Initialize shared multipart tree |
| `src/cas/shared_block_store.rs` | 72-74 | Getter for shared multipart tree |
| `src/cas/fs.rs` | 177-211 | CasFS::new_multi_user() accepts shared tree |
| `src/cas/fs.rs` | 207 | Store shared multipart_tree (not per-user) |
| `src/cas/fs.rs` | 327-329 | Generate part key format |
| `src/cas/fs.rs` | 332-350 | insert_multipart_part() |
| `src/cas/fs.rs` | 352-362 | get_multipart_part() |
| `src/cas/fs.rs` | 364-374 | remove_multipart_part() |
| `src/s3fs.rs` | 203-223 | create_multipart_upload() |
| `src/s3fs.rs` | 662-721 | upload_part() |
| `src/s3fs.rs` | 79-168 | complete_multipart_upload() |
| `src/cas/multipart.rs` | 179-186 | MultiPartTree::insert() |
| `src/cas/multipart.rs` | 188-190 | MultiPartTree::remove() |
| `src/cas/multipart.rs` | 192-200 | MultiPartTree::get_multipart_part() |

---

## 9. TESTING MULTIPART IN MULTI-USER MODE

### Test commands (using aws-cli):

```bash
# Start server in multi-user mode
./target/release/s3-cas server \
  --users-config users.toml \
  --fs-root ./data \
  --meta-root ./metadata

# Configure two users
aws configure --profile delandtj
# Access Key: AKIAIOSFODNN7EXAMPLE1
# Secret Key: wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY1

aws configure --profile smetlee
# Access Key: AKIAIOSFODNN7EXAMPLE2
# Secret Key: wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY2

# User A: Create bucket and upload multipart
aws --profile delandtj --endpoint-url http://localhost:8014 \
  s3 mb s3://testbucket

# Create 10MB test file
dd if=/dev/urandom of=/tmp/bigfile bs=1M count=10

# User A: Multipart upload
aws --profile delandtj --endpoint-url http://localhost:8014 \
  s3 cp /tmp/bigfile s3://testbucket/bigfile

# User B: Same operation simultaneously
aws --profile smetlee --endpoint-url http://localhost:8014 \
  s3 mb s3://testbucket2

aws --profile smetlee --endpoint-url http://localhost:8014 \
  s3 cp /tmp/bigfile s3://testbucket2/bigfile

# Verify no conflicts in shared multipart tree
# Check metadata locations
ls -lah ./metadata/blocks/db/        # Shared (blocks, paths, multipart)
ls -lah ./metadata/user_delandtj/db/ # User A's objects
ls -lah ./metadata/user_smetlee/db/  # User B's objects
```

### Expected behavior:
- ✅ Both uploads succeed without conflicts
- ✅ Multipart metadata temporarily in `/metadata/blocks/db/_MULTIPART_PARTS/`
- ✅ Final objects in user-specific trees
- ✅ Blocks deduplicated if same content
- ✅ No leftover multipart metadata after completion
