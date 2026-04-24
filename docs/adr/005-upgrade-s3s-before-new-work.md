# ADR 005: Upgrade `s3s` off `async_trait` before new S3-adjacent work

Status:      Accepted - 2026-04-25 - **second-look retired the
             blockade; see "Second-look addendum 2026-04-25" at the
             bottom. S3-surface feature work is NOT blocked by this
             ADR anymore.**
Author:      Jan De Landtsheer
Related:     docs/adr/004-drop-async-trait.md
             docs/prd/prd000-current-state-and-restructure.md (PRD-002)
             commit 7953e35 (the hand-rolled macro that motivates this)

## Context

In `7953e35` we collapsed 32 near-identical method wrappers (16 in
`MetricFs`, 16 in `S3UserRouter`) into two local `macro_rules!`
invocations. The refactor deleted ~160 lines, but it had to work
around an interop problem between proc-macro attributes and
declarative macros:

- `s3s::S3` is defined upstream with `#[async_trait::async_trait]`.
- Our impls therefore have to use `#[async_trait]` too.
- `macro_rules!` inside an `#[async_trait]`-annotated impl **does not
  work** with the obvious `async fn $method(...)` form. The attribute
  macro expands over raw tokens first and never sees the methods that
  `macro_rules!` would generate. Result: `error[E0195] lifetime
  parameters or bounds on method ... do not match the trait
  declaration` for every expanded method.
- The workaround is to emit, from `macro_rules!`, the exact hand-rolled
  form that `async_trait` would have produced:

  ```rust
  fn $method<'life0, 'async_trait>(
      &'life0 self,
      req: S3Request<$input>,
  ) -> ::core::pin::Pin<Box<
      dyn ::core::future::Future<Output = S3Result<S3Response<$output>>>
          + ::core::marker::Send + 'async_trait
  >>
  where 'life0: 'async_trait, Self: 'async_trait,
  {
      Box::pin(async move { /* body */ })
  }
  ```

That is the shape living in `s3-cas/src/metrics.rs` and
`s3-cas/src/s3_wrapper.rs` today. It compiles. It passes tests. It is
also hostile to anyone editing it later: the signature is specific to
what `async_trait` v0.1.x happens to emit today, and any upstream change
in `s3s` or `async_trait` internals risks silent drift.

The only reason we are in this situation is that `s3s` (the external
crate we depend on) still uses `async_trait`. Rust 1.75 stabilised
native `async fn` in traits; 1.82 added return-type notation for
`dyn`-compat. We compile on 1.95-class toolchains. Every piece of our
own code could be on native AFIT -- except the S3 glue, which is
forced to follow upstream.

## Decision

**Before any new S3-adjacent feature work, upgrade off `s3s` with
`async_trait` to an `s3s` (or equivalent) with native `async fn` in
traits.** This ADR exists to make that prerequisite explicit rather
than letting the macro workaround rot in place.

Three options were considered, in descending original preference:

1. **Wait for and adopt upstream `s3s` native-AFIT.** Track
   `Nugine/s3s` for the migration; pin to the first tag that drops
   `async_trait` on the `S3` and `S3Auth` traits.
2. **Fork `s3s` locally and strip `async_trait` ourselves.** The fork
   stays in our workspace and we carry the maintenance burden.
3. **Replace `s3s` with another S3 crate entirely.** Only if (1) and
   (2) are both infeasible. Most invasive; tracked solely as an escape
   hatch.

**Chosen (see Addendum below, 2026-04-25): option 2.** Option 1 is
not dead long-term but is on its own clock; option 2 is executed in
parallel so feature work is not gated on an external contributor.

The actual code change on our side, after the upgrade, is subtractive:

- Delete `async_trait` from `s3-cas/Cargo.toml`.
- Delete `use async_trait::async_trait` from `s3fs.rs` and
  `s3_wrapper.rs`.
- Rewrite `metric_fwd!` and `route_fwd!` to the obvious `async fn`
  form -- the E0195 error class disappears as soon as the trait is
  not `#[async_trait]`-annotated upstream.
- Drop the large doc comment on both macros that explains the
  workaround.

Estimated after-upgrade delta in our tree: ~30 more lines removed,
plus the mental-overhead savings of not having hand-rolled
`Pin<Box<Future>>` signatures in the repo.

## Scope: what "new territory" means for this ADR

**Blocked by this ADR** (do NOT start these before the upgrade):

