# docs/ index

One line per document, with status. Use this as the map. Last updated
2026-04-24.

## Anchor

- [prd/prd000-current-state-and-restructure.md](prd/prd000-current-state-and-restructure.md)
  -- baseline description of today's repo; lists candidate spin-out PRDs.

## Active PRDs

| ID      | Title                                                          | Status   |
| ------- | -------------------------------------------------------------- | -------- |
| PRD-000 | Current-state baseline and restructure                         | anchor   |
| PRD-001 | cas-storage internal restructure (split fs.rs, unify CasFS)    | proposed |
| PRD-006 | s3s upgrade, Rust edition bump, TLS stack consolidation        | draft    |

Candidates named in PRD-000 section 8 but not yet written:

- PRD-003 remove `rusoto_core::ByteStream` from the library surface
- PRD-005 per-user metrics and quotas
- PRD-008 expand integration tests and bring benches into CI

(PRD-002, PRD-004, PRD-007 were collapsed into the simplify pass and
will not be written as standalone PRDs.)

## Active ADRs

| ID      | Title                                                       | Status                                              |
| ------- | ----------------------------------------------------------- | --------------------------------------------------- |
| ADR-002 | Migration from OpenSSL to rustls                            | proposed, partial progress                          |
| ADR-004 | Drop `async_trait` where we own the trait                   | proposed                                            |
| ADR-005 | Upgrade `s3s` off `async_trait` before new S3-adjacent work | proposed - **prerequisite for S3-surface features** |

## Architecture notes

These describe load-bearing behaviour or invariants; any change to the
code in their area must preserve what they document.

- [arch/refcount.md](arch/refcount.md) -- the refcount invariant and
  the "data leakage OK, data loss never" rule.
- [arch/deadlock-fix.md](arch/deadlock-fix.md) -- the Fjall deadlock
  postmortem, the fix, and guardrails.
- [arch/multipart-trace.md](arch/multipart-trace.md) -- trace through
  the multipart upload path.

## Historical

Frozen snapshots. Useful as context, not as current guidance.

- [historical/adr/001-multi-user-authentication.md](historical/adr/001-multi-user-authentication.md)
  -- accepted 2025-11-17; HTTP UI portion superseded by simplify branch.
- [historical/adr/003-cas-storage-library-extraction.md](historical/adr/003-cas-storage-library-extraction.md)
  -- accepted and implemented; library / application split is in place.
- [historical/cas-storage-library-api.md](historical/cas-storage-library-api.md)
  -- manual API summary; superseded by `cargo doc` output from the
  rustdoc comments in `cas-storage/src/lib.rs`.
- [historical/multi-user-prd.md](historical/multi-user-prd.md) -- the
  pre-DB-backed multi-user PRD; replaced by the current implementation.
- [historical/IMPLEMENTATION_STATUS.md](historical/IMPLEMENTATION_STATUS.md)
  -- a snapshot from the multi-user work; the integration it tracks
  is long since complete.

## Conventions

- PRDs and ADRs are numbered and never renumbered.
- Superseded documents move to `docs/historical/` with a status header
  pointing at the successor.
- Unicode glyphs are avoided in source files per repo convention (plain
  ASCII, no em dashes / curly quotes / ellipsis / arrows). Historical
  documents predate this rule and are not reformatted.
