# PRD-000: Current-State Baseline and Restructure

Status:      Draft
Author:      Jan De Landtsheer
Date:        2026-04-24
Supersedes:  (none -- this is the anchor PRD for the project)
Related:     docs/adr/001-multi-user-authentication.md,
             docs/adr/002-rustls-migration.md,
             docs/adr/003-cas-storage-library-extraction.md,
             docs/multi-user-prd.md (historical),
             docs/IMPLEMENTATION_STATUS.md (historical),
             docs/refcount.md,
             MULTIPART_TRACE.md

## 1. Purpose

Establish a shared, honest baseline of what this repository currently is, how it
got here, and a target shape it should evolve toward so that future fixes and
features land in a coherent structure instead of accreting around organic
hotspots.

This PRD is intentionally descriptive before it is prescriptive. It freezes the
"as-found" state as of 2026-04-24 so later PRDs (PRD-001, PRD-002, ...) can
reference a stable baseline.

## 2. Background

s3-cas is a continuation of Lee Smet's original s3-cas project
(https://github.com/leesmet/s3-cas): an S3-compatible server backed by a
content-addressable store with block-level deduplication and reference
counting. The Fjall LSM key-value store holds all metadata; blocks live on a
plain filesystem under an adaptive directory tree.

Since the fork, the project grew the following capabilities on top of the
original single-tenant S3 server:

- Multi-user isolation with shared block deduplication (ADR-001).
- Session-based HTTP UI with first-time admin setup and an admin panel.
- Dual storage backends (fjall transactional vs fjall_notx non-transactional).
- Inline metadata for small objects.
- Prometheus metrics abstraction.
- CLI inspection / integrity-check / retrieval subcommands.
- Extraction of the storage core into a reusable library crate (ADR-003).

That growth happened opportunistically rather than against a single design
spine, which is the motivation for this document.

## 3. Current Repository Layout

```
s3-cas/
  Cargo.toml                  workspace (edition 2018)
  README.md
  MULTIPART_TRACE.md          ad-hoc trace doc, sits at repo root
  users.toml                  legacy sample config, no longer consumed
  docs/
    adr/
      001-multi-user-authentication.md
      002-rustls-migration.md
      003-cas-storage-library-extraction.md
    cas-storage-library-api.md
    IMPLEMENTATION_STATUS.md  status snapshot from the multi-user work
    multi-user-prd.md         original multi-user PRD (pre-DB-backed users)
    refcount.md
  cas-storage/                library crate
    src/
      lib.rs
      cas.rs, cas/{fs,block_stream,buffered_byte_stream,
                   multipart,range_request,shared_block_store}.rs
      metastore/{mod,meta_store,traits,block,object,bucket_meta,
                 errors,constants}.rs
      metastore/stores/{fjall,fjall_notx,test_utils}.rs
      metrics.rs
  s3-cas/                     application crate (server binary + adapters)
    src/
      main.rs, lib.rs, internal_macros.rs
      s3fs.rs                 s3s::S3 trait implementation
      s3_wrapper.rs           DynamicS3Auth + S3UserRouter
      auth/{mod,router,session,user_store}.rs
      http_ui/{mod,admin,auth,handlers,login,middleware,
                profile,responses,templates}.rs
      inspect.rs, check.rs, retrieve.rs
      metrics.rs              Prometheus wrapper of cas_storage::MetricsCollector
    benches/
      casfs_benchmark.rs, fjall_benchmark.rs
    tests/
      it_s3.rs                single integration test file
  .github/workflows/
    build.yaml, release.yaml
```

Indexing (moderate mode) currently reports 1092 nodes / 2931 edges across
~55 Rust source files. The largest concentrations are:

- `cas-storage/src/cas/fs.rs` -- 1198 lines, the core write path.
- `s3-cas/src/s3fs.rs`        -- 834  lines, the S3 protocol glue.
- `s3-cas/src/main.rs`        -- 604  lines, bootstrap + dual-mode wiring.

## 4. What Exists Today (Functional Inventory)

### 4.1 Storage core (crate: cas-storage)

- `MetaStore` over a pluggable `Store` trait with two Fjall backends.
  Partitions: `_BUCKETS`, `_BLOCKS`, `_PATHS`, `_MULTIPART_PARTS`, plus one
  partition per bucket.
- `Transaction::write_block` holds the single invariant that must never fail
  silently (increment refcount on duplicate block from a different key;
  otherwise data loss). See docs/refcount.md.
- `CasFS` -- the write path. 1 MiB MD5-chunked blocks, streamed through a
  `BufferedByteStream`, written with bounded concurrency (5). Handles inline
  storage for small objects, key-replacement cleanup
  (`handle_key_replacement`), and bucket/object deletion with refcount
  decrement.
- `SharedBlockStore` -- singleton wrapping the shared partitions (blocks,
  paths, multipart) that all user-scoped `CasFS` instances borrow from.
- `BlockStream` -- async read path with range-request support.
- `AsyncFileSystem` trait exists but has exactly one impl (`RealAsyncFs`).
  Intended seam for testing / alternative backends; not yet exploited.
- Metrics: `MetricsCollector` trait, `NoOpMetrics`, `SharedMetrics`. Library
  is metrics-backend-agnostic.

### 4.2 S3 adapter (crate: s3-cas)

- `S3FS` implements `s3s::S3` for a single `Arc<CasFS>`.
- `S3UserRouter` wraps `S3FS` creation per request, looking up
  `access_key -> user_id -> CasFS` on every call.
- `DynamicS3Auth` implements `s3s::auth::S3Auth` by querying `UserStore` on
  every request (constant-time compare of secret key via `subtle`).
- Depends on `s3s` pinned to git tag `v0.11.1`.

### 4.3 Authentication and user management

- `UserRecord` with separate UI credentials (login + bcrypt password, cost 12)
  and S3 credentials (access_key 20 chars + secret_key 40 chars).
- Users stored in three Fjall partitions on the shared store:
  `_USERS`, `_USERS_BY_LOGIN`, `_USERS_BY_S3_KEY`.
- `SessionStore` is in-memory only (24 h sessions, 32-byte random IDs, cookie
  `session_id`, HttpOnly / SameSite=Strict). Sessions are lost on restart.
- First-time setup: if the user table is empty, `/login` shows a setup form
  that creates the initial admin and auto-generates S3 credentials.
- Admin panel: list / create / delete users, reset passwords, toggle admin.

### 4.4 HTTP UI

Two distinct services:

- `HttpUiService` (single-user) with Basic Auth.
- `HttpUiServiceMultiUser` with session auth + admin panel.

Both are hidden behind an `HttpUiServiceEnum` wrapper. Each duplicates
request routing (`/buckets`, `/buckets/{bucket}`, `/download/...`, `/api/v1/...`,
`/health`) with minor differences.

### 4.5 Bootstrap (`s3-cas/src/main.rs`)

`main.rs` branches on `--access-key` / `--secret-key` presence:

- Both present -> `run_single_user`: constructs a dedicated `CasFS`, plus a
  second `CasFS` if the HTTP UI is enabled. Uses `SimpleAuth` for S3.
- Both absent  -> `run_multi_user`: constructs `SharedBlockStore`,
  `UserStore`, `SessionStore`, `UserRouter`, `S3UserRouter`, `DynamicS3Auth`,
  and spawns a background task for session cleanup and metric updates.
- One-of-two present -> error.

### 4.6 Ops surface

CLI subcommands beyond `server`:

- `inspect`    with subcommands `num-keys`, `disk-space`, `list-users`,
               `user-stats`, `list-buckets`, `bucket-stats`, `block-stats`,
               `object-info` (all accept `--users-config` which is a legacy
               artefact).
- `retrieve`   pull an object out directly from the store.
- `check`      integrity check.

Metrics server on a separate port, Prometheus text format.

## 5. Pain Points and Accretion (the "rabbit-hole" diagnosis)

### 5.1 Two-shape `CasFS`

`CasFS` carries both `user_meta_store: MetaStore` and
`shared_meta_store: Option<Arc<MetaStore>>`, plus `shared_path_tree:
Option<Arc<dyn BaseMetaTree>>`. `new` leaves the optional fields `None`;
`new_multi_user` fills them. Downstream code branches on "is this single- or
multi-user" at multiple points instead of treating single-user as a
one-user special case. Every new storage-level feature has to remember the
bifurcation.

### 5.2 Multipart tree is global in multi-user mode

`_MULTIPART_PARTS` lives under `/meta_root/blocks/db/` and is shared by every
user. It works today because part keys include a UUID-per-upload, but it
couples user-facing state into the shared namespace and makes per-user
quotas, auditing, and cleanup harder to reason about.

### 5.3 Dual HTTP UI and dual bootstrap

Single-user and multi-user modes duplicate request routing, auth wiring, and
CasFS construction. New UI features have to be written twice or explicitly
guarded behind "multi-user only". The "enum wrapper over two full services"
shape is the symptom.

### 5.4 Stale and scattered documentation

- `docs/multi-user-prd.md` describes a TOML-driven user config that was
  replaced by DB-backed users. Never marked superseded.
- `docs/IMPLEMENTATION_STATUS.md` claims "~90% complete, integration pending"
  but `run_multi_user` is fully wired today.
- `MULTIPART_TRACE.md` sits at the repo root instead of under `docs/`.
- `users.toml` at the repo root is no longer consumed by the server (the
  runtime path is DB-backed), but it still ships as the example.
- There is no index or changelog for the ADR / PRD set.

### 5.5 Lingering knobs and deps

- `inlined_metadata_size` defaults to 1 byte (inlining effectively disabled)
  and is plumbed through every constructor; never actually exercised in prod.
- `rusoto_core::ByteStream` is still a library-level type on the core write
  path, despite ADR-003 flagging it as a concern; drags an AWS dependency
  into what should be a protocol-agnostic library.
- `openssl` is still optional on the `vendored` feature despite ADR-002
  proposing full rustls migration.
- Workspace `edition = "2018"`.
- `s3s` is pinned to a git tag rather than a crates.io release.

### 5.6 Known gaps documented only in passing

- Sessions non-persistent (lost on restart).
- No CSRF tokens on HTML forms.
- No rate limiting on `/login` or `/setup-admin`.
- No audit log for admin actions.
- No per-user metrics, no quotas.
- Single integration test (`it_s3.rs`); benches exist but not in CI.

None of these are bugs in the current feature set; they are future-feature
shaped holes.

## 6. Design Invariants (must not be broken by restructure)

These are load-bearing and should be preserved by any future refactor:

1. **Block refcount safety**: `write_block` never silently skips an
   increment on a duplicate block from a different key. Data leakage on
   failure is acceptable; data loss is not. (docs/refcount.md)
2. **Short metadata transactions**: transactions hold locks for microseconds
   only; no filesystem I/O inside a transaction. This is what lets multi-user
   writes scale.
3. **Shared block CAS, per-user metadata**: deduplication is global; object
   namespaces are per-user. Restructuring must not accidentally split or
   merge those.
4. **MD5 as the block identifier** for now. Multi-hash support is explicitly
   out of scope until a dedicated PRD opens it (YAGNI, per ADR-003).
5. **1 MiB chunk size** as the write-path invariant.
6. **Inline-metadata size as a knob** (even if defaulted off); do not hard
   code.

## 7. Target Shape (proposed)

The goal is not a rewrite. It is to name the seams that already exist and
stop making "single-user vs multi-user" a branch at every layer.

### 7.1 Component: cas-storage (library)

Scope: generic content-addressable storage with block dedup, refcount, and
per-namespace metadata. Protocol-agnostic. No S3, no HTTP, no auth, no AWS.

Public surface (unchanged intent, tightened implementation):

- `MetaStore`, `Store`, `BaseMetaTree`, `MetaTreeExt`, `Transaction`,
  `Durability`, `StorageEngine`, `FjallStore`, `FjallStoreNotx`.
- `Block`, `BlockID`, `Object`, `ObjectData`, `ObjectType`, `BucketMeta`,
  `MetaError`.
- `CasFS`, `SharedBlockStore`, `BlockStream`, `RangeRequest`, `MultiPart`,
  `MultiPartTree`.
- `MetricsCollector`, `NoOpMetrics`, `SharedMetrics`.

Invariants it must enforce internally:

- `CasFS` is always namespace-scoped. "Single-user" is simply the namespace
  `default`. There is one constructor (`CasFS::new`) that always takes a
  `SharedBlockStore` and a namespace identifier. No optional shared trees.
- `ByteStream` input is abstracted behind a local trait
  (`cas_storage::AsyncByteStream` or re-used `futures::Stream`) so that
  `rusoto_core` is not a dependency of the library.
- `AsyncFileSystem` seam gets at least one test-only in-memory impl, used by
  `cas-storage`'s own unit tests.

### 7.2 Component: s3-cas-server (application)

Scope: S3 protocol adapter, HTTP UI, auth, metrics export, CLI.
No knowledge of Fjall internals or block mechanics beyond what the library
exposes.

Internal split:

- `protocol::s3`     S3FS + s3-protocol-specific routing.
- `auth`             UserStore, SessionStore, session + S3 credential
                     adapters. No "single-user vs multi-user" branch; the
                     single-user case is a pre-provisioned namespace with
                     a fixed credential pair.
- `http_ui`          One service, parameterised by auth policy, instead of
                     two services joined by an enum.
- `ops`              inspect / check / retrieve. Becomes a single module
                     with one dispatch; CLI subcommands are thin shells.
- `bootstrap`        `main.rs` wires the above. A single `run_server`
                     function; no `run_single_user` / `run_multi_user`
                     fork.

### 7.3 Future adapters (non-blocking, but library must support)

The library must stay clean enough that any of these can land without
touching `cas-storage`:

- `resp-cas`      RESP (Redis-protocol) adapter -- already referenced in
                  ADR-003.
- `grpc-cas`      gRPC adapter.
- `webdav-cas`    WebDAV adapter for filesystem-style consumers.

Each adapter lives in its own crate under the workspace and depends only on
`cas-storage` + auth (if they need user isolation).

### 7.4 Documentation layout

```
docs/
  prd/
    prd000-current-state-and-restructure.md   (this file)
    prd001-...
  adr/
    001, 002, 003, ...
  arch/
    refcount.md
    multipart.md      (MULTIPART_TRACE.md moved and cleaned up)
    storage-layout.md
  historical/
    multi-user-prd.md
    IMPLEMENTATION_STATUS.md
  INDEX.md            (one-liner index of PRDs + ADRs, status column)
```

- PRDs are numbered, never renumbered; superseded PRDs link forward.
- ADRs are numbered, never renumbered; ADRs can reference PRDs and vice
  versa.
- `docs/historical/` holds frozen artefacts that are still useful as
  context but must not be read as current design.

## 8. Near-Term Restructure Work (candidate PRDs)

Out of scope for this PRD (each gets its own):

- PRD-001: unify `CasFS` single- / multi-user construction;
  retire optional shared trees; single-user as namespace `default`.
- PRD-002: (collapsed into the baseline-simplify pass, see below) --
  HTTP UI and single-user mode were deleted outright rather than
  unified.
- PRD-003: remove `rusoto_core::ByteStream` from the library surface;
  define an internal stream abstraction.
- PRD-004: (deferred, no longer urgent) persistent session store, CSRF,
  login rate limiting. Reopens only if an HTTP admin UI is reintroduced.
- PRD-005: per-user metrics and quotas.
- PRD-006: upgrade to a current Rust edition, pin `s3s` to a crates.io
  release or an internal mirror, execute ADR-002 rustls migration.
- PRD-007: (partially done in the baseline-simplify pass, see below)
  docs reorganisation; `users.toml` and `MULTIPART_TRACE.md` at the
  repo root have been relocated. Remaining: `docs/INDEX.md`.
- PRD-008: expand integration tests and bring benchmarks into CI.

The ordering above is roughly dependency order, not schedule; PRDs do not
block each other except where explicitly stated.

### Baseline simplify (2026-04-24, branch `simplify/drop-ui-and-single-user`)

Applied directly rather than spun out as a PRD because the change was
subtractive:

- Deleted `s3-cas/src/http_ui/` (9 files) and the session store.
- Deleted `run_single_user` and all HTTP UI / single-user CLI flags;
  server mode is multi-user only.
- Added `s3-cas user {add,list,delete,reset-password}` to replace the
  admin panel as the user-management surface.
- Dropped deps: `maud`, `urlencoding`, `cookie`, `base64`, `subtle`,
  `toml`, `lazy_static`, `console-subscriber`, `openssl` (+ `vendored`
  feature).
- Relocated `users.toml` (deleted), `MULTIPART_TRACE.md` ->
  `docs/arch/multipart-trace.md`, `docs/refcount.md` ->
  `docs/arch/refcount.md`, `docs/multi-user-prd.md` ->
  `docs/historical/`, `docs/IMPLEMENTATION_STATUS.md` ->
  `docs/historical/`.
- Tests fixed to use the post-ADR-003 `cas_storage::` paths.

## 9. Non-Goals

- Rewriting the storage core. The refcount logic, transaction shape, and
  block layout are considered correct and must survive the restructure.
- Switching hash algorithm, chunk size, or block-path scheme.
- Introducing new user-facing features (quotas, audit, TLS termination) as
  part of the restructure itself. Those are separate PRDs.

## 10. Open Questions

- Should `s3-cas` remain one binary with subcommands, or split into a
  dedicated `s3-cas-inspect` / `s3-cas-check` / `s3-cas-retrieve`?
- Should the Fjall backend choice be compile-time (feature flag) or remain
  a runtime `--metadata-db` flag? Today both engines link unconditionally.
- Is the shared `_MULTIPART_PARTS` tree worth moving under per-user
  metadata for isolation, or does the UUID key space make it a non-issue?
- Do we want to keep the `inlined_metadata_size` knob or commit to always
  inlining below a fixed threshold?

Resolution of each belongs in its own PRD.

## 11. References

- docs/adr/001-multi-user-authentication.md
- docs/adr/002-rustls-migration.md
- docs/adr/003-cas-storage-library-extraction.md
- docs/refcount.md
- MULTIPART_TRACE.md
- docs/multi-user-prd.md (historical)
- docs/IMPLEMENTATION_STATUS.md (historical)
- README.md
- CLAUDE.md (in-repo code map)
