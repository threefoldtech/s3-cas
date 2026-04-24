# PRD-003: Remove `rusoto_core::ByteStream` from the library surface

Status:     Draft
Author:     Jan De Landtsheer
Date:       2026-04-25
Related:    docs/adr/003-cas-storage-library-extraction.md (historical;
            flagged `ByteStream` as a leak into the library surface)
            docs/prd/prd000-current-state-and-restructure.md
            docs/prd/prd002-s3s-edition-tls.md (Deliverable C
            interaction, see sect 5)

## 1. Purpose

`rusoto_core::ByteStream` is the only reason `rusoto_core` is still in
the workspace dependency graph. Nothing in the project uses Rusoto as
an S3 client; the crate comes in solely because `cas-storage`'s
public `store_object` / `store_single_object_and_meta` APIs take a
`rusoto_core::ByteStream` as input.

`rusoto_core` drags in:

- A TLS backend (currently `rustls` per the recent switch; previously
  `openssl` via the default features). `rusoto_core` requires a TLS
  backend to compile because of an internal `SHARED_CLIENT` global,
  even when we use only the `ByteStream` type.
- `hyper-rustls`, `tokio-rustls`, `webpki`, `rustls-native-certs`,
  `openssl-probe`, and a small transitive tail of AWS-shaped
  unrelated machinery.

None of that serves this project. The Rusoto crates are
unmaintained upstream (last release 2022). Keeping them drags a dead
dep tree through every build.

This PRD defines a minimal replacement type inside `cas-storage`,
migrates the library and the `s3-cas` binary to it, and removes
`rusoto_core` from both Cargo.tomls.

## 2. Scope

In scope:

- Define `cas_storage::AsyncByteStream` -- a thin wrapper over
  `Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send + 'static>>`.
  `Send` bound (no `Sync`) matches what `rusoto_core::ByteStream`
  exposed for our use and matches what `futures::Stream` consumers
  actually need.
- Replace `rusoto_core::ByteStream` in the public and internal
  surface of `cas-storage`:
  - `CasFS::store_object` input type.
  - `CasFS::store_single_object_and_meta` input type.
  - `BufferedByteStream::new` input type.
  - Internal test helpers in `cas/fs.rs::tests`.
- Update the `s3-cas` callers:
  - `s3-cas/src/s3fs.rs::put_object` / `upload_part` build
    `AsyncByteStream` instead of `ByteStream::new_with_size(...)`.
    Length tracking, if needed, is orthogonal -- the current code
    uses `content_length` from the S3 request independently.
  - `s3-cas/benches/casfs_benchmark.rs` (pre-existing broken, see
    non-goals). Update for consistency when touched.
- Drop `rusoto_core` from `cas-storage/Cargo.toml` and
  `s3-cas/Cargo.toml`.

Out of scope:

- Any change to block layout, refcount logic, transaction shape, or
  chunk size. This is a type-substitution PRD.
- Fixing `s3-cas/benches/*` beyond the mechanical `ByteStream`
  substitution. Benches have been broken since the library split
  (see `cargo check --workspace --lib --bins --tests` excluding
  them).
- Any user-visible behaviour change.
- Introducing `bytes::Bytes` through more of the internal pipeline
  than it already flows. The library continues to produce
  `Vec<Vec<u8>>` out of `BufferedByteStream`.

## 3. Invariants

Restated from PRD-000 sect 6; this PRD must preserve them by
structure, not by comment:

1. `Transaction::write_block` refcount safety.
2. Short metadata transactions; no `.await` inside them.
3. Shared block CAS, per-namespace metadata.
4. MD5 block id, 1 MiB chunk size, `inlined_metadata_size` runtime
   knob.
5. Metadata commits before on-disk write; compensating delete on
   failure.

## 4. Target type

