# PRD-001: cas-storage Internal Restructure

Status:     Draft
Author:     Jan De Landtsheer
Date:       2026-04-24
Related:    docs/prd/prd000-current-state-and-restructure.md (anchor)
            docs/arch/deadlock-fix.md (the invariants this PRD protects)
            docs/arch/refcount.md    (the invariants this PRD protects)
            docs/adr/003-cas-storage-library-extraction.md

## 1. Purpose

Make the `cas-storage` crate small enough to read in one sitting. Today
`cas-storage/src/cas/fs.rs` is 1198 lines and does five jobs in one
file: `CasFS` construction (with two incompatible constructors),
write path, read path, delete path, and bucket / multipart CRUD. The
load-bearing function `store_object` is ~180 lines embedded in the
middle of that file, which is precisely the function that needs to be
reasoned about whenever the deadlock fix or the refcount invariant are
touched.

This PRD specifies the target shape of the crate's internals. It is
subtractive and mechanical: no new features, no new behaviour, no
change to the public API the `s3-cas` binary depends on.

## 2. Scope

In scope:

- Reshape `cas-storage/src/cas/*` into a set of small modules each with
  a single, namable job.
- Collapse the "two-shape `CasFS`" (one constructor for single-user,
  one for multi-user, optional shared trees) into a single construction
  path. Single-user becomes the degenerate case of multi-user with one
  namespace.
- Replace `PendingMarker` with an RAII guard that encodes the metric
  state machine in its drop behaviour.
- Decide the fate of the `AsyncFileSystem` trait: either keep it as a
  real testing seam with a real mock, or inline the `std::fs` calls.

Out of scope (each has its own PRD or will get one):

- Changing any refcount logic, transaction boundary, chunk size, or
  hash algorithm.
- Removing `rusoto_core::ByteStream` from the public API (the
  ByteStream-removal PRD; not yet written).
- Switching disk writes to `tokio::task::spawn_blocking` (noted as a
  followup in the deadlock postmortem).
- Reintroducing intra-object concurrency in `store_object`.
- Persistent sessions, quotas, per-user metrics, new protocol adapters.

## 3. Invariants (must not be weakened)

Restated from PRD-000 section 6 and the deadlock postmortem. Any
component in this PRD must preserve these by structure, not by comment:

1. `Transaction::write_block` never silently skips a refcount increment
   on a duplicate block from a different key. Data leakage on failure
   is acceptable; data loss is not.
2. No `.await` inside a metadata transaction. The Fjall write permit is
   held for microseconds only.
3. Shared block CAS, per-user metadata.
4. MD5 block id, 1 MiB chunk size, `inlined_metadata_size` remains a
   runtime knob.
5. For new blocks: metadata commits *before* the on-disk write; a
   compensating delete runs if the on-disk write fails.

## 4. Target Module Layout

```
cas-storage/src/
    lib.rs                           unchanged public re-exports
    metastore/                       unchanged
    metrics.rs                       unchanged
    cas/
        mod.rs                       module declarations, CasFS re-export
        fs.rs                        CasFS struct, construction, public API
                                     (thin delegation, ~200 lines)
        write_path.rs                store_object and its per-block pipeline
                                     (the deadlock-safe core, ~300 lines)
        read_path.rs                 get_object_paths, block-path resolution
                                     (~100 lines)
        delete_path.rs               delete_object, bucket_delete, and the
                                     refcount-decrement path (~100 lines)
        buckets.rs                   create_bucket, list_buckets,
                                     bucket_exists, key_exists (~80 lines)
        multipart.rs                 unchanged (MultiPart, MultiPartTree,
                                     insert/get/remove already here)
        shared_block_store.rs        unchanged
        block_stream.rs              unchanged
        buffered_byte_stream.rs      unchanged
        range_request.rs             unchanged
        write_guard.rs               BlockWriteGuard RAII (see sect 6)
        async_fs.rs                  AsyncFileSystem trait + RealAsyncFs
                                     (+ test mock, see sect 7)
```