- New S3 protocol features (new methods, new request/response types,
  anything that extends the s3s-facing surface).
- New wrappers around `s3s::S3` or `s3s::auth::S3Auth` beyond the
  existing two. Every new wrapper would re-pay the hand-rolled
  `Pin<Box<Future>>` cost.
- The per-user metrics and quotas PRD (not yet written) if its
  implementation plan involves another `MetricFs`-shaped wrapper.
- Any change that touches `s3fs.rs`, `s3_wrapper.rs`, or
  `s3-cas/src/metrics.rs`'s `MetricFs` impl beyond bug fixes.

**Not blocked by this ADR** (proceed freely):

- PRD-001 (cas-storage internal restructure). It does not touch the
  s3s surface.
- Any work inside `cas-storage/`. `cas-storage` already has zero
  direct `async_trait` dependency after ADR-004; the only remaining
  transitive path is through `rusoto_core::ByteStream`, which is the
  ByteStream-removal PRD's concern (not yet written).
- Docs, ADRs, dependency bumps that do not affect the S3 glue.
- Bug fixes inside the existing S3 wrappers.

## Consequences

### Positive

- Once executed, the hand-rolled `Pin<Box<Future>>` macro goes away and
  `async_trait` leaves the workspace entirely. ADR-004 becomes fully
  satisfied (today it is only partially: `cas-storage` is clean but
  `s3-cas` still carries `async_trait` because of `s3s`).
- Future S3-adjacent refactors -- including any new wrapper -- can use
  the obvious `async fn` form in `macro_rules!`.
- Removes a class of silent breakage on `async_trait` internal
  changes.

### Negative

- Introduces a scheduling dependency: new S3 feature work waits on
  an upstream (option 1), a fork (option 2), or a replacement
  (option 3). If none materialise, this ADR becomes a bottleneck.
- Whichever option we take, the upgrade itself will have to prove
  API compatibility with what `s3fs.rs` and `s3_wrapper.rs` rely on
  today. That is a non-trivial upgrade test pass, not a no-op.

### Neutral

- Relationship to PRD-002: PRD-002 is the implementation plan that
  executes this ADR's Deliverable A (s3s off async_trait) alongside
  the edition bump and TLS stack consolidation. This ADR sets the
  policy; PRD-002 does the work.
- Relationship to ADR-002 (rustls): the s3s upgrade may or may not
  come with a TLS stack change. Handle them independently.

## Verification

When executed, success looks like:

- `cargo tree -p s3-cas -e normal | grep async-trait` returns nothing.
- `s3-cas/src/metrics.rs::metric_fwd!` and
  `s3-cas/src/s3_wrapper.rs::route_fwd!` use the plain
  `async fn $method(...)` form.
- All existing tests pass unchanged.
- Line count of `s3-cas/src/metrics.rs` and `s3-cas/src/s3_wrapper.rs`
  drops by ~15 lines each (macro body shrinks).

## Open questions

- **How long do we wait for upstream?** If `Nugine/s3s` has no
  AFIT-migration signal on `main` three months from this ADR
  (by 2026-07-24), fall back to option 2.
- **Who watches upstream?** This ADR does not name an owner.
  Assign on execution.

## References

- `docs/adr/004-drop-async-trait.md` -- the partial-progress parent;
  explains which traits we own vs do not.
- `7953e35` -- the commit that introduces the hand-rolled macro and
  thereby creates the motivation for this ADR.
- `async_trait` crate docs -- the specific expansion this ADR's
  workaround mimics.

## Addendum 2026-04-25: upstream status check and path chosen

### What we verified

- `s3s` v0.13.0 (current crates.io release) **still** annotates both
  `S3` and `S3Auth` with `#[async_trait::async_trait]`. Verified
  directly against the tag on `Nugine/s3s`:
  - `crates/s3s/src/s3_trait.rs` line 9 (auto-generated by
    `s3s_codegen::v1::s3_trait::codegen`).
  - `crates/s3s/src/auth/mod.rs` trait declaration.
- The trait attribute is emitted by the codegen template, not hand
  written -- meaning the decision lives in one place upstream, but
  also that any release flipping it is a deliberate template change,
  not a drive-by.
- No open or closed issue on `Nugine/s3s` mentions AFIT / native
  `async fn` in trait migration. Zero upstream signal as of
  2026-04-25.

### Decision

