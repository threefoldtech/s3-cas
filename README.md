# QSS S3-CAS

> Note: this is a continuation of Lee Smet's original
> [s3-cas](https://github.com/leesmet/s3-cas), the foundational work on
> content-addressable S3 storage.

QSS S3-CAS is an S3-compatible object storage server with a Content-Addressed Storage (CAS) backend. It provides Amazon S3 API compatibility while storing objects in a deduplicated, content-addressed manner. Existing S3-based applications can use it with minimal or no changes.

## What this is

QSS S3-CAS exposes a standard S3 API while internally deduplicating data blocks using MD5 hashing. Objects are split into blocks; identical blocks across different users and objects are stored only once. Reference counting ensures blocks are deleted only when no longer referenced by any object. Multi-user isolation is maintained at the bucket and object level while sharing block storage globally.

## What this repository contains

- **`s3-cas server`** — An S3-compatible server with configurable durability, inline metadata, and Prometheus metrics
- **User management CLI** — Commands to create, list, and delete users with S3 access/secret credentials
- **Inspect tooling** — Commands to report on-disk state including users, buckets, blocks, and objects
- **Presigned URL helper** — CLI tool to generate time-limited S3 URLs without requiring the AWS CLI
- **Two Fjall backends**: `fjall` (transactional) and `fjall_notx`

This build focuses on the S3 server plus the underlying CAS storage library. The previous HTTP browser UI, admin panel, and single-user mode have been removed on the `simplify/drop-ui-and-single-user` branch to reduce surface area; see [docs/prd/prd000-current-state-and-restructure.md](docs/prd/prd000-current-state-and-restructure.md) for the rationale and roadmap.

## Role in the stack

QSS S3-CAS functions as a storage gateway layer, providing standard S3 API access over a content-addressed storage backend. It can be used wherever S3 compatibility is required but storage efficiency and deduplication are desired. It fits alongside other storage components in the broader stack.

## Relation to ThreeFold

This technology is used within the ThreeFold ecosystem and was first deployed on the ThreeFold Grid. The component itself is designed as reusable infrastructure technology and should be understood by its technical function first, independent of any specific deployment.

## Ownership

This repository is owned and maintained by TF-Tech NV, a Belgian company responsible for the development and maintenance of this technology.

## Building

```bash
git clone https://github.com/threefoldtech/qss_s3_cas
cd s3-cas
cargo build --release
```

## Creating the first user

The server refuses to start against an empty user database. Create a user first:

```bash
s3-cas user --meta-root /tmp/s3/meta add alice --admin
```

Output:

```
User 'alice' created (admin=true)
  access_key: XXXXXXXXXXXXXXXXXXXX
  secret_key: xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
Save these credentials -- they will not be shown again.
```

Other subcommands:

```bash
s3-cas user --meta-root /tmp/s3/meta list
s3-cas user --meta-root /tmp/s3/meta delete alice
```

## Running the server

```bash
s3-cas server \
  --fs-root=/tmp/s3/fs \
  --meta-root=/tmp/s3/meta
```

Each user's objects are isolated; users cannot list or access each other's buckets. Block-level dedup is global across users.

## Storage backends

- `fjall` (default) -- transactional LSM tree with ACID guarantees
- `fjall_notx` -- non-transactional, faster, not recommended for multi-user workloads

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

Objects smaller than or equal to the configured threshold are stored directly in their metadata record, avoiding a separate block file.

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

## Presigned URLs

Hand out a time-limited URL to a single object without needing the AWS CLI or `boto3` installed on the host. The subcommand reads the user's S3 credentials from the local `_USERS` partition and emits a standard AWS SigV4 query-string URL that any S3 client (curl included) will accept:

```bash
s3-cas presign \
  --meta-root /tmp/s3/meta \
  --user alice \
  --endpoint http://localhost:8014 \
  --ttl 15m \
  mybucket path/to/file.txt
```

Prints one URL to stdout. Flags:

- `--ttl <duration>` accepts `30s`, `15m`, `2h`, `1d`. Capped at 7 days (SigV4 limit).
- `--method <GET|PUT|HEAD|DELETE>` defaults to GET.
- `--region <name>` defaults to `us-east-1`.

The URL verifies server-side via the same SigV4 path a normal request uses; no server configuration or state is required. See [docs/adr/006-presigned-urls-and-cli-helper.md](docs/adr/006-presigned-urls-and-cli-helper.md) for the background.

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

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.