```rust
// cas-storage/src/cas/byte_stream.rs (or colocated)

use bytes::Bytes;
use futures::Stream;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A borrowed-or-owned async byte stream, modelled as a pinned,
/// type-erased `Stream` of `Bytes` chunks.
///
/// This is the minimum viable input type for `CasFS::store_object`.
/// It replaces `rusoto_core::ByteStream`, which was pulled in solely
/// for this role.
pub struct AsyncByteStream {
    inner: Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send + 'static>>,
}

impl AsyncByteStream {
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = io::Result<Bytes>> + Send + 'static,
    {
        Self { inner: Box::pin(stream) }
    }
}

impl Stream for AsyncByteStream {
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}
```

Re-exported from `cas_storage::AsyncByteStream` for consumers.

## 5. Migration path

### 5.1 `cas-storage`

Step-by-step so each intermediate state compiles:

1. Add `byte_stream.rs` with `AsyncByteStream` as above. Add to
   `cas.rs` module tree and re-export from `lib.rs`.
2. Update `BufferedByteStream::new(bs: AsyncByteStream)` and switch
   `bs: AsyncByteStream` field. Internal `poll_next` loop unchanged.
3. Update `CasFS::store_object(data: AsyncByteStream)` and
   `CasFS::store_single_object_and_meta(data: AsyncByteStream)`.
4. Update in-crate tests to build `AsyncByteStream::new(...)` from
   the same `stream::once(...)` sources as today.
5. Remove `rusoto_core` from `cas-storage/Cargo.toml`.

### 5.2 `s3-cas`

`s3fs.rs` currently converts the incoming `s3s` `StreamingBlob` into
a rusoto `ByteStream` via `ByteStream::new_with_size(converted, len)`.
The replacement is `AsyncByteStream::new(converted)` -- the
`new_with_size` is for hyper body compat which we do not need, we
already receive `content_length` as a separate parameter in the S3
request.

`retrieve.rs`, `check.rs`, and `tests/it_s3.rs` are read-only against
CasFS; they do not construct `ByteStream` and need no changes.
`benches/casfs_benchmark.rs` does construct `ByteStream` but was
broken pre-existing (ADR-003 artefact).

Remove `rusoto_core` from `s3-cas/Cargo.toml` last.

## 6. Interaction with PRD-002 (TLS stack consolidation)

PRD-002 Deliverable C calls for "one TLS stack across the whole
binary, with a documented decision for or against rustls". This PRD
is part of that story: `rusoto_core` was one of the two entry points
into the TLS tree (the other being `s3s` via `hyper-rustls`, which
we need and do not want to remove). Once this PRD lands:

- `cas-storage` ships with zero transitive TLS dependencies. It is
  truly protocol-agnostic.
- `s3-cas` still has `hyper-rustls` transitively via `s3s`; that is
  load-bearing and stays.

After this PRD, PRD-002 Deliverable C's scope is reduced to
"decide whether to also drop the `s3s-aws` dev-dep's TLS surface" --
which is a dev-only concern and probably does not need its own PRD.

## 7. Verification

- `cargo tree -p s3-cas -e normal | grep rusoto` returns nothing.
- `cargo tree -p s3-cas -e normal | grep openssl` returns nothing
  (was already true after the earlier rustls switch; this PRD keeps
  it true).
- `cargo test --workspace --lib --bins --tests` passes with the
  same test count.
- `cargo clippy --workspace --lib --bins --tests --no-deps` clean.
- `cas_storage/src/lib.rs` no longer mentions `rusoto_core` in its
  re-exports or docs.

## 8. Success criteria

- The public `cas-storage` crate API exposes `AsyncByteStream` as
  the input type for object writes; the exported surface does not
  leak any `rusoto_core` type.
- `rusoto_core` and the `rusoto_*` transitive chain are gone from
  `Cargo.lock`.
- No on-disk format change.

## 9. Non-goals

- Introducing an "AsyncRead"-shaped input alternative. The
  `poll_next`-based `Stream` model is the one the write path wants;
  see the comment in `BufferedByteStream` about not adding a
  `tokio::io` dependency here.
- Making `AsyncByteStream` `Sync`. Neither the old `rusoto_core`
  ByteStream nor our own usage requires `Sync`.
- A `TryFrom<reqwest::Response>` or similar convenience impl. Keep
  the type minimal; callers build it from whatever `Stream` they
  have.
