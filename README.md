# S3-CAS

> Note: this is a continuation of Lee Smet's original
> [s3-cas](https://github.com/leesmet/s3-cas), the foundational work on
> content-addressable S3 storage.

## Features

- Content-addressable storage with automatic deduplication via MD5 hashing
- Reference counting -- blocks are deleted when no longer referenced
- Multi-user isolation with shared block-level dedup
- CLI-managed users (S3 access/secret credentials per user)
- Inline metadata for small objects
- Two Fjall backends: `fjall` (transactional) and `fjall_notx`

This build focuses on the S3 server plus the underlying CAS storage
library. The previous HTTP browser UI, admin panel, and single-user
mode have been removed on the `simplify/drop-ui-and-single-user`
branch to reduce surface area; see
[docs/prd/prd000-current-state-and-restructure.md](docs/prd/prd000-current-state-and-restructure.md)
for the rationale and roadmap.

## Building

```bash
git clone https://github.com/threefoldtech/s3-cas
cd s3-cas
cargo build --release
```

## Creating the first user

The server refuses to start against an empty user database. Create a
user first:

```bash
s3-cas user --meta-root /tmp/s3/meta add alice --admin
```

Output:

```
User 'alice' created (admin=true)
  access_key: XXXXXXXXXXXXXXXXXXXX
  secret_key: xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
  ui_password: xxxxxxxxxxxxxxxx
Save these credentials -- they will not be shown again.
```

Other subcommands:

```bash
s3-cas user --meta-root /tmp/s3/meta list
s3-cas user --meta-root /tmp/s3/meta reset-password alice
s3-cas user --meta-root /tmp/s3/meta delete alice
```

## Running the server

```bash
s3-cas server \
  --fs-root=/tmp/s3/fs \
  --meta-root=/tmp/s3/meta
```

Each user's objects are isolated; users cannot list or access each
other's buckets. Block-level dedup is global across users.

## Storage backends

- `fjall` (default) -- transactional LSM tree with ACID guarantees
- `fjall_notx` -- non-transactional, faster, not recommended for
  multi-user workloads

```bash
--metadata-db fjall
--metadata-db fjall_notx
```

## Durability

```bash
--durability buffer      # no fsync
--durability fdatasync   # default
--durability fsync       # sync data + metadata
```

## Inline metadata

Objects smaller than or equal to the configured threshold are stored
directly in their metadata record, avoiding a separate block file.

```bash
--inline-metadata-size 4096
```

Multipart uploads are never inlined.

## Metrics

Prometheus metrics are served on a separate port (default 9100):

```bash
--metric-host localhost
--metric-port 9100
```

Access at `http://localhost:9100/metrics`.

## Inspect subcommand

`s3-cas inspect` reports on the on-disk state:

```bash
s3-cas inspect --meta-root /tmp/s3/meta list-users
s3-cas inspect --meta-root /tmp/s3/meta user-stats alice
s3-cas inspect --meta-root /tmp/s3/meta list-buckets --user alice
s3-cas inspect --meta-root /tmp/s3/meta bucket-stats mybucket --user alice
s3-cas inspect --meta-root /tmp/s3/meta block-stats
s3-cas inspect --meta-root /tmp/s3/meta object-info mybucket file.bin --user alice
```

## On-disk layout

```
meta_root/
  blocks/   shared block metadata, refcounts, user records
  user_<id>/   per-user bucket and object metadata
fs_root/
  blocks/   actual block files (deduplicated, adaptive depth)
```

## Known limitations

- Only basic S3 API (no policies, ACLs, versioning, lifecycle rules)
- Server-side copy between different instances is not implemented
- Multipart uploads are not inlined even for small parts
