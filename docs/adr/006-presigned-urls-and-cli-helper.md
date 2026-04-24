# ADR 006: Presigned URLs as first-class support via s3s SigV4; optional CLI helper

Status:     Proposed - 2026-04-24
Author:     Jan De Landtsheer
Related:    docs/adr/005-upgrade-s3s-before-new-work.md
            docs/prd/prd000-current-state-and-restructure.md
            docs/prd/prd002-s3s-edition-tls.md

## Context

A recurring question: "can a user hand out a time-limited URL that
lets someone download an object from a bucket without having s3
credentials themselves?" That is exactly what AWS presigned URLs do,
and it is how third parties typically share S3-backed artefacts.

A presigned URL is not a new protocol. It is a SigV4 signature
(`X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`, `X-Amz-Expires`,
`X-Amz-SignedHeaders`, `X-Amz-Signature`) encoded as query parameters
instead of as an `Authorization` header. The server verifies the
signature the same way, checks `X-Amz-Expires` against the request
arrival time, and serves the object if both match. No server-side
state is needed -- the signature is computed offline by the user from
their own access_key/secret_key.

Today in s3-cas:

- `s3s` v0.11.1 handles SigV4 on both the header and query-string
  paths (that is inherent to the crate; our code does not ride a
  separate signing path).
- `DynamicS3Auth::get_secret_key` (`s3-cas/src/s3_wrapper.rs:25`) is
  what s3s calls during signature verification. It works identically
  whether the signature came in as a header or as query parameters,
  so presigned URLs already work end to end against s3-cas.
- `aws-sigv4` v1.4.3 is already in the workspace transitively via
  `aws-sdk-s3`, so any first-party CLI helper that generates presigns
  adds zero external dependencies.

What is missing is not functionality. It is:

1. An explicit statement that presigned URLs are supported, so that
   neither users nor future contributors reinvent a parallel
   "share-link" mechanism.
2. A convenience CLI subcommand so admins do not need the AWS CLI or
   `boto3` installed on the host to generate a URL.

## Decision

**Treat presigned URLs as a first-class, already-shipped feature of
s3-cas.** Document it. Add an optional CLI helper. Do not add any
server-side signing state, expiration store, or custom URL format.

Scope of this ADR:

- **A.** Update `README.md` and `docs/arch/` with a short note:
  presigned URLs work as defined by the AWS S3 SigV4 specification;
  every standard S3 client generates them; `X-Amz-Expires` is the
  authoritative expiry mechanism.
- **B.** Add `s3-cas presign` as a top-level CLI subcommand. It takes
  a user_id (to look up credentials from the local `_USERS` tree), a
  bucket and key, an HTTP method (`GET` by default), and a
  `--ttl <duration>` (default 1h, max 7 days per SigV4 spec). Outputs
  one URL to stdout.
- **C.** Explicitly decline any server-side "share link" feature
  (random tokens, per-link revocation list, stored expirations,
  quotas). If and when that becomes necessary, it is a separate PRD;
  it is not in the SigV4 model.

Open-loop item handed off to the HTTP-exposure work
(`docs/adr/007-http-exposure-options.md`): presigned URLs and
bucket-level public-read are orthogonal mechanisms. Keep them that
way; do not collapse one into the other.

## Scope

In scope:

- Implementation of `s3-cas presign` subcommand (Rust, ~80-120 LOC,
  uses `aws-sigv4::http_request::sign`).
- Documentation of the feature.

Out of scope:

- Server-side signature generation endpoint (e.g. a REST endpoint
  that accepts a user cookie and emits a presigned URL). That is a
  UI feature, gated by a future UI PRD.
- Custom short-link format. A SigV4 URL is long; that is a cosmetic
  complaint, not an architectural concern, and every S3 client on the
  planet handles them.
- POST presigned forms (policy-based uploads). Separate mechanism; add
  only if there is a concrete consumer.
- Bucket / object ACLs. See ADR-007 for the relationship.

