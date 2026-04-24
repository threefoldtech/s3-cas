# ADR 007: HTTP exposure options for buckets and objects (path/filename access)

Status:     Proposed - 2026-04-24, refined 2026-04-25 - **option
            analysis; not a commitment. As of the 2026-04-25 refresh,
            no option is gated by ADR-005 anymore.**
Author:     Jan De Landtsheer
Related:    docs/adr/006-presigned-urls-and-cli-helper.md
            docs/adr/005-upgrade-s3s-before-new-work.md
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

**No ADR-005 gate.** The original 2026-04-24 draft described all
three options as gated by ADR-005's "upgrade s3s off async_trait"
prerequisite. ADR-005's second-look addendum (same-day 2026-04-25)
retired that gate entirely -- it turns out the fork plan can't
deliver what it claimed because of dyn-compat constraints in s3s
itself, and the hand-rolled macro cost is now accepted as a known
wart. All three options below are unblocked as of 2026-04-25. See
ADR-005's second-look for the full reasoning.

## What "expose over HTTP" can mean

Three distinct features get mashed into this one question. They are
independent; picking one does not force the others.

### Option A: Bucket-level public-read (minimal)

> "Mark this one bucket as publicly readable. Unsigned GET/HEAD on
> any key works. Everything else still needs auth."

Shape:

- `BucketMeta` (in `cas-storage/src/metastore/bucket_meta.rs`) gains
  a `public_read: Option<bool>` field (Option-wrapped so existing
  bincode blobs decode to `None` == private; new writes can set
  `Some(true)`). Migration-safe.
- **Preferred integration point: `S3Access` hook, not a wrapper.**
  s3s (including v0.11.1) has `s3s::access::S3Access` -- a
  pre-dispatch authorization trait with a small surface (primary
  method `check(&mut S3AccessContext)`). Implementing `S3Access`
  sidesteps the `metric_fwd!` / `route_fwd!` macro problem entirely
  because it is not a 16-method `impl s3s::S3` wrapper. ADR-005's
  second-look surfaced this as the right shape; ADR-007 adopts it.
  The alternative -- adding a third branch to `S3UserRouter`'s
  wrapper -- works but re-pays the hand-rolled macro cost and should
  only be considered if `S3Access` turns out to lack necessary
  request context (unlikely).
- `S3Access::check` implementation: if the request has no valid
  signature, look up the bucket from the path, read `BucketMeta`,
  allow only when `public_read == Some(true)` and the op is in the
  anon-allowlist (see the "What public-read covers" table below).
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
| works without opening a second port     | yes      | yes(*)   | no       |
| touches the S3-surface trait            | no(*)    | yes      | no       |

(*) Option A lands as an `S3Access` impl (a sibling hook trait), not
    a wrapping `impl s3s::S3`. Technically still "the s3s surface",
    but it does not replicate the 16-method surface, so the
    hand-rolled macro cost stays zero.
(*) Option B same-port works via vhost-style host matching
    (`bucket.s3-website-<region>.host/...`); the "separate website
    port" alternative is also viable but not preferred.

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

### Neutral

- Option A rides the `S3Access` hook trait, which is a narrow
  single-method surface -- it does not pay the hand-rolled macro
  cost that `impl s3s::S3` wrappers do. This is an improvement over
  the 2026-04-24 framing, which assumed Option A meant another
  16-method wrapper.
- Option C is the lowest-dependency path (no s3s touch at all) and
  can start at any time.

## Open questions (any PRD that picks an option must answer)

Several of the original open questions were promoted to decisions
in the "Addendum 2026-04-25" section below. The ones that remain
here are genuinely PRD-level and depend on the specific use case.

- **How does a user learn an object is public?** Is there an
  `s3-cas bucket info` CLI? An API surface? A Web UI?
- **`Cache-Control` and `Last-Modified` header policy.** Option A
  reuses s3s's header shape. Option C gets to decide for itself.
  (ETag is decided; see addendum.)
- **Directory listings (Option C only).** Paging threshold,
  hierarchical folder view (split on `/`), per-page item count,
  style (nginx-autoindex vs custom). Minimum viable shape for v1
  is a PRD call.
- **Quotas and rate limiting.** A public bucket is a bandwidth
  risk. Is there a per-bucket egress quota? A rate limit on anon
  access? Out of scope for this ADR; PRDs picking A or C must at
  least mention it.
- **CORS.** Public buckets serving web assets need
  `Access-Control-Allow-Origin` (blanket, per-origin, per-bucket
  config, ...). Explicitly out of scope for v1 of any option;
  follow-up PRD if and when a browser-side consumer needs it.
- **Observability.** Per-public-bucket request counter, egress
  bytes, rate-limit rejections. Reuse existing `s3_cas` prometheus
  metrics; bucket name as a label. PRD should wire this in day
  one.

## References

- `s3-cas/src/s3_wrapper.rs::DynamicS3Auth` -- current default-deny
  behaviour.
- `s3-cas/src/s3fs.rs` -- the S3-surface trait impl, 16 methods.
- `cas-storage/src/cas/` (PRD-001 restructure landed 2026-04-24;
  PRD-001 sect 5/6/7 closed 2026-04-25) -- the library seam Option
  C would ride.
- AWS documentation on S3 static website hosting and the
  `WebsiteConfiguration` schema.
- `docs/adr/003-cas-storage-library-extraction.md` (historical) --
  the decision that made Option C cheap to reach.