**Execute option 2 now.** Fork `Nugine/s3s`, patch
`s3s_codegen::v1::s3_trait::codegen` (and the auth-module equivalent)
to emit native `async fn` instead of the `#[async_trait]` attribute,
maintain the fork in the workspace until upstream catches up.
Concretely:

- Fork location: `github.com:delandtj/s3s` (created 2026-04-25,
  default branch `main`, empty of local patches as of this ADR).
  The patch work will land on a named branch (e.g. `afit`) so main
  can track upstream.
- Pin shape: `s3-cas/Cargo.toml` replaces the current
  `git = "https://github.com/Nugine/s3s", tag = "v0.11.1"` with a
  `git = "https://github.com/delandtj/s3s", branch = "afit"` (or a
  pinned rev once the patch stabilises).
- Scope of the local patch: template change only. No AWS-model
  regeneration, no behavioural change. The diff is mechanical.
- Rebase cadence: follow upstream tagged releases, not `main`.

Option 1 is demoted to "future outreach, not on this ADR's critical
path". Jan may hand-file a PR against `Nugine/s3s` later; if it
lands and a release ships with native-AFIT, the fork can be
abandoned in favour of crates.io. But the fork does not wait on
upstream and does not track an upstream PR timeline.

### What changes downstream of this decision

- **ADR-005 blockade is now time-boxed**, not open-ended. As soon as
  the fork compiles and the macros are rewritten to plain `async fn`,
  the "do NOT start these before the upgrade" list (section above)
  dissolves.
- PRD-002 Deliverable A gets a concrete owner and a concrete target:
  land the fork, rewrite `metric_fwd!` and `route_fwd!` to native
  `async fn`, drop `async_trait` from `s3-cas/Cargo.toml`.
- ADR-007 options A and B (bucket public-read, website hosting)
  become unblocked in the same PR sequence. Option C (first-party
  HTTP browser crate, new `http-cas`) is unaffected either way.

### Side finding (belongs in ADR-007, noted here to avoid losing it)

s3s issue #64 was closed by PR #65 upstream, introducing an
`S3ContextProvider`-style hook for per-request authorization
context. This may let ADR-007 Option A (bucket-level public-read)
plug into an existing hook instead of wrapping `S3UserRouter`.
Worth investigating when Option A becomes concrete; it could
eliminate one of the two macro-using wrappers entirely.

### Open loop

- **Upstream PR.** Optional follow-up; Jan may hand-file against
  `Nugine/s3s` after the fork is stable. Not on this ADR's critical
  path. Track here if and when filed.
- **Fork branch name.** `afit` proposed; create the branch and land
  the codegen template patch, then switch `s3-cas/Cargo.toml` over.

## Second-look addendum 2026-04-25 (same day)

Within hours of writing the above, while starting execution on the
fork patch, a blocking wrinkle surfaced. This addendum supersedes
the decision path the earlier addendum locked in.

### What we missed in the original ADR

The original claim -- "rewrite `metric_fwd!` and `route_fwd!` to the
obvious `async fn` form -- the E0195 error class disappears as soon
as the trait is not `#[async_trait]`-annotated upstream" -- is
**wrong** because of a dyn-compatibility constraint that is intrinsic
to s3s, not a per-release choice.

Verified by reading source on `Nugine/s3s` at tags `v0.11.1`,
`v0.13.0`, and branch `main` (version `0.14.0-dev`):

- `crates/s3s/src/service.rs` stores the impl as `Arc<dyn S3>`,
  `Box<dyn S3Auth>`, `Box<dyn S3Host>`, `Box<dyn S3Access>`,
  `Box<dyn S3Route>` -- and in 0.14.0-dev a new
  `Arc<dyn S3ConfigProvider>` joins the list. Dyn dispatch is load-
  bearing across the router.
- Native `async fn` in trait (stable since Rust 1.75) is **not
  dyn-compatible** today. Dyn-compat for async fn in trait is still
  experimental (needs RTN / nightly-only features).
- `#[async_trait::async_trait]` exists precisely to make such traits
  dyn-safe, by desugaring every `async fn` method into
  `fn foo(...) -> Pin<Box<dyn Future + Send + 'async_trait>>`.

Consequences for the three fork strategies:

1. **Drop `#[async_trait]` naively in the codegen template.** Breaks
   `Arc<dyn S3>`. s3s itself stops compiling. Non-starter.
