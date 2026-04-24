# PRD-006: `s3s` upgrade, Rust edition bump, TLS stack consolidation

Status:      Draft
Author:      Jan De Landtsheer
Date:        2026-04-24
Executes:    docs/adr/005-upgrade-s3s-before-new-work.md (prerequisite)
             docs/adr/002-rustls-migration.md              (partial)
Related:     docs/prd/prd000-current-state-and-restructure.md (parent)
             docs/prd/prd001-cas-storage-internal-restructure.md
             docs/adr/004-drop-async-trait.md

## 1. Purpose

Execute the three dependency-surface changes PRD-000 bundled under this
slot: drop `async_trait` from the `s3s` glue, move the workspace to a
current Rust edition, and pick one TLS stack for the whole binary. The
three are grouped because they share a common risk profile (touching
Cargo.toml across the workspace) and because at least one of them
(s3s upgrade) gates feature work under ADR-005. They are specified
below as three independent deliverables; landing them in any order is
fine.

## 2. Scope

In scope:

- **Deliverable A** -- unblock ADR-005: `s3s` no longer requires
  `async_trait` in our code.
- **Deliverable B** -- `[workspace.package] edition` moves from `2018`
  to the current stable edition.
- **Deliverable C** -- one TLS stack across the whole binary, with a
  documented decision for or against rustls. Closes ADR-002.

Out of scope (has its own PRD or later work):

- PRD-001 (cas-storage internal split) is independent.
- PRD-003 (remove `rusoto_core::ByteStream` from the library) is
  independent. If it lands first, Deliverable C's scope shrinks --
  see section 5.4.
- Any user-visible feature change.
- Any on-disk format change.

## 3. Invariants (must not be broken)

- 22/22 tests pass on every milestone.
- No change to the public API exported from `cas-storage/src/lib.rs`
  beyond what is forced by the edition bump.
- `cargo build` on a stock stable toolchain (no nightly).
- No regression in `cargo tree | wc -l`. The three deliverables taken
  together should *shrink* the tree, not grow it.

## 4. Deliverable A: `s3s` off `async_trait`

### 4.1 Target state

- `s3-cas/Cargo.toml` no longer depends on `async-trait`.
- `s3fs.rs`, `s3_wrapper.rs`, and `metrics.rs` use plain
  `async fn` in their `impl s3s::S3 for ...` blocks -- no
  `#[async_trait]` attribute, no `Pin<Box<dyn Future>>` hand-roll.
- `metric_fwd!` and `route_fwd!` macros shrink to the obvious form:

  ```rust
  macro_rules! metric_fwd {
      ($method:ident, $input:ty, $output:ty) => {
          async fn $method(
              &self,
              req: S3Request<$input>,
          ) -> S3Result<S3Response<$output>> {
              self.metrics.add_method_call(stringify!($method));
              self.storage.$method(req).await
          }
      };
  }
  ```

  i.e. what commit `7953e35` wanted to write before E0195 forced the
  hand-roll.
- `cargo tree -p s3-cas -e normal | grep async-trait` returns nothing
  (modulo a transitive path via `rusoto_core`, which is PRD-003's
  concern; see section 5.3).

### 4.2 Acquisition paths

Three options, listed in ADR-005 in descending preference:

- **A1** -- adopt upstream `Nugine/s3s` once it migrates to native
  AFIT. Pin to the first tag that drops `#[async_trait]` on the
  `S3` and `S3Auth` trait definitions.
- **A2** -- fork `s3s` into `third_party/s3s` in our workspace,
  strip `#[async_trait]` from the trait surface, maintain against
  upstream. Carries indefinite maintenance.
- **A3** -- replace `s3s` with another S3 crate. Largest scope;
  only if A1 and A2 are both infeasible.

Deliverable A is "done" regardless of which path is taken, as long as
the target state in 4.1 holds.

### 4.3 Verification

- `cargo build --workspace` passes.
- `cargo test --workspace` passes with the same test count.
- `cargo tree -p s3-cas -e normal | grep async-trait` is empty.
- The integration test `tests/it_s3.rs` passes against the new `s3s`
  (this is the compat canary).
- ADR-005's "When executed, success looks like" section is satisfied.

### 4.4 Owner and deadline

Not assigned. ADR-005 sets 2026-07-24 as the fallback point:
if upstream `s3s` shows no AFIT migration by then, fall through to
option A2.

## 5. Deliverable B: Rust edition bump

### 5.1 Target state

`Cargo.toml` at the workspace root:

```toml
[workspace.package]
edition = "2024"
```

(Or whichever edition is stable on the toolchain at execution time;
see section 5.5 below.) Both crates inherit via `edition.workspace = true`.

### 5.2 Expected breakage

Edition 2018 -> 2024 brings:

- 2021: closures capture disjoint fields (breaking), `IntoIterator` for
  arrays, or-patterns in macro fragments, reserved `#[` on macro RHS,
  panic macro consistency.
- 2024: `static mut` references require `&raw`, lifetime capture rule
  changes (`impl Trait` captures fewer lifetimes by default, fix with
  `+ use<...>`), unsafe-op-in-unsafe-fn becomes default, `gen` keyword
  reserved, async closures stabilised.

