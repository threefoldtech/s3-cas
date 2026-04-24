# The Fjall Deadlock -- Postmortem and Fix

Status:  historical-with-guardrails
Authors: Jan De Landtsheer (the commits); this writeup reconstructed from
         the tree.
Commits: c5f9cc9 "Fix critical fjall database deadlock and add HTTP browser UI"
         (2025-11-13)
         228cd24 "feat: Implement HTTP file download and fix locking race
         condition" (2025-11-23)  -- follow-up refactor + compensating tx

## TL;DR

Under concurrent S3 PUT traffic the write path could permanently hang with
every worker parked in `fjall`. The root cause was that the write-block
transaction held an internal Fjall lock across an `.await` that crossed
thread boundaries through tokio's work-stealing scheduler. Two fixes
landed, one week apart:

1. `c5f9cc9` stopped the scheduler from ever crossing threads while a
   Fjall lock was held: turned `AsyncFileSystem` into a blocking trait,
   collapsed `for_each_concurrent(5, ...)` to sequential `for_each(...)`,
   replaced `.send().await` with `unbounded_send`, and cached partition
   handles and the `BlockTree` so that tree opens stopped contending with
   in-flight transactions.
2. `228cd24` inverted the remaining lock window: the metadata transaction
   is now committed *before* the on-disk block write instead of after, so
   the Fjall lock is never held across slow I/O at all. To preserve the
   "data leakage OK, data loss never" invariant, a compensating delete
   runs when the subsequent I/O fails.

Both are in `cas-storage/src/cas/fs.rs` and
`cas-storage/src/metastore/stores/fjall.rs` today.

## Symptoms

- Server accepts PUT requests, starts streaming blocks, then stops making
  progress.
- `top` shows worker threads at 0% CPU.
- Ctrl-C eventually produces a stack where every tokio worker is parked
  inside `fjall::TxKeyspace::write_tx` or
  `fjall::Keyspace::open_partition`.
- Reproduces reliably with parallel multi-block PUTs. More likely with
  small blocks, many parallel uploads, or heterogenous block sizes
  (mix of new and duplicate blocks).
- Single-threaded tokio runtime does not reproduce the hang; only
  multi-threaded / default runtimes do.

## Background: the write path before the fix

The core loop in `CasFS::store_object` looked like this (simplified, as
it was just before `c5f9cc9`):

```rust
byte_stream
    .chunks(BLOCK_SIZE)
    .enumerate()
    .for_each_concurrent(5, |(idx, bytes)| async move {
        let mut tx = meta_store.begin_transaction();      // (A) take Fjall lock
        let (is_new, block) = tx.write_block(hash, size, key_has_block)?;

        if !is_new {
            tx.commit()?;                                  // (A1) release lock
            return Ok((idx, hash));
        }

        // block is new -- write it to disk, then commit
        let path = block.disk_path(root);
        self.async_fs.create_dir_all(path.parent()).await; // (B) await across threads
        self.async_fs.write(&path, &bytes).await;          // (C) await across threads
        tx.commit()?;                                      // (A2) release lock
        tx_chan.send(Ok((idx, hash))).await;               // (D) await across threads
    })
    .await;
```

Three properties matter:

- **The transaction holds a Fjall-internal lock** between `begin_transaction`
  and `commit`/`rollback`. Fjall's transactional partition uses a global
  write lock under the hood; multiple concurrent write transactions on
  the same keyspace serialise at this lock.
- **`for_each_concurrent(5, ...)`** lets up to five of these closures run
  at once. Each closure is an independent future that the tokio scheduler
  polls on whichever worker is available.
- **`async_fs::{create_dir_all, write}` and `mpsc::Sender::send` are
  `.await` points.** When a future suspends at an `.await`, tokio's
  work-stealing scheduler is free to resume it on a *different* worker
  thread when it wakes up.

## The deadlock

The actual deadlock is not a classical "two locks, two orders" cycle. It
is a single lock being reacquired on a different thread while the same
logical task still believes it holds it, combined with the fact that
Fjall's lock is not async-aware.

Three cooperating effects produced the hang:

### 1. Thread-hopping across an await while holding a Fjall internal

Consider two futures F1 and F2 running under `for_each_concurrent`:

```
worker thread T1              worker thread T2
---------------------         ---------------------
F1: begin_transaction
    (acquires write perm)
F1: write_block (is_new=true)
F1: create_dir_all(...).await  <-- suspend, F1 yields
                              F1 woken here; polled on T2
                              F1: write(...).await          <-- suspend
T1 now picks F2
F2: begin_transaction
    (blocks: write perm held
     by F1, which is parked
     on T2 in async_fs::write)
```

