# ADR 007: HTTP exposure options for buckets and objects (path/filename access)

Status:     Proposed - 2026-04-24 - **option analysis; not a commitment**
Author:     Jan De Landtsheer
Related:    docs/adr/006-presigned-urls-and-cli-helper.md
            docs/adr/005-upgrade-s3s-before-new-work.md (PREREQUISITE)
            docs/adr/001-multi-user-authentication.md (historical)
            docs/prd/prd000-current-state-and-restructure.md

## Context

The server currently answers only signed S3 requests. `DynamicS3Auth`
(`s3-cas/src/s3_wrapper.rs:25`) rejects any request without a valid
access_key with `InvalidAccessKeyId`. `S3UserRouter` uses the
authenticated user to pick a per-user `CasFS` namespace. There is no
unauthenticated read path, no `PutBucketWebsite` / `GetBucketWebsite`
in our `s3s::S3` impl, no `public_read` flag on `BucketMeta`, and no
object ACLs.

Question behind this ADR:

> "Can a bucket be exposed over plain HTTP so a browser can fetch
> `http://host/bucket/dir/file.html` without an S3 client?"

The honest answer is "not today, and it splits into three very
different features depending on what you actually want." This ADR
lays out the option space so the eventual implementer (or the
person who writes the next PRD) is not starting cold.

**Prerequisite, not bypassable**: ADR-005 gates every S3-surface
change behind an `s3s` upgrade off `async_trait`. Options A, B, and
C below all touch the S3 surface or its routing layer. None of them
start until ADR-005 is executed (PRD-002 Deliverable A).

## What "expose over HTTP" can mean

Three distinct features get mashed into this one question. They are
independent; picking one does not force the others.

### Option A: Bucket-level public-read (minimal)

> "Mark this one bucket as publicly readable. Unsigned GET/HEAD on
> any key works. Everything else still needs auth."

Shape:

- `BucketMeta` (in `cas-storage/src/metastore/bucket_meta.rs`) gains
  a `public_read: bool` field.
- `S3UserRouter` learns a third path: when an incoming request has
  no signature and the method is `GET` or `HEAD`, resolve the
  bucket from the path, look up `BucketMeta`, and if `public_read`
  is set route the request to the bucket-owner's `CasFS`. Anything
  else -> reject as today.
- Admin CLI: `s3-cas bucket set-public <user> <bucket> [--public|--private]`.
- S3 surface: optionally expose `PutBucketAcl` / `GetBucketAcl` if
  AWS-CLI compatibility matters; otherwise the CLI is enough.

What this gets you:

- `curl http://host/public-bucket/path/file.html` works.
- Directory-listing pages do NOT work (there is no index.html
  resolution, no listing rendering). You get keys, not a browser
  experience.

Cost (rough):

- `cas-storage`:  field + bincode re-encode for `BucketMeta`
                  (schema break, needs a migration or a reset).
- `s3-cas`:       ~150-250 lines in `s3_wrapper.rs` + CLI.
- Tests:          one presigned-free `curl` GET case in `it_s3.rs`
                  or a sibling file.
- On-disk format: `BucketMeta` encoding changes. Plan for it with
                  a schema tag if existing data matters.

### Option B: S3-compatible static website (heavy)

> "Full `PutBucketWebsite` / `GetBucketWebsite` support with index
> document, error document, routing rules, and redirects."

Shape:

- Implement `put_bucket_website`, `get_bucket_website`,
  `delete_bucket_website` in `s3fs.rs`.
- Store a `WebsiteConfiguration` blob per bucket (new partition or
  new column in `BucketMeta`).
- Teach the HTTP layer about an entirely separate "website"
  endpoint pattern (`http://bucket.s3-website-<region>.host/...`)
  OR wire index-document resolution into the path-style GET
  handler.
- Implement index document (`index.html` appended when the URL ends
  in `/`), 404/403 redirection to the error document, and optionally
  `RoutingRules`.

What this gets you:

- Drop-in replacement for `aws s3 website` / S3 static site hosting.
- Works with tools that already know the AWS website-hosting URL
  convention.
- Still no browsable directory listings unless you add them as a
  non-AWS extension.

Cost (rough):

- `s3-cas`:     ~600-1000 lines new + a non-trivial URL-routing
                change (how does the server know a request is
                "website endpoint" vs "S3 endpoint"?). Could be a
                separate listener on a different port, or vhost-style
                host matching.
- Tests:        new integration tests for index/error resolution and
                404 behaviour.
- Docs:         user-facing docs on how to enable, how the index
                doc works, which SDK call configures it.

Only worth it if the consumer is a tool (CI, s3-website-aware CDN)
that expects AWS website semantics verbatim.

### Option C: First-party HTTP browser (new crate)

> "A separate HTTP service, not S3-protocol, that serves objects
> under `http://host/<bucket>/<key>` with optional
> directory-style listing pages, caching headers, and range
> requests. Reads from `cas-storage` directly."

Shape:

- New crate, e.g. `http-cas`, depending only on `cas-storage` +
  auth. Lives next to `s3-cas` in the workspace.
- Binds its own port; reads objects via the library's
  `BlockStream` and `RangeRequest` (already in `cas-storage/src/cas/`).
