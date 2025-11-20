# ADR 003: Extract CAS Storage Layer into Reusable Library

## Status
Proposed - 2025-11-20

## Context

The S3-CAS system currently combines two distinct concerns in a single codebase:

1. **Generic CAS storage layer**: Content-addressable block storage with deduplication, reference counting, metadata management, and multi-user support
2. **S3 protocol adapter**: S3 API implementation, authentication, HTTP UI, and server runtime

### Current Structure

```
s3-cas/
├── metastore/        # Generic metadata management
├── cas/              # Generic block storage with dedup
├── s3fs.rs           # S3 protocol implementation
├── s3_wrapper.rs     # S3 multi-user routing
├── auth/             # User management
├── http_ui/          # Admin web interface
└── main.rs           # Server binary
```

### Problems with Monolithic Structure

1. **No reusability**: Cannot use the excellent CAS storage layer in other protocols (RESP, gRPC, custom APIs)
2. **Unclear boundaries**: Protocol-specific and generic code mixed in same project
3. **Testing complexity**: Cannot test storage layer independently from S3 protocol
4. **Coupling**: Storage improvements require rebuilding entire S3 server

### Motivation for Library

We want to reuse the CAS storage layer in other contexts:
- RESP (Redis protocol) server for content-addressable file storage
- gRPC API for high-performance block storage
- Custom protocols with different semantics than S3
- Embedded use cases (library integration into other Rust applications)

The storage layer is already well-architected with clean abstractions (`Store` trait, `MetaStore`, `CasFS`, `SharedBlockStore`). Extraction requires organizational changes more than architectural redesign.

## Decision

We will **extract the CAS storage layer into a separate `cas-storage` library crate** that can be used by multiple protocol adapters.

### Architecture

```
┌─────────────────────────────────────────┐
│  Applications (separate binaries)       │
│  ├─ s3-cas (S3 server)                  │
│  ├─ resp-cas (RESP server - future)     │
│  └─ grpc-cas (gRPC server - future)     │
└──────────────┬──────────────────────────┘
               │ (depends on)
┌──────────────▼──────────────────────────┐
│  cas-storage (library crate)            │
│  ├─ metastore/ (metadata management)    │
│  ├─ cas/ (block storage + dedup)        │
│  └─ Public API (CasFS, MetaStore, etc.) │
└─────────────────────────────────────────┘
```

### What Moves to Library

**New crate: `cas-storage`**

```rust
cas-storage/
├── Cargo.toml
├── src/
│   ├── lib.rs              // Public API exports
│   ├── metastore/
│   │   ├── mod.rs
│   │   ├── meta_store.rs   // MetaStore
│   │   ├── traits.rs       // Store, BaseMetaTree, MetaTreeExt, TransactionBackend
│   │   ├── block.rs        // Block, BlockID
│   │   ├── object.rs       // Object, ObjectData, ObjectType
│   │   ├── bucket_meta.rs
│   │   ├── errors.rs
│   │   ├── constants.rs
│   │   └── stores/
│   │       ├── fjall.rs
│   │       └── fjall_notx.rs
│   └── cas/
│       ├── mod.rs
│       ├── fs.rs                    // CasFS
│       ├── shared_block_store.rs   // SharedBlockStore
│       ├── block_stream.rs
│       ├── buffered_byte_stream.rs
│       ├── multipart.rs
│       └── range_request.rs
```

**Public API** (example):
```rust
// Core types
pub use metastore::{
    Block, BlockID, Object, ObjectData, ObjectType,
    MetaStore, BlockTree, BucketMeta, MetaError,
    Store, BaseMetaTree, MetaTreeExt, Transaction,
    FjallStore, FjallStoreNotx, Durability, StorageEngine,
};

pub use cas::{
    CasFS, SharedBlockStore,
    BlockStream, RangeRequest, MultiPart,
};

// Main entry point
impl CasFS {
    pub fn new(...) -> Self;
    pub fn new_multi_user(...) -> Self;

    pub async fn store_object(...) -> Result<...>;
    pub async fn get_object_paths(...) -> Result<...>;
    pub async fn delete_object(...) -> Result<()>;

    pub fn create_bucket(...) -> Result<()>;
    pub fn list_buckets(...) -> Result<Vec<BucketMeta>>;
    // ...
}
```

### What Stays in s3-cas

**Application crate: `s3-cas`**

```rust
s3-cas/
├── Cargo.toml          // depends on: cas-storage = { path = "../cas-storage" }
├── src/
│   ├── main.rs         // Server binary
│   ├── s3fs.rs         // S3FS (implements s3s::S3 trait)
│   ├── s3_wrapper.rs   // S3UserRouter, DynamicS3Auth
│   ├── auth/           // User management, sessions
│   │   ├── user_store.rs
│   │   ├── session.rs
│   │   └── router.rs
│   ├── http_ui/        // Admin web interface
│   │   ├── mod.rs
│   │   ├── templates.rs
│   │   ├── admin.rs
│   │   └── middleware.rs
│   └── metrics.rs      // Prometheus metrics wrapper
```

### Design Principles

1. **Protocol-agnostic core**: Library has zero knowledge of S3, HTTP, or any protocol
2. **Keep MD5**: Continue using MD5 as hash algorithm (no multi-algorithm support for v1)
3. **Preserve multi-user**: SharedBlockStore and per-user CasFS architecture stays intact
4. **Minimal changes**: Leverage existing abstractions (`Store`, `MetaStore`, `CasFS`)
5. **Clean API**: Only expose public types needed by protocol adapters

