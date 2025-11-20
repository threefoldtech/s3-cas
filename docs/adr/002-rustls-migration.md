# ADR 002: Migration from OpenSSL to rustls

## Status
Proposed - 2025-11-20

## Context

The S3-CAS system currently depends on OpenSSL for TLS/HTTPS functionality through:
- **Direct dependency**: `openssl` crate (optional, via `vendored` feature)
- **Transitive dependencies**:
  - `rusoto_core` (AWS S3 API client) uses OpenSSL by default for request signing
  - `aws-sdk-s3` and `aws-config` (dev dependencies for testing) use OpenSSL

### Current Build Requirements

To build s3-cas with the vendored feature (for static/musl builds):
```bash
cargo build --features vendored
```

This compiles OpenSSL from source, which:
- ✅ Enables static linking and cross-platform builds
- ❌ Significantly increases build time (OpenSSL compilation)
- ❌ Requires C compiler toolchain
- ❌ Increases binary size
- ❌ Adds C code to the dependency chain (security/audit surface)

On systems without OpenSSL development headers:
```bash
# Arch Linux
sudo pacman -S pkg-config openssl

# Ubuntu/Debian
sudo apt-get install pkg-config libssl-dev
```

### Problems with Current Approach

1. **Build complexity**: Requires either system OpenSSL libs or vendored compilation
2. **C dependency chain**: Introduces C code into otherwise pure-Rust project
3. **Cross-compilation friction**: OpenSSL cross-compilation is notoriously difficult
4. **Supply chain**: Additional attack surface from C libraries
5. **Maintenance**: OpenSSL version management and security updates

### Alternative: rustls

**rustls** is a pure-Rust TLS library that:
- ✅ Written entirely in safe Rust (memory safe by design)
- ✅ No C dependencies (easier builds, smaller supply chain)
- ✅ Faster compilation (no C compilation step)
- ✅ Modern TLS 1.2 and 1.3 implementation
- ✅ Well-maintained by the Rust community (used by Cloudflare, AWS SDK, etc.)
- ✅ Transparent drop-in replacement for most use cases

## Decision

We will **migrate from OpenSSL to rustls** for all TLS functionality.

### Changes Required

#### 1. Remove OpenSSL Dependencies

```toml
# DELETE from Cargo.toml:
[features]
vendored = ["openssl"]

openssl = { version = "0.10.68", features = ["vendored"], optional = true }
```

#### 2. Update rusoto_core to use rustls

```toml
# BEFORE:
rusoto_core = "0.48.0"

# AFTER:
rusoto_core = { version = "0.48.0", default-features = false, features = ["rustls"] }
```

#### 3. Update AWS SDK dev-dependencies

```toml
# In [dev-dependencies]:

# BEFORE:
aws-config = { version = "1.5.8", default-features = false }
aws-sdk-s3 = { version = "1.56.0", features = ["behavior-version-latest"] }

# AFTER:
aws-config = { version = "1.5.8", default-features = false, features = ["rustls"] }
aws-sdk-s3 = { version = "1.56.0", default-features = false, features = ["behavior-version-latest", "rustls"] }
```

### TLS Configuration Transparency

**Important**: This migration affects **only** the TLS implementation library, not the application's HTTP/HTTPS configuration.

- ✅ S3 port can still run HTTP or HTTPS (your choice)
- ✅ Web UI can still run HTTP or HTTPS (your choice)
- ✅ Same certificate loading mechanisms
- ✅ No changes to server configuration code

**Example**: Configuring HTTPS with rustls
```rust
use rustls::{Certificate, PrivateKey, ServerConfig};
use rustls_pemfile::{certs, private_key};

// Load certificates (same PEM files as with OpenSSL)
let cert_file = File::open("cert.pem")?;
let key_file = File::open("key.pem")?;

let certs = certs(&mut BufReader::new(cert_file))?
    .into_iter()
    .map(Certificate)
    .collect();

let key = PrivateKey(private_key(&mut BufReader::new(key_file))?.unwrap());

// Build TLS config
let tls_config = ServerConfig::builder()
    .with_no_client_auth()
    .with_single_cert(certs, key)?;

// Use with hyper (or keep using plain HTTP)
Server::bind_with_tls(&addr, tls_config).serve(service)
```

## Consequences