Fjall's write-permit is acquired by F1 via a blocking primitive (a std
`Mutex` / parking_lot guard internally). Even after F1 yields to tokio
by awaiting `async_fs::write`, the *permit* has not been released -- the
`Transaction` value still owns it. T1 now tries to run F2, which calls
`begin_transaction` and parks on that same Fjall lock. T1 can no longer
run anything else, because it is blocked on a non-async lock that is
held by a future currently parked on T2.

So far that is only thread starvation, not yet a deadlock. The deadlock
comes in layer 2.

### 2. `async_fs` spawns onto the blocking pool

`async_fs::{create_dir_all, write}` are `smol`/`blocking` shims: they
schedule the blocking syscall on a dedicated blocking-pool thread and
return a future that completes when that thread finishes. So F1's
`create_dir_all(...).await` is really:

```
F1 on T2 -> sends job to blocking-pool thread B -> awaits completion
```

When B finishes and signals, tokio wakes F1. The scheduler may resume F1
on **yet another** worker thread T3. That by itself is fine -- except
that, together with (1), it means the Fjall lock guard that F1 owns can
migrate across arbitrary worker threads between suspensions.

### 3. `mpsc::Sender::send(...).await` can back-pressure

The downstream collection channel used `futures::channel::mpsc::channel`
(bounded). `send(...).await` can block the sender when the channel is
full. In a run with five concurrent block futures and a slow collector,
F1 can end up parked *at the final `send` after commit*, still on a
worker thread, while other futures that just committed are ahead of it
in the queue. This multiplies the blast radius: each parked sender
pins a worker, and the pool runs out of runnable threads.

### Putting it together

Under load the scheduler reaches a state where:

- Each of the five concurrent block futures holds or is trying to hold
  the Fjall write permit.
- Futures that hold it are parked on some worker across an `.await` into
  `async_fs` or `mpsc::send`.
- Futures that want it are parked inside `fjall` on a blocking lock.
- `async_fs`'s blocking pool cannot always release in time, because the
  completion wake must land on a tokio worker -- and tokio workers are
  all parked on the Fjall lock.

The easiest way to see why this locks up, rather than merely serialising,
is that the Fjall write permit is a *blocking* primitive held *across an
async await*. That combination is what the async-sync rule exists to
forbid. As long as both (a) the permit is async-blind and (b) the permit
can travel across threads via `.await`, the scheduler is free to produce
a cycle between "worker waiting for permit" and "permit held by task
waiting for a worker to resume it".

## The c5f9cc9 fix: break the invariant, don't paper over it

The commit made four changes, all in `cas/fs.rs` and the Fjall store,
and each targets one of the three effects above.

### a. `AsyncFileSystem` is no longer async

```rust
// before
trait AsyncFileSystem {
    async fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    async fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
}
impl AsyncFileSystem for RealAsyncFs {
    async fn create_dir_all(...) { async_fs::create_dir_all(path).await }
    async fn write(...)          { async_fs::write(path, contents).await }
}

// after
trait AsyncFileSystem {
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
}
impl AsyncFileSystem for RealAsyncFs {
    fn create_dir_all(...) { std::fs::create_dir_all(path) }
    fn write(...)          { std::fs::write(path, contents) }
}
```

This removes the two `.await` points inside the critical section. The
block write now runs inline on whatever worker is polling the block
future. That worker blocks on the syscall for a few milliseconds, which
is far from ideal -- it should really be `spawn_blocking` -- but it is
**safe**: the Fjall permit, if held, cannot migrate to another thread,
and no other future can be scheduled on this worker mid-critical-section.
The name `AsyncFileSystem` is a misnomer after this change; it stayed
for diff hygiene.

### b. `for_each_concurrent(5, ...)` -> `for_each(...)`

```rust
// before
.for_each_concurrent(5, |(idx, bytes)| async move { ... })

// after
.for_each(|(idx, bytes)| async move { ... })
```

Sequential block processing per object. Multiple objects (from different
S3 requests) still run concurrently on the outer axis, but *within* a
single `store_object` there is never more than one transaction in flight.
This is the belt to the braces of (a): even if some future refactor
accidentally reintroduces an `.await` inside the critical section, there
is no second future to deadlock against on the same object.

Cost: a single large object writes its blocks serially. In practice the
bottleneck is disk anyway, and the object-level write path already fans
out via concurrent S3 requests, so the throughput hit is small compared
to the reliability win. We should revisit this once `spawn_blocking` is
wired in (see "Followups" below).