## Consequences

### Positive

- ✅ **Reusable storage layer**: Can build RESP, gRPC, or custom protocol servers using same storage
- ✅ **Clean separation**: Protocol concerns clearly separated from storage concerns
- ✅ **Independent testing**: Storage layer can be tested without S3 dependencies
- ✅ **Modularity**: Applications only include what they need (no S3 deps if using RESP)
- ✅ **Standard Rust structure**: Follows cargo workspace conventions
- ✅ **Low risk**: Storage layer already well-abstracted, minimal architectural changes needed
- ✅ **Future-proof**: Easy to add new protocols without modifying storage layer

### Negative

- ⚠️ **Build complexity**: Now have 2 crates instead of 1 (mitigated by cargo workspace)
- ⚠️ **API surface**: Need to carefully define public API (what should be exposed vs internal)
- ⚠️ **Breaking change**: Existing deployments need to rebuild from new crate structure
- ⚠️ **Migration effort**: ~4-6 hours to reorganize codebase and update imports

### Neutral

- 📝 **Metrics**: Need to decide if metrics stay in library (pluggable) or move to applications
- 📝 **ByteStream**: May need to abstract or replace `rusoto_core::ByteStream`
- 📝 **Error types**: MetaError and others become part of library public API
- 📝 **Documentation**: Library needs good API docs for external users

## Implementation Plan

### Phase 1: Create Library Crate (2 hours)

1. Create `cas-storage/` directory
2. Initialize `Cargo.toml` with dependencies:
   - `fjall`, `tokio`, `async-fs`, `md-5`, `futures`, `bytes`, `anyhow`, `tracing`
3. Create `src/lib.rs` with public API exports
4. Move `metastore/` module to `cas-storage/src/metastore/`
5. Move `cas/` module to `cas-storage/src/cas/`
6. Update internal imports (convert absolute to relative)

### Phase 2: Update s3-cas Application (1 hour)

7. Add dependency: `cas-storage = { path = "../cas-storage" }`
8. Update imports in `s3fs.rs`, `s3_wrapper.rs`, `auth/router.rs`, `main.rs`
9. Remove moved modules from s3-cas
10. Update `use` statements throughout

### Phase 3: Handle Edge Cases (1-2 hours)

11. **Metrics**: Decide approach (keep in library with trait? move to app layer?)
12. **ByteStream**: Abstract or keep `rusoto_core` dependency in library
13. **Shared types**: Ensure all public types are properly exported
14. **Error types**: Make MetaError and related types part of public API

### Phase 4: Testing & Verification (1 hour)

15. Build library: `cd cas-storage && cargo build`
16. Build application: `cd s3-cas && cargo build --release`
17. Run existing tests: `cargo test`
18. Verify server still works: Start s3-cas server and test S3 operations
19. Check no circular dependencies: `cargo tree`

### Phase 5: Documentation (1 hour)

20. Add rustdoc comments to public API in `cas-storage/src/lib.rs`
21. Create `cas-storage/README.md` with usage examples
22. Update main `README.md` to document crate structure
23. Document migration guide for existing deployments

**Total estimated effort**: ~6-8 hours

## Verification Criteria

✅ **Success indicators**:
1. `cas-storage` builds independently: `cargo build` succeeds
2. `s3-cas` builds with library dependency: `cargo build --release` succeeds
3. All existing tests pass: `cargo test`
4. S3 server starts and handles requests normally
5. No circular dependencies: `cargo tree` shows clean dependency graph
6. Public API is minimal and well-documented

❌ **Rollback triggers**:
1. Cannot achieve clean separation without major refactoring
2. Performance regression > 5%
3. Existing tests fail and cannot be fixed within reasonable time

## Alternatives Considered

### 1. Keep monolithic structure
**Rejected**: Prevents reuse of storage layer in other protocols

### 2. Extract only metastore, keep CasFS in s3-cas
**Rejected**: CasFS contains the core deduplication logic and should be reusable

### 3. Create separate repo for cas-storage
**Rejected**: Cargo workspace in same repo is simpler for initial version. Can always split later.

### 4. Use git submodules
**Rejected**: Cargo workspaces are the idiomatic Rust approach

### 5. Expose everything as public API
**Rejected**: Need to carefully curate public API to avoid breaking changes later

### 6. Support multiple hash algorithms immediately
**Rejected**: YAGNI - MD5 works fine, can add abstraction later if needed

## References

- [Cargo Workspaces](https://doc.rust-lang.org/book/ch14-03-cargo-workspaces.html) - Multi-crate projects
- [API Guidelines](https://rust-lang.github.io/api-guidelines/) - Rust API design principles
- [ADR 001](001-multi-user-authentication.md) - Multi-user architecture
- [ADR 002](002-rustls-migration.md) - rustls migration (library would use rustls too)

## Notes

- This refactoring enables future protocols (RESP, gRPC) without modifying storage layer
- The storage layer is already well-designed with clean abstractions
- Main work is organizational (moving files, updating imports) not architectural
- Metrics and ByteStream abstraction are the main technical challenges
- Consider workspace structure: `workspace { members = ["cas-storage", "s3-cas"] }`
- Library should have minimal dependencies (no S3, no HTTP, no UI frameworks)