## Addendum 2026-04-25: decisions promoted from open questions

Following a review pass, a handful of items previously listed as
open questions are decided here to unblock whichever PRD picks one
of the options. These are not option-specific where not marked;
they are ADR-level decisions that bind all three options.

### Bucket-name disambiguation under anonymous access (affects A and C)

**Decision: path-style URLs with global uniqueness enforced on
public buckets.**

Bucket URLs are `http://host/<bucket>/<key>`. Buckets are per-user
internally (owner-scoped), but a bucket flagged `public_read` must
have a globally unique name across all users. The uniqueness check
runs at flip time (`s3-cas bucket set-public`): refuse to flip a
bucket public if another user already has a public bucket with the
same name. A user can rename a bucket (via delete + re-create,
since we do not have rename) to resolve a collision.

Rejected alternatives:

- URL-namespace per user (`host/u/<user_id>/bucket/key`): uglier
  URLs, leaks user_ids into every public URL. First-come-first-
  served global uniqueness is simpler and matches AWS convention.
- Vhost style (`bucket.host/key`): requires DNS control that
  self-hosters may not have; leaks bucket names into DNS. Pure
  path-style is more deployable.

### What public-read covers (affects A)

Table below binds which operations an anon-authenticated caller
can execute against a `public_read` bucket. Everything not in the
allowlist is rejected with 403 AccessDenied regardless of the
bucket flag.

| S3 op                           | allowed anon? | notes                                           |
| ------------------------------- | ------------- | ----------------------------------------------- |
| GetObject                       | yes           | the primary use case                            |
| HeadObject                      | yes           | needed for conditional GET / ETag probes        |
| GetObject with Range            | yes           | load-bearing for large objects                  |
| GetBucketLocation               | yes           | trivial, always safe                            |
| HeadBucket                      | yes           | existence probe; matches GetBucketLocation      |
| ListObjectsV2 / ListObjects     | **no** by default | opt-in separate `public_list: Option<bool>` flag on BucketMeta if browsing is wanted |
| GetBucketAcl                    | no            | info leak                                       |
| everything else                 | no            | PutObject, DeleteObject, CompleteMultipart, ... |

`public_list` is intentionally a second flag rather than folded
into `public_read`. Most public-asset hosting (CI artefacts, static
sites) does not want anon browsing of the bucket.

### Error shape on anon access (affects A)

**Decision: 403 AccessDenied for all denied anon access, regardless
of whether the bucket exists. Do not distinguish "bucket does not
exist" from "bucket exists but not public" in the response.**

Matches AWS S3's observed behaviour and prevents bucket-existence
enumeration via 404-vs-403 distinction. Specifically:

- Anon GET `/nonexistent-bucket/key` -> 403 AccessDenied
- Anon GET `/private-bucket/key`     -> 403 AccessDenied
- Anon GET `/public-bucket/missing`  -> 404 NoSuchKey
- Anon GET `/public-bucket/present`  -> 200 with body

The 404 vs 403 distinction only appears once the caller has proven
they can see the bucket (i.e., it is public). Bucket existence is
not enumerable via the anon endpoint.

### Option C v1 auth model

**Decision: Option C v1 serves public-read buckets only and
requires Option A to be implemented first.**

No cookie auth, no bearer tokens, no embedded login for v1. If a
logged-in browsing experience becomes necessary, it is a follow-up
PRD. This keeps Option C's v1 scope tight: it is a thin HTTP
adapter over the public-read subset of `cas-storage`'s read API.

Composition with Option A: Option C reads `BucketMeta.public_read`
via the same `cas_storage::MetaStore` accessors the server uses
(`get_bucket_ext`, `list_buckets`); no new library API is needed.
Whether Option C opens its own `MetaStore` or reuses the server's
is a PRD call (shared-process vs sidecar deploy shape).

### ETag shape (affects A and C)

**Decision: `ETag: "<hex content_hash>"` for single-part objects.
Multipart objects get `ETag: "<hex md5_of_part_md5s>-<N>"` per S3
convention.**

`content_hash` is already MD5 on every object in `cas-storage`
(see `Object::hash()`); the header is a format-and-quote away.
No new state or computation needed. Matches S3 clients' expectations
for conditional requests.

### Schema-migration plan for `BucketMeta` (affects A)

**Decision: new field on `BucketMeta` is added as
`public_read: Option<bool>` (and later `public_list: Option<bool>`
when that lands). Bincode-decode of a pre-migration record yields
`None` for the new field, which reads as "private" at every access
site. No on-disk format break; no migration step required.**

This constrains the PRD that lands Option A to write
`Option<bool>`, not `bool`, even though the flag is semantically
binary. The `Option` wrapper is the migration guarantee. If a
future PRD wants to collapse to `bool`, it must write a dedicated
migration.

### Preferred integration shape for Option A (affects A)

Already noted in the Option A section above, but captured here as
a decision: **Option A plugs into `s3s::access::S3Access`, not a
wrapping `impl s3s::S3`.** `S3Access` is a narrow, single-primary-
method trait that lives alongside `S3` in s3s; implementing it does
not pay the hand-rolled `Pin<Box<dyn Future>>` macro cost. If
`S3Access` turns out to lack request context the PRD needs
(unlikely but possible), falling back to a third branch in
`S3UserRouter` is acceptable; the macro cost is then one per
wrapper method, paid once.