The breakage surface in this codebase is likely small -- no `static
mut`, no extensive closure gymnastics, no macro authoring with those
fragments. Expect a few `use<...>` annotations on `impl Trait` returns
at worst.

`cargo fix --edition` will handle most mechanical rewrites. The
residual manual work is what this deliverable records.

### 5.3 Verification

- `cargo build --workspace` passes on stable with the new edition.
- `cargo test --workspace` passes with the same test count.
- `cargo clippy --workspace --no-deps` produces no *new* warnings
  beyond the pre-existing set (the pre-existing warnings documented
  in the simplify-branch notes are acceptable baseline).

### 5.4 Interaction with ADR-004 and Deliverable A

Neither affected. ADR-004 only touched `cas-storage`'s direct
`async_trait` dep; the edition bump does not reintroduce it.
Deliverable A has its own Cargo.toml changes that are orthogonal to
the edition.

### 5.5 Open question

Do we target 2021 or the latest stable (2024) edition? 2021 has lower
breakage risk and is widely deployed; 2024 buys the newer language
features. Default answer: go for the latest stable at execution time
and accept the small cleanup cost. If breakage exceeds a half-day of
work, back off to 2021.

## 6. Deliverable C: TLS stack

### 6.1 Target state

One TLS implementation is present in the binary's transitive
dependency graph. `cargo tree -p s3-cas | grep -E 'openssl|native-tls|rustls'`
returns only one family (rustls).

### 6.2 Current state

After the `simplify/drop-ui-and-single-user` branch:

- The direct `openssl` optional dependency and the `vendored` feature
  are gone from `s3-cas/Cargo.toml`.
- `rustls` is already in the tree via `hyper-rustls` (pulled by
  `aws-sdk-s3` dev-dependencies) and via transitive S3 paths.
- `openssl` / `native-tls` are still pulled via `rusoto_core`
  (used for `ByteStream` in the write path) and via
  `hyper-tls` from `aws-config` / `aws-sdk-s3`.

### 6.3 Two execution paths

- **C1** -- switch `rusoto_core` to its `rustls` feature in
  `cas-storage/Cargo.toml` and drop `native-tls` features on
  `aws-config` and `aws-sdk-s3` in `s3-cas/[dev-dependencies]`.
  Smallest change. Keeps `rusoto_core::ByteStream` on the write
  path.
- **C2** -- **preferred if PRD-003 lands first** -- PRD-003 replaces
  `rusoto_core::ByteStream` with an internal stream abstraction,
  after which `rusoto_core` (and its native-tls path) can be
  dropped from `cas-storage` entirely. At that point Deliverable C
  becomes a one-line change in the dev-dependencies and this
  deliverable is mostly done for free.

If PRD-003 lands before Deliverable C is executed, take path C2.
Otherwise path C1.

### 6.4 Verification

- `cargo tree -p s3-cas -e normal | grep -E 'openssl|native-tls'`
  is empty.
- `cargo build --workspace` passes.
- `cargo test --workspace` passes, in particular the integration
  test that uses `aws-sdk-s3` against our server.
- ADR-002's "Verification Criteria" section is satisfied.

### 6.5 Consequences

After this deliverable lands, ADR-002 moves to `docs/historical/adr/`
with a "superseded -- done" status note.

## 7. Success criteria (all deliverables)

- Workspace `edition` on the current stable edition.
- `cargo tree -p s3-cas -e normal | grep -E 'async-trait|openssl|native-tls'`
  is empty.
- `metric_fwd!` and `route_fwd!` use the obvious `async fn` form; the
  explanatory doc comments about E0195 are deleted.
- `ADR-005` moves to `docs/historical/adr/`. ADR-002 moves to
  `docs/historical/adr/`.
- 22/22 tests pass; integration tests pass.

## 8. Risks

- **`s3s` upgrade forces non-trivial API changes.** The integration
  test is the first place this will surface. If request/response types
  reshape, the existing `s3fs.rs` implementation follows mechanically.
  Budget: half a day for Deliverable A's actual code work; most of the
  time is waiting for upstream (A1) or maintaining a fork (A2).
- **Edition bump surprises.** 2024's lifetime-capture change is the
  most likely tripwire. `cargo fix --edition` handles the common cases.
  Fall back to 2021 if 2024 exceeds a half-day of cleanup.
- **rusoto_core is unmaintained.** It builds today. If at any point
  it no longer builds against a current toolchain, C2 (drop rusoto
  entirely) becomes mandatory rather than preferred. Track upstream
  status when executing.

## 9. Non-goals

- Replacing `hyper` / `tokio` / `fjall` versions. Those stay as-is.
- Any feature change. This PRD is strictly dependency-surface work.
- Performance tuning. If measurements move, that is a signal something
  structural changed; revert rather than chase.

## 10. Open questions

- **Who watches upstream `s3s`?** ADR-005 noted this. Assign on
  execution.
- **Minimum supported Rust version.** This PRD implicitly sets it to
  whatever supports the chosen edition. Document explicitly in the
  workspace `Cargo.toml` once executed.
- **Fork location, if A2 is taken.** `third_party/s3s` inside this
  workspace, a sibling repo, or a separate workspace? Decide at
  execution time.