2. **Keep the Pin-Box shape by hand-rolling it in the template.**
   The trait is still dyn-safe, but our `metric_fwd!` /
   `route_fwd!` impls still must emit the hand-rolled
   `Pin<Box<dyn Future>>` shape to match. Zero net win -- the
   hand-roll just moves between our repo and the fork.
3. **Restructure s3s to not use `dyn S3`.** Turn `Arc<dyn S3>` into
   `Arc<impl S3>` via generics through the service/router. Heavy
   change; leaks generics through the s3s public API. Upstream is
   moving in the opposite direction (0.14.0-dev *adds* another
   `dyn`-hook, `S3ConfigProvider`), so this is against the grain
   and not a tractable fork.

### Decision (replaces the earlier addendum's option 2 commitment)

**Lift the ADR-005 blockade. Accept the hand-rolled macros as
permanent, document them as a known wart, and let new S3-surface
feature work proceed.**

Concretely:

- The "Blocked by this ADR" list in the Scope section above is now
  historical -- nothing is blocked by this ADR anymore. New S3
  protocol methods, new wrappers around `s3s::S3`, per-user metrics,
  ADR-007 Options A and B, all unblocked as of 2026-04-25.
- `metric_fwd!` and `route_fwd!` stay as written. Their comment
  blocks (introduced in commit `7953e35`) already explain why; no
  code change required for this ADR.
- The `delandtj/s3s` fork (cloned locally, `afit` branch created
  from `v0.11.1`, zero commits on top) **is not pushed** and
  **does not land**. It stays as a vendoring parking spot the day
  we need to cherry-pick an upstream fix.
- `s3-cas/Cargo.toml` stays pinned to `Nugine/s3s` tag `v0.11.1`.

### What replaces "flip async_trait off" as the real architectural fix

The hand-rolled macros exist only because we chose to wrap
`s3s::S3` with `MetricFs` and `S3UserRouter`. The dyn-compat
constraint makes wrappers expensive; the way out is to stop
wrapping, not to patch the trait. Two independent moves, either of
which kills one wrapper + one macro:

- **2a -- merge per-user routing into `S3FS`.** `S3FS` learns to
  look up the per-user `CasFS` internally from a shared
  `UserStore`, keyed on the authenticated access key in the
  request. `S3UserRouter` goes away; `route_fwd!` goes away.
  Medium scope -- rewrites request entry in `s3fs.rs` and the
  bootstrap in `main.rs`.
- **2b -- move `MetricFs` to a `tower::Service` layer in front of
  s3s.** Metrics get tracked at the HTTP layer, keyed on op-name
  attribution that either re-parses the request or uses an s3s
  hook that exposes the resolved op. `MetricFs` goes away;
  `metric_fwd!` goes away. Smaller scope in terms of surface
  area but needs op-name attribution sorted.

Neither 2a nor 2b touches the `#[async_trait]` attribute on s3s's
traits -- they sidestep it by not implementing those traits a
second time.

Tracked as a candidate PRD ("retire `MetricFs` + `S3UserRouter`
via 2a+2b") in `docs/INDEX.md`. That PRD is the real closer for the
macro wart. It is not a prerequisite for anything; feature work
can proceed in parallel.

### Side finding re-affirmed (useful for ADR-007 Option A)

Even with the blockade gone, upstream's hook traits are still
architecturally cleaner than new `s3s::S3` wrappers:

- `S3Access` (with a general `check(&mut S3AccessContext)` called
  pre-dispatch) is the natural home for ADR-007 Option A's bucket
  public-read policy. Only one method needs the hand-rolled
  macro shape; no 16-method fan-out.
- `S3Route` handles side routes (health checks, custom endpoints)
  without touching the S3 trait at all.
- `S3ConfigProvider` (new in 0.14.0-dev) is not relevant here but
  worth knowing exists.

When ADR-007 Option A gets a PRD, prefer an `S3Access` impl over a
new `S3`-trait wrapper.

### Status transitions

- Original claim (first addendum, 2026-04-25 morning): accepted
  option 2 (local fork via codegen template patch).
- Second-look (this addendum, 2026-04-25 same day): option 2
  retired as infeasible given the dyn-compat analysis above;
  blockade lifted; fork demoted to vendoring parking spot; real
  architectural fix handed off to a new candidate PRD.

### Open loops replaced

The prior "Open loop" entries (upstream PR, fork branch name) are
no longer on this ADR's critical path. If Jan still wants to file
an upstream PR or land a vendoring branch later, those are
independent ops items, not ADR-005 deliverables.