### c. `mpsc::send(..).await` -> `unbounded_send(..)`

```rust
// before
let (tx, rx) = futures::channel::mpsc::channel(BLOCK_SIZE / ...);
tx.send(Ok((idx, hash))).await?;

// after
let (tx, rx) = futures::channel::mpsc::unbounded();
tx.unbounded_send(Ok((idx, hash)))?;
```

Sending becomes a non-awaiting operation: push into an unbounded queue
and return. This removes the final `.await` point inside the block
future. It also eliminates effect (3) -- senders can no longer pile up
parked on back-pressure.

Memory footprint is bounded in practice by the fact that (b) made the
loop sequential; at most one send is outstanding per object at a time.

### d. Partition cache + cached `BlockTree`

```rust
// fjall.rs
pub struct FjallStore {
    keyspace: Arc<fjall::TxKeyspace>,
    // ...
    partition_cache: Arc<Mutex<HashMap<String, TxPartitionHandle>>>,
}

fn get_partition(&self, name: &str) -> Result<TxPartitionHandle, MetaError> {
    Ok(self.partition_cache.lock().unwrap()
        .entry(name.to_string())
        .or_insert_with(|| self.keyspace.open_partition(name, default).unwrap())
        .clone())
}

// fs.rs
pub struct CasFS {
    // ...
    block_tree: BlockTree,  // cached at construction
}
fn block_tree(&self) -> Result<BlockTree, MetaError> {
    Ok(self.block_tree.clone())
}
```

Before this change, `meta_store.get_block_tree()` was called once per
block write, and each call went through `keyspace.open_partition`, which
has its own internal lock inside Fjall. With the cache, the per-block
path touches only the already-opened handle: no Fjall-internal locks
beyond the write permit itself. `BlockTree` went from `Box<dyn ...>` to
`Arc<dyn ...>` so it could be cached and cheaply cloned.

A related ripple that `c5f9cc9` carried: `Box<dyn BaseMetaTree>` ->
`Arc<dyn BaseMetaTree>` everywhere in the metastore trait surface
(`MetaStore::get_*_tree`, `Store::tree_open`, `Store::tree_ext_open`).
Without this, the cache above would force reopening partitions on every
call just to convert ownership.

## The 228cd24 follow-up: shorten the critical section to zero

`c5f9cc9` made the deadlock impossible, but the Fjall write permit was
still held while the block file was being written to disk. That is bad
for throughput -- a slow disk syscall effectively serialises every other
block write against it -- and it reopens the original risk class the
moment anyone accidentally reintroduces an `.await` in that window.

`228cd24` flipped the order:

```rust
// before: commit after disk write
match write_meta_result {
    Ok((true, block)) => {
        pm.block_pending();
        block  // store_tx kept alive as Some(store_tx)
    }
    // ...
}
// ... disk write happens here, still inside the tx lifetime ...
Box::new(store_tx).commit()?;

// after: commit before disk write
match write_meta_result {
    Ok((true, block)) => {
        pm.block_pending();
        tracing::debug!(target: "cas_storage::locks", "Committing metadata transaction (new block)");
        Box::new(store_tx).commit().unwrap();  // lock released here
        block
    }
    // ...
}
// ... disk write happens here, no tx lock held ...
```

Commit happens *before* the block file is written. The Fjall write permit
is released the moment the metadata update (refcount +1 or new block
entry) is durable. The on-disk block write then runs on its own.

This breaks the refcount invariant if we stop there: a successful commit
followed by a failed disk write would leave the block tree claiming a
block exists that is not on disk. A later read of any object that
includes that hash would fail.

So `228cd24` also added a compensating transaction:

```rust
let cleanup_on_failure = || {
    // We just inserted the block with rc=1. If the on-disk write fails,
    // remove the block tree entry. Data leakage (keeping the entry) would
    // break reads; this is the data-loss-avoidance side of the coin.
    let block_tree = match &self.shared_meta_store {
        Some(shared) => shared.get_block_tree(),
        None => self.user_meta_store.get_block_tree(),
    };
    if let Ok(tree) = block_tree {
        if let Err(e) = tree.remove(&block_hash) {
            tracing::warn!(block = %hex_string(&block_hash), error = %e,
                "Failed to cleanup orphan block metadata");
        }
    }
};

if let Err(e) = self.async_fs.create_dir_all(block_path.parent().unwrap()) {
    cleanup_on_failure();
    tx.unbounded_send(Err(e))?;
    return;
}
if let Err(e) = self.async_fs.write(&block_path, &bytes) {
    cleanup_on_failure();
    tx.unbounded_send(Err(e))?;
    return;
}
```