Each of `fs.rs`, `write_path.rs`, `read_path.rs`, `delete_path.rs`,
`buckets.rs` has one reason to change and fits comfortably in a screen
or two.

## 5. `CasFS` One-Shape

### 5.1 Current shape (to be removed)

```rust
pub struct CasFS {
    async_fs: Box<dyn AsyncFileSystem>,
    user_meta_store: MetaStore,
    root: PathBuf,
    metrics: SharedMetrics,
    multipart_tree: Arc<MultiPartTree>,
    block_tree: Arc<BlockTree>,
    shared_path_tree: Option<Arc<dyn BaseMetaTree>>,   // None = single-user
    shared_meta_store: Option<Arc<MetaStore>>,          // None = single-user
}

impl CasFS {
    pub fn new(...) -> Self;                // single-user, options = None
    pub fn new_multi_user(...) -> Self;     // options = Some
    fn path_tree(&self) -> ... {
        match &self.shared_path_tree {
            Some(t) => Ok(Arc::clone(t)),
            None    => self.user_meta_store.get_path_tree(),
        }
    }
}
```

Every method that touches shared state has to decide which store to
use. That decision is duplicated across `path_tree`, `delete_object`,
`store_object`, and any future method.

### 5.2 Target shape

```rust
pub struct CasFS {
    async_fs: Box<dyn AsyncFileSystem>,
    namespace: MetaStore,                 // per-namespace (was user_meta_store)
    shared: Arc<SharedBlockStore>,        // required, not optional
    root: PathBuf,
    metrics: SharedMetrics,
}

impl CasFS {
    pub fn new(
        root: PathBuf,
        namespace_meta_path: PathBuf,
        shared: Arc<SharedBlockStore>,
        metrics: SharedMetrics,
        storage_engine: StorageEngine,
        inlined_metadata_size: Option<usize>,
        durability: Option<Durability>,
    ) -> Self;
}
```

One constructor. One code path. `SharedBlockStore` is always present;
the single-user case (the `s3-cas` binary does not use it anymore,
but the library must still support it for downstream consumers) is
constructed by passing a `SharedBlockStore` that lives in the same
directory tree as the one namespace. The `CasFS` does not know whether
its `shared` is dedicated or truly shared -- it simply uses it.

Accessors that used to branch on `shared_*.is_some()`:

```rust
fn block_tree(&self)   -> Arc<BlockTree>            { self.shared.block_tree() }
fn path_tree(&self)    -> Arc<dyn BaseMetaTree>     { self.shared.path_tree() }
fn multipart_tree(&self)-> Arc<MultiPartTree>       { self.shared.multipart_tree() }
fn shared_store(&self) -> Arc<MetaStore>            { self.shared.meta_store() }
```

All four become one-liners.

### 5.3 Migration rule for s3-cas

The `s3-cas` binary constructs `CasFS` via `UserRouter::create_casfs_for_user`.
After this PRD lands, that function shrinks from a 10-argument call to
a 7-argument call using the new constructor. No behaviour changes.

For third-party consumers (hypothetical RESP/gRPC adapters, per ADR-003),
a helper `CasFS::single_namespace(root, meta_path, metrics, engine,
inline, durability)` builds a `SharedBlockStore` pointing at
`meta_path.join("blocks")` and returns a `CasFS` for namespace
`"default"`. This is a 20-line convenience shim, nothing more.

## 6. `BlockWriteGuard` (replaces `PendingMarker`)

### 6.1 Current shape

```rust
struct PendingMarker {
    metrics: SharedMetrics,
    in_flight: u64,
}
impl PendingMarker {
    fn block_pending(&mut self)      { self.metrics.block_pending();     self.in_flight += 1; }
    fn block_write_error(&mut self)  { self.metrics.block_write_error(); self.in_flight -= 1; }
    fn block_ignored(&mut self)      { self.metrics.block_ignored(); }
    fn block_written(&mut self, sz)  { self.metrics.block_written();     self.in_flight -= 1; }
}
impl Drop for PendingMarker {
    fn drop(&mut self) { self.metrics.blocks_dropped(self.in_flight); }
}
```