- Authentication: either "only serves buckets flagged
  `public_read`" (requires Option A first), or a cookie / bearer
  token scheme if logged-in browsing is wanted.
- Directory listing: render `BucketMeta::iter_all` as a simple
  HTML index. Optional.

What this gets you:

- Browsable buckets. Cache-friendly (ETag, conditional GET, partial
  content). No S3-protocol baggage.
- `Content-Type` guessing from key extension.
- Logical home for "drop this bucket behind nginx and forget about
  S3 clients".

Cost (rough):

- `http-cas`:    ~800-1500 lines for a meaningful first version
                 (axum or hyper, plus a templating layer for the
                 listing).
- `cas-storage`: no change. Public API already sufficient
                 (`get_object_paths`, `BlockStream`, `RangeRequest`).
- Docs:          a dedicated user-facing section.

This is the most strategically interesting option because it
decouples HTTP exposure from S3 protocol compat (ADR-003 already set
up `cas-storage` as a reusable library for exactly this kind of
adapter). The historical value is that a future WebDAV crate,
RESP-protocol crate, or gRPC crate slots into the same shape.

## Matrix

| want                                    | Option A | Option B | Option C |
| --------------------------------------- | -------- | -------- | -------- |
| `curl http://.../bucket/key` works      | yes      | yes      | yes      |
| browser sees `index.html` at `/`        | no       | yes      | yes      |
| browser sees a directory listing        | no       | no       | yes      |
| AWS-CLI `s3 website` compat             | no       | yes      | no       |
| works without opening a second port     | yes      | maybe    | no       |
| touches the S3-surface trait            | yes      | yes      | no       |
| gated by ADR-005                        | yes      | yes      | no       |

## Decision

This ADR **does not commit** to any of A/B/C. It is option
analysis. The decision it does make:

- A, B, and C are the three lanes. Do not invent a fourth without
  a PRD that justifies it.
- When a concrete need arrives, the chooser writes a PRD picking
  one and explicitly rejecting the other two. The PRD cites this
  ADR and shows why the picked option matches the driving use case.
- Presigned URLs (ADR-006) are orthogonal to all three. A user can
  share a single object from an otherwise-private bucket using
  SigV4; that does not require any of A/B/C.

Default recommendation if the driving use case is "host static
assets built by CI": Option A (minimal) + Option C (first-party
browser) combined. Option A gives the server-side flag; Option C
gives the browsable UX. Option B only earns its cost if an external
tool insists on AWS website semantics.

Default recommendation if the driving use case is "share one
document with one person for 10 minutes": none of A/B/C. Use
ADR-006's presigned URL mechanism.

## Invariants (regardless of which option lands)

- The refcount / transaction / commit-before-write invariants from
  PRD-000 sect. 6 are untouchable. HTTP exposure is a read-side
  feature; it touches neither.
- User isolation: a public bucket is public under its owner's
  namespace. There is no cross-user publishing of other users'
  keys, whatever option lands.
- No option weakens the default-deny behaviour of
  `DynamicS3Auth`. Public-read routing is a separate code path, not
  a hole in the auth verifier.

## Consequences

### Positive

- Having A/B/C named keeps the next conversation from relitigating
  the scope. Next person writes "I want X; it matches Option A" and
  the PRD shape is already known.
- Option C is the long-term healthy shape because it exercises the
  `cas-storage` library seam. Every time we add a non-S3 protocol
  adapter the library design gets validated (ADR-003 intent).

### Negative

- Three options means three ways to get it wrong. The PRD that
  picks one has to spend explicit effort rejecting the other two.
- Options A and B are both gated by ADR-005. If ADR-005 is stuck,
  all S3-surface-level HTTP exposure work is stuck with it.

### Neutral

- Option C can start the day after ADR-005 lands **or earlier**,
  since it does not touch the S3 surface. That makes it the
  lowest-dependency path if you want HTTP exposure fast.

## Open questions (any PRD that picks an option must answer)

- **Bucket naming in URLs.** Path style (`host/bucket/key`)
  vs vhost style (`bucket.host/key`). Path style is simpler; vhost
  style matches AWS convention more closely. Pick one.
- **How does a user learn an object is public?** Is there an
  `s3-cas bucket info` CLI? An API surface? A Web UI?
- **Caching headers.** Options A and B reuse s3s's header shape.
  Option C gets to decide `Cache-Control`, `ETag`, `Last-Modified`
  for itself.
- **Directory listings (Option C only).** Is the listing HTML
  part of v1 or a follow-up? What does it look like?
- **Quotas.** A public bucket is a bandwidth risk. Is there a
  per-bucket egress quota? A rate limit? Out of scope for this
  ADR, but any PRD that picks A or C should at least mention it.

## References

- `s3-cas/src/s3_wrapper.rs::DynamicS3Auth` -- current default-deny
  behaviour.
- `s3-cas/src/s3fs.rs` -- the S3-surface trait impl, 16 methods.
- `cas-storage/src/cas/` (PRD-001 restructured) -- the library seam
  Option C would ride.
- AWS documentation on S3 static website hosting and the
  `WebsiteConfiguration` schema.
- `docs/adr/003-cas-storage-library-extraction.md` (historical) --
  the decision that made Option C cheap to reach.
