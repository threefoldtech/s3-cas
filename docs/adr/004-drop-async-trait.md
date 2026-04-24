# ADR 004: Drop `async_trait` where we own the trait

Status:     Proposed - 2026-04-24
Author:     Jan De Landtsheer
Related:    docs/prd/prd000-current-state-and-restructure.md (PRD-006 note)
            docs/prd/prd001-cas-storage-internal-restructure.md

## Context

Rust 1.75 stabilised `async fn` in traits (AFIT); 1.82 added return-type
notation for `dyn`-compatibility; we are currently compiling on
1.95-class toolchains. The `async_trait` procedural macro is a
compatibility shim for compilers that predate AFIT and for the cases
where `dyn` compatibility forces heap-allocated return types. Neither
constraint applies to this crate anymore.

Current usage across the workspace:

| File                               | Trait / impl                               | Owned by us?                            |
| ---------------------------------- | ------------------------------------------ | --------------------------------------- |
| `cas-storage/src/cas/fs.rs`        | `trait AsyncFileSystem` + `RealAsyncFs`    | yes                                     |
| `s3-cas/src/s3fs.rs`               | `impl s3s::S3 for S3FS`                    | no (s3s defines `S3` with async_trait)  |
| `s3-cas/src/s3_wrapper.rs`         | `impl s3s::auth::S3Auth`, `impl s3s::S3`   | no                                      |
| `s3-cas/src/metrics.rs`            | `impl<T: S3> s3s::S3 for MetricFs<T>`      | no                                      |

The `AsyncFileSystem` case is worse than "we could drop async_trait":
the trait's methods are not even `async` anymore. After the deadlock
fix in `c5f9cc9`, both methods became plain synchronous functions
(`fn create_dir_all`, `fn write`) that delegate to `std::fs`. The
`#[async_trait]` attribute on the trait and its impl is dead
decoration that slows builds, widens the dependency tree, and
misleads readers into thinking there is async work happening.

The `s3s::S3` and `s3s::auth::S3Auth` cases are the opposite:
`async_trait` is imposed on us because the upstream trait is defined
with it. We cannot remove the macro from an impl while the trait itself
still uses it -- the desugared signatures would not match.

## Decision

Remove `async_trait` from code we own. Keep it in the s3s glue until
s3s itself migrates.

Concretely:

1. Strip the `#[async_trait]` attributes from `AsyncFileSystem` and
   `RealAsyncFs` in `cas-storage/src/cas/fs.rs`. The trait methods are
   already synchronous; this is a one-line delete per attribute.
2. Remove the `use async_trait::async_trait;` import in
   `cas-storage/src/cas/fs.rs`.
3. Remove the `async-trait` workspace dependency from
   `cas-storage/Cargo.toml` once step 1 is done (s3-cas/Cargo.toml
   still needs it).
4. Keep `async_trait` in `s3-cas/Cargo.toml`, `s3fs.rs`,
   `s3_wrapper.rs`, and `metrics.rs`. They implement external traits
   that still require it.
5. When PRD-001 lands and `AsyncFileSystem` moves to
   `cas-storage/src/cas/async_fs.rs`, the new file must not
   reintroduce the macro.

## Why not also remove it from the s3s impls

Two options exist and both are worse than waiting:

- **Rewrite the impls to use manual `Pin<Box<dyn Future ...>>` returns.**
  This is exactly what `async_trait` does for us; we would be inlining
  a macro expansion for zero readability win.
- **Fork s3s locally and strip `async_trait` there.** Drags the whole
  s3s codebase into our maintenance surface to save one dependency.
  Out of scope; the upstream project will migrate when it does.

The right time to remove it from the s3s-facing code is when s3s
itself switches to native AFIT. That upgrade is tracked as part of
PRD-006 ("edition bump, pin s3s to a crates.io release or an internal
mirror, rustls migration"); this ADR does not cover it.

## Migration rule

After this ADR lands, the rule for reviewers is:

- **New code we own:** no `async_trait`. Native `async fn` in traits.
  If `dyn` compatibility is needed, use RTN or Box-returning adapter
  methods; document why.
- **New code that implements an s3s trait:** use `async_trait`. That
  is a s3s constraint, not ours, and documenting it as a policy
  exception here keeps the trail clear.
- **Existing s3s impls:** untouched until PRD-006 upgrades s3s.

## Consequences

### Positive

- One fewer procedural macro in the `cas-storage` build graph.
  Faster cold builds; smaller dependency diff on `cargo tree`.
- Honest code: the `AsyncFileSystem` file stops pretending to be
  async. A reader can see at a glance that it is a sync seam for
  blocking `std::fs` calls on a tokio worker (which is what the
  deadlock postmortem documents).
- The testing mock planned in PRD-001 (`InMemoryFs` in
  `async_fs.rs`) does not need to carry the macro either.

### Negative

- Minor inconsistency with the s3s-facing code: `async_trait` appears
  in `s3-cas/` but not in `cas-storage/`. This ADR exists partly to
  document that this split is deliberate, not accidental.

### Neutral

- No runtime impact. `async_trait` desugars to `Pin<Box<dyn Future>>`;
  removing it from a trait whose methods are already synchronous is
  purely cosmetic for the binary.
- No public API change. `AsyncFileSystem` is crate-private.

## Alternatives considered

1. **Keep `async_trait` everywhere** for uniformity. Rejected --
   uniformity is not a virtue when one of the uses is dead and the
   other is externally forced. Keeping both lets the wrong reading
   ("we still use async_trait because it matters") persist.
2. **Drop `async_trait` from the s3s impls too**, by manual future
   boxing. Rejected -- we would reimplement the macro by hand.
3. **Block on s3s upgrade** before doing anything. Rejected -- the
   `AsyncFileSystem` cleanup is independent of s3s and costs nothing.

## Verification

- `cargo build --workspace` succeeds after the cleanup.
- `cargo tree -p cas-storage | grep async-trait` returns nothing.
- `cargo tree -p s3-cas | grep async-trait` still returns a line
  (because s3s depends on it transitively via s3-cas's impls).
- All tests pass unchanged.

## References

- Rust RFC 3425: return-type notation for `dyn` compatibility.
- `async_trait` crate docs: explains what the macro generates and
  when it is still needed.
- `docs/arch/deadlock-fix.md`: section "a. `AsyncFileSystem` is no
  longer async" -- the commit that made the macro dead code.