The caller has to remember to invoke the right method on every branch.
Miss a branch and the metrics state drifts. The compiler does not help.

### 6.2 Target shape

```rust
/// RAII guard for a single block write. Exactly one terminal method
/// must be called; `Drop` reports the block as dropped if none was.
#[must_use = "a BlockWriteGuard must be resolved with .written(), .ignored(), or .failed()"]
pub(crate) struct BlockWriteGuard<'a> {
    metrics: &'a SharedMetrics,
    state: GuardState,
}

enum GuardState { Pending, Resolved }

impl<'a> BlockWriteGuard<'a> {
    pub fn new_pending(metrics: &'a SharedMetrics) -> Self { ... }
    pub fn ignored(mut self)  { self.state = Resolved; self.metrics.block_ignored(); }
    pub fn written(mut self, size: usize) { self.state = Resolved; self.metrics.block_written(size); }
    pub fn failed(mut self)   { self.state = Resolved; self.metrics.block_write_error(); }
}

impl<'a> Drop for BlockWriteGuard<'a> {
    fn drop(&mut self) {
        if matches!(self.state, GuardState::Pending) {
            self.metrics.blocks_dropped(1);
        }
    }
}
```

Properties:

- Forgetting to resolve the guard produces `#[must_use]` warning and a
  `blocks_dropped` increment at `Drop`.
- The `ignored` case no longer needs a pending count; it is the case
  where we never transitioned to `Pending` in the first place, so call
  it directly on the metrics instead of via a guard.
- The `write_path.rs` call sites become: create guard (`new_pending`)
  once per new-block branch, resolve it with `.written()` or
  `.failed()` before return.

This is cosmetic to the user but removes a category of "someone added a
return path and forgot to tick the counter" bugs.

## 7. `AsyncFileSystem` decision

Two acceptable outcomes. Pick one in this PRD's review:

**Option A -- keep the seam, commit to it.**
Move the trait to `cas/async_fs.rs`, add an `InMemoryFs` mock in the
same file behind `#[cfg(any(test, feature = "test-utils"))]`, and have
the `store_object` tests in `write_path.rs` use it. This is what the
trait was introduced for and is the only reason it still exists.

**Option B -- delete the trait.**
Inline `std::fs::{create_dir_all, write}` in `write_path.rs`. The
existing tests that inject a failure through `AsyncFileSystem` get
rewritten to poison the filesystem (e.g. make the parent directory a
read-only path) so the failure path is still exercised.

Recommendation: Option A. The trait is the only seam that lets the
write-failure test run in-process without touching a real disk, and
the compensating delete path is the most important thing to keep under
test. Cost is ~30 lines of mock.

Non-goal: switching to async I/O via `tokio::task::spawn_blocking`.
That is tracked as a followup in `docs/arch/deadlock-fix.md` and is
orthogonal to this PRD.

## 8. `write_path.rs` contract

This is the module whose existence justifies the whole PRD. The target
file exposes one public entry point:

```rust
pub(crate) async fn store_object(
    ctx: &CasFS,
    bucket: &str,
    key: &str,
    data: ByteStream,
) -> io::Result<(Vec<BlockID>, BlockID, u64)>;
```

Internally, `store_object` is a pipeline of named stages. Each stage
is a free function with an explicit contract:

```rust
// 1. Chunk the incoming byte stream into fixed-size blocks with a
//    running MD5 of the full content.
fn chunk_stream(data: ByteStream) -> impl Stream<Item = io::Result<(usize, Bytes, BlockID, Md5Running)>>;

// 2. For a single chunk: run write_block inside a transaction, commit
//    immediately (before any I/O), return the block metadata and a
//    flag indicating whether the block needs to be written to disk.
fn commit_block_metadata(
    shared: &SharedBlockStore,
    path_tree: &dyn BaseMetaTree,
    block_hash: BlockID,
    data_len: usize,
    key_has_block: bool,
) -> Result<(NeedsDiskWrite, Block), MetaError>;

// 3. Write a committed block to disk. On failure, run the compensating
//    delete against the block tree.
fn write_block_to_disk(
    async_fs: &dyn AsyncFileSystem,
    root: &Path,
    block: &Block,
    bytes: &[u8],
    block_hash: BlockID,
    block_tree: &BlockTree,
) -> io::Result<()>;

// 4. The old-vs-new block reconciliation after all chunks finish.
fn handle_key_replacement(
    shared_store: &MetaStore,
    bucket: &str,
    key: &str,
    new_blocks: &[BlockID],
) -> Result<Vec<BlockID>, MetaError>;
```

Mechanical properties of the layout:

- `commit_block_metadata` is the *only* function that holds a Fjall
  write permit. It begins a transaction, calls `write_block`, and
  commits. No `.await`. No I/O.
- `write_block_to_disk` never holds a transaction. It is called only
  after `commit_block_metadata` has returned.
- The compensating delete is a static function with a signature
  that reads as "here is the block-tree, here is the hash I just
  inserted, undo it". A reviewer can see the invariant without having
  to trace through a closure.
- The sequential `for_each` (not `for_each_concurrent`) stays, and the
  reason stays documented both in `docs/arch/deadlock-fix.md` and in a
  single-line comment at the loop.

## 9. `read_path.rs` and `delete_path.rs` and `buckets.rs`

These are mechanical extractions. Each file contains free functions
that take `&CasFS` (or the pieces of it they need) and forward to the
metastore. The goal is to make `fs.rs` a list of 15-line methods that
each call one function in one of these files.

### read_path.rs

- `get_object_paths(ctx, bucket, key) -> Result<ObjectPaths, MetaError>`
  The current `get_object_paths` method, unchanged in behaviour.
- `get_object_meta(ctx, bucket, key) -> Result<Option<Object>, MetaError>`

### delete_path.rs

- `delete_object(ctx, bucket, key) -> impl Future<Output = Result<(), MetaError>>`
  The current `delete_object` method. Calls `MetaStore::delete_object`
  for the refcount decrement then `async_fs::remove_file` for the
  on-disk cleanup. No holding locks across I/O (same discipline as
  `write_path`).
- `bucket_delete(ctx, bucket) -> impl Future<...>`
  Cascades through objects, reusing `delete_object`.

### buckets.rs

- `create_bucket`, `list_buckets`, `bucket_exists`, `key_exists`.
  Thin wrappers over `MetaStore` methods; exist in `buckets.rs` so the
  reader looking at `fs.rs` can skip over them.

## 10. `cas/fs.rs` target

```rust
// ~200 lines including docs
pub struct CasFS { ... }  // see sect 5.2

impl CasFS {
    pub fn new(...) -> Self { ... }                        // single constructor
    pub fn single_namespace(...) -> Self { ... }           // convenience shim
    pub fn fs_root(&self) -> &PathBuf;
    pub fn max_inlined_data_length(&self) -> usize;

    // Buckets (thin delegation to buckets.rs)
    pub fn create_bucket(&self, name: &str) -> Result<(), MetaError>;
    pub fn list_buckets(&self) -> Result<Vec<BucketMeta>, MetaError>;
    pub fn bucket_exists(&self, name: &str) -> Result<bool, MetaError>;
    pub fn key_exists(&self, bucket: &str, key: &str) -> Result<bool, MetaError>;
    pub fn get_bucket(&self, name: &str) -> Result<Arc<dyn MetaTreeExt + Send + Sync>, MetaError>;
    pub async fn bucket_delete(&self, name: &str) -> Result<(), MetaError>;

    // Objects (thin delegation)
    pub fn create_object_meta(&self, b: &str, k: &str, size: u64, hash: BlockID, data: ObjectData) -> Result<Object, MetaError>;
    pub fn get_object_meta(&self, b: &str, k: &str) -> Result<Option<Object>, MetaError>;
    pub fn get_object_paths(&self, b: &str, k: &str) -> Result<ObjectPaths, MetaError>;
    pub async fn delete_object(&self, b: &str, k: &str) -> Result<(), MetaError>;

    // Write path (thin delegation)
    pub async fn store_object(&self, b: &str, k: &str, data: ByteStream) -> io::Result<(Vec<BlockID>, BlockID, u64)>;
    pub async fn store_single_object_and_meta(&self, b: &str, k: &str, data: ByteStream) -> io::Result<Object>;
    pub fn store_inlined_object(&self, b: &str, k: &str, data: Vec<u8>) -> Result<Object, MetaError>;

    // Multipart (thin delegation to multipart.rs)
    pub fn insert_multipart_part(...) -> Result<(), MetaError>;
    pub fn get_multipart_part(...) -> Result<Option<MultiPart>, MetaError>;
    pub fn remove_multipart_part(...) -> Result<(), MetaError>;
}
```