## Target CLI shape (informative)

```
$ s3-cas presign --meta-root /var/s3-cas \
      --user delandtj GET my-bucket path/to/file.tgz --ttl 15m
http://s3-cas.internal/my-bucket/path/to/file.tgz?X-Amz-Algorithm=AWS4-HMAC-SHA256&...
```

Flags:

- `--user <user_id>`       Required. Looks up the user's
                           s3_access_key and s3_secret_key from the
                           same `_USERS` partition the server uses.
- `--endpoint <url>`       Base endpoint URL to sign against.
                           Defaults to `http://localhost:<s3-port>`
                           (read from server config or a flag).
- `--region <name>`        Defaults to `us-east-1` (what s3s
                           accepts by default).
- `--ttl <duration>`       Defaults to 1h. Maximum 7d (SigV4 limit).
- `--method <GET|PUT|...>` Defaults to GET.

Error modes:

- Unknown user -> non-zero exit, message on stderr.
- TTL > 7d     -> non-zero exit, print the SigV4 7-day ceiling.
- Any other failure bubbles up as anyhow error text on stderr.

The subcommand is read-only against the meta store; it never opens
Fjall for writes.

## Invariants (must not be broken)

- No new persistent state in Fjall for presigned URLs. The `_USERS`
  tree is the only thing the subcommand reads.
- No change to `DynamicS3Auth` or the s3s glue. Presigned verification
  is s3s's job; we do not own that path.
- Presigned URLs do not bypass user isolation. A URL signed with
  user A's credentials can only address objects under A's namespace,
  which already follows from `S3UserRouter` using the `access_key` in
  the signature to pick the `CasFS` instance.

## Consequences

### Positive

- Users can hand out time-limited links with zero server-side config.
- The CLI helper removes the need for Python / AWS CLI / boto3 on
  the server machine just to produce a URL.
- Keeps the server stateless with respect to link lifecycle. Expiry
  is enforced by signature validity, not by a revocation list that
  would need a cleanup task.

### Negative

- Presigned URLs are **not revocable** before their expiry, except by
  rotating the user's secret key (which invalidates every URL signed
  with it, not just the one you wanted to kill). If "revoke a
  specific link" is a real requirement, that is a share-link PRD and
  goes in a different direction than this ADR.
- Expiry is client-controlled. A compromised client can mint URLs
  with max TTL (7d). Mitigated only by rotating the user's secret
  key.

### Neutral

- Relationship to ADR-007 (HTTP exposure): both answer "how do I give
  someone a URL to an object". ADR-007 talks about bucket-level public
  read for static content; this ADR talks about user-initiated,
  time-limited, per-object sharing. They coexist -- a bucket can be
  fully private and still have individual objects shared via
  presigned URLs.

## Verification

Success for this ADR is reached when:

- `s3-cas presign ...` produces a URL that a plain `curl` can fetch
  within the TTL and that returns 403 (`SignatureDoesNotMatch` or
  `AccessDenied`) after expiry.
- Integration test: `it_s3.rs` gets a `test_presigned_get` case that
  exercises the helper against the local server.
- No server-side code change lands; `git log -- s3-cas/src/s3fs.rs
  s3-cas/src/s3_wrapper.rs` is empty for this feature.

## Open questions

- **Endpoint URL source.** Does the CLI read the server's configured
  listen address from a shared config file, or require the user to
  pass `--endpoint` explicitly? First pass: require `--endpoint`. If
  that turns out to be painful, add a lookup later.
- **POST-form presigns.** Defer until someone asks.
- **Generated-URL display.** One URL per stdout line; callers can
  pipe. No JSON mode in v1.

## References

- AWS SigV4 spec: presigned URLs (query string request authentication).
- `aws-sigv4` crate docs.
- s3s v0.11.1 signature verification path (inherent to the crate).
- `s3-cas/src/s3_wrapper.rs::DynamicS3Auth` -- the piece s3s calls.