This is a compensating transaction, not a rollback. The original
transaction committed; we are now issuing a second write to undo its
visible effect. Two properties hold:

- The cleanup runs only for the `is_new=true` branch. The duplicate
  case (`is_new=false`) committed no state change for the on-disk
  block, so no compensation is needed.
- If the cleanup itself fails, we are back in the "data leakage" regime:
  a block tree entry exists with `rc=1` but no file on disk, and any
  object using that block will fail to read. The fix for that is the
  integrity-check path (`s3-cas/src/check.rs`), which can scan for
  tree entries that have no backing file and clean them up offline.

To make the critical section visible in logs, `228cd24` also sprinkled
`tracing::debug!(target: "cas_storage::locks", ...)` at
`begin_transaction`, `commit` start, `commit` finish, and `rollback`.
Turn the `cas_storage::locks` target on if you ever suspect this path
again:

```
RUST_LOG=info,cas_storage::locks=debug s3-cas server ...
```

## Properties after both commits

- The Fjall write permit is held only for the microseconds that the
  metadata update takes. No `.await` inside the critical section. No
  slow I/O inside the critical section.
- Per-object block processing is sequential; parallelism comes from
  concurrent S3 requests, not from intra-object fan-out.
- Partition handles are cached; opening a handle is no longer a source
  of internal contention.
- Refcount invariants are preserved:
  - New block: metadata commits, then file is written. File-write
    failure triggers a compensating delete. Failure to compensate is
    logged and left for offline integrity check.
  - Duplicate block (same key): no refcount change, no file write, no
    risk.
  - Duplicate block (different key): refcount increment commits; no
    file write needed because the file is already on disk.
- The `cas_storage::locks` tracing target gives you a lock timeline if
  this ever misbehaves again.

## What could still go wrong (guardrails)

1. **Don't reintroduce `.await` inside a transaction.** The transaction
   must be committed or rolled back synchronously between `begin` and
   the next suspend point. If you find yourself wanting to await in
   there, split the transaction.
2. **Don't switch `AsyncFileSystem` back to async** without also
   guaranteeing that no Fjall lock is held when those methods are
   called. After `228cd24` that is already true, but the trait name is
   misleading and the next person to touch it could reasonably assume
   it was meant to be async.
3. **Don't widen the concurrency in `for_each` back to
   `for_each_concurrent`** without replacing the blocking disk writes
   with `tokio::task::spawn_blocking`. Even then, verify against the
   test `cas-storage/src/cas/fs.rs::tests::test_store_object_*` and
   consider adding a torture test with hundreds of parallel PUTs.
4. **Don't remove the compensating `cleanup_on_failure` call** in the
   new-block branch. The commit-before-write ordering depends on it.
5. **Don't cache partition handles anywhere other than `FjallStore`.**
   The handle is cheap to clone once opened; opening it is where the
   contention lives.

## Followups worth doing

- Wrap `std::fs::{create_dir_all, write}` in
  `tokio::task::spawn_blocking` so the tokio worker is not parked on the
  syscall. Runtime correctness does not require this (the critical
  section is gone) but throughput and fairness under load would improve.
- Reintroduce bounded concurrency inside `store_object` *after* the
  above, to amortise disk I/O across blocks of a large object. Likely a
  small `buffer_unordered(N)` on top of the `spawn_blocking` futures.
- Add a regression test that exercises concurrent multi-block PUTs
  across many S3 requests and asserts the server makes progress.
  Stalling tokio runtimes are easy to detect with a watchdog.
- Promote `cas_storage::locks` from `debug` to a dedicated span with
  timings, so `begin -> commit` latency shows up in metrics.

## References in the tree

- `cas-storage/src/cas/fs.rs`
  - `AsyncFileSystem` trait and `RealAsyncFs` impl -- blocking, not async.
  - `store_object` -- sequential `for_each`, `unbounded_send`,
    commit-before-write, `cleanup_on_failure`.
- `cas-storage/src/metastore/stores/fjall.rs`
  - `FjallStore::partition_cache` -- `HashMap<String, TxPartitionHandle>`
    behind a `Mutex`.
  - `FjallTransaction::commit`/`rollback` -- lock-timeline tracing under
    target `cas_storage::locks`.
- `cas-storage/src/metastore/meta_store.rs`
  - `BlockTree` -- now `Clone`, holds `Arc<dyn BaseMetaTree>`, cached on
    `CasFS` at construction.
- `docs/arch/refcount.md` -- the invariants this fix must never break.