Every method in `impl CasFS` should be no more than 10-20 lines and
should call exactly one function in a sibling file. The file is the
table of contents for the crate.

## 11. Tests

- Unit tests in each sibling file cover that file's functions. In
  particular `write_path.rs` is the only place the compensating-delete
  behaviour is tested, using `InMemoryFs` (Option A above) to inject
  failures.
- `fs.rs` keeps an integration-style `#[cfg(test)]` block that
  exercises the public `CasFS` surface end-to-end with a `tempfile`
  directory. This is the one that must keep passing unchanged.
- The existing test `test_store_object_write_failure` must still pass
  against the new split; it is the canary for the refcount invariant
  under disk-write failure.

## 12. Success Criteria

- `cas-storage/src/cas/*.rs` file sizes: each under 400 lines.
- `cas-storage/src/cas/fs.rs`: under 250 lines.
- `store_object`'s callable body (including its named stages) fits in
  one file and can be read top-to-bottom without jumping.
- `cargo test --workspace` passes with the same test count as before.
- `cargo clippy --workspace --no-deps` produces no new warnings.
- No change to the public surface re-exported from
  `cas-storage/src/lib.rs`.
- `s3-cas` binary builds with a simple `CasFS::new(...)` call in
  `UserRouter::create_casfs_for_user`; `new_multi_user` is gone.

## 13. Non-goals (reprise)

- No performance work. If throughput changes measurably after the
  split, something structural changed by accident; revert.
- No change to the on-disk format. Existing databases keep working
  against the new binary.
- No change to the refcount invariant, chunk size, hash, or durability
  knobs.
- **Do not try to abstract the Fjall backend away.** The graph-coupling
  analysis shows `cas/fs.rs` and `metastore/stores/fjall.rs` at a
  change-coupling score of 1.00 -- every historical change to the
  write path also touched the Fjall backend. That coupling is
  load-bearing: the write-permit / partition-cache / transaction-shape
  contract only makes sense when write_path.rs and fjall.rs are co-
  designed. After the split, write_path.rs will still need to move in
  lockstep with fjall.rs whenever the tx shape changes; that is
  correct and not a smell. Adding a second storage backend (sled,
  redb, whatever) is a separate PRD if it ever happens.

## 14. Open Questions

- Should `SharedBlockStore` expose a `for_tests()` helper that points
  at a `tempdir()` and builds the `CasFS`? Or keep that wiring in
  `cas/fs.rs::tests`? Small, defer to implementation.
- Is there any caller outside the crate that uses `CasFS::new_multi_user`
  directly (i.e. without going through `UserRouter`)? If yes, keep it
  as a thin wrapper around `new` for a release, mark deprecated. The
  only in-tree call is `UserRouter::create_casfs_for_user`.
- `BlockWriteGuard` vs keeping `PendingMarker`: if the `#[must_use]`
  static guarantee turns out to fight the borrow checker (because the
  block-write closure currently owns `pm` across an `.await` boundary),
  fall back to keeping `PendingMarker` and move on. The split is the
  90% win; the guard is the 10%.