### Positive
- ✅ **Pure Rust supply chain**: No C dependencies, reduced attack surface
- ✅ **Faster builds**: No OpenSSL compilation (especially with vendored feature)
- ✅ **Easier cross-compilation**: No OpenSSL cross-compilation headaches
- ✅ **Simpler builds**: No need for system OpenSSL dev packages
- ✅ **Modern TLS**: TLS 1.3 support with security-focused implementation
- ✅ **Memory safety**: Safe Rust throughout the TLS stack
- ✅ **Smaller binaries**: Often smaller than OpenSSL-linked binaries
- ✅ **Consistent across platforms**: Same TLS implementation everywhere

### Negative
- ⚠️ **Testing required**: Must verify AWS S3 signature generation works identically with rustls
- ⚠️ **Breaking change**: Users building with `--features vendored` will need to update their build scripts
- ⚠️ **Ecosystem maturity**: While rustls is mature, OpenSSL has longer history in production

### Neutral
- 📝 **New dependencies**: `rustls`, `rustls-pemfile` (small, well-maintained)
- 📝 **Certificate handling**: May need minor adjustments for certificate loading
- 📝 **Migration effort**: ~1 hour to update dependencies and test

## Implementation Plan

### Phase 1: Update Dependencies (5 minutes)
1. Remove `openssl` dependency and `vendored` feature from `Cargo.toml`
2. Update `rusoto_core` to use `rustls` feature
3. Update AWS SDK dev-dependencies to use `rustls` features
4. Run `cargo update` to fetch rustls dependencies

### Phase 2: Build Verification (10 minutes)
5. Verify clean build: `cargo build --release`
6. Check dependency tree: `cargo tree | grep -E "(openssl|rustls)"`
7. Confirm no OpenSSL in final binary: `ldd target/release/s3-cas | grep ssl`

### Phase 3: Functional Testing (30 minutes)
8. Run existing unit tests: `cargo test`
9. Run integration tests with AWS SDK: `cargo test --test it_s3`
10. Manual testing:
    - S3 API operations (PUT/GET/DELETE objects)
    - AWS signature verification
    - Multi-user authentication flows
    - HTTP UI functionality

### Phase 4: Documentation (15 minutes)
11. Update README build instructions (remove OpenSSL installation steps)
12. Update CI/CD scripts if they reference OpenSSL
13. Document any certificate loading changes (if needed)

## Verification Criteria

✅ **Success indicators**:
1. `cargo tree` shows no OpenSSL dependencies
2. `cargo build --release` completes without OpenSSL
3. All tests pass (`cargo test`)
4. S3 API integration tests pass
5. Manual upload/download works
6. Binary size same or smaller

❌ **Rollback triggers**:
1. AWS signature generation fails
2. Integration tests fail
3. Performance regression > 10%

## Alternatives Considered

### 1. Keep OpenSSL
**Rejected**: Maintains build complexity, C dependencies, and cross-compilation issues

### 2. Support both OpenSSL and rustls via features
```toml
[features]
default = ["rustls"]
rustls = ["dep:rustls", "rusoto_core/rustls"]
openssl = ["dep:openssl", "rusoto_core/native-tls"]
```
**Rejected**: Doubles testing surface, maintenance burden, and binary variants. Pick one standard.

### 3. Use native-tls (platform TLS)
**Rejected**: Platform-dependent behavior, still requires OpenSSL on Linux, not pure Rust

### 4. Defer until rustls is "more mature"
**Rejected**: rustls is already production-ready (used by Cloudflare, AWS SDK, etc.), waiting gains nothing

## References
- [rustls GitHub](https://github.com/rustls/rustls) - Modern TLS library in Rust
- [rusoto rustls support](https://docs.rs/rusoto_core/latest/rusoto_core/#tls) - AWS client rustls integration
- [AWS SDK for Rust](https://github.com/awslabs/aws-sdk-rust) - Uses rustls by default
- [Cargo.toml](../../Cargo.toml) - Current dependencies

## Notes
- This change aligns with Rust ecosystem trend toward pure-Rust dependencies
- Major Rust projects (Tokio, Hyper, AWS SDK) have adopted or are adopting rustls
- No application code changes required - purely a dependency swap
- Certificate files (PEM format) remain unchanged
- TLS configuration APIs are similar between OpenSSL and rustls bindings
