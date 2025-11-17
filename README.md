# S3-CAS

A simple POC implementation of the (basic) S3 API using content-addressable storage. The current implementation
has been running in production for 1.5 years storing some 250M objects.

## Features

- **Content-addressable storage** with automatic deduplication via MD5 hashing
- **Reference counting** - data blocks are automatically deleted when no longer referenced
- **Multi-user support** - isolate storage per user with separate S3 credentials
- **HTTP browser interface** - browse buckets and objects via web UI
- **Admin panel** - manage users, reset passwords, and view system info
- **Inline metadata** - store small objects directly in metadata for improved performance
- **Multiple storage backends** - fjall (transactional) or fjall_notx (non-transactional)

## Building

To build it yourself, clone the repo and then use the standard rust tools.
The `vendored` feature can be used if a static binary is needed.

```bash
git clone https://github.com/leesmet/s3-cas
cd s3-cas
cargo build --release --features binary
```

## Running

S3-CAS supports two modes of operation: **single-user** and **multi-user**.

### Single-User Mode

Perfect for personal use or simple deployments with one set of credentials.

```bash
s3-cas server \
  --access-key=MY_KEY \
  --secret-key=MY_SECRET \
  --fs-root=/tmp/s3/fs \
  --meta-root=/tmp/s3/meta
```

**Optional: Enable HTTP UI**

Add browser access with basic authentication:

```bash
s3-cas server \
  --access-key=MY_KEY \
  --secret-key=MY_SECRET \
  --fs-root=/tmp/s3/fs \
  --meta-root=/tmp/s3/meta \
  --enable-http-ui \
  --http-ui-host=localhost \
  --http-ui-port=8080 \
  --http-ui-username=admin \
  --http-ui-password=secret
```

Access the UI at `http://localhost:8080` (login with username/password above).

### Multi-User Mode

For production deployments requiring isolated storage per user with separate credentials.

#### 1. Create `users.toml` configuration

```toml
# users.toml
[users.alice]
access_key = "AKIAIOSFODNN7EXAMPLE"
secret_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"

[users.bob]
access_key = "AKIAI44QH8DHBEXAMPLE"
secret_key = "je7MtGbClwBF/2Zp9Utk/h3yCo8nvbEXAMPLEKEY"

[users.charlie]
access_key = "AKIDEXAMPLE"
secret_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYZEXAMPLE"
```

**Important:** Each user's data is completely isolated. Users cannot access each other's buckets or objects.

#### 2. Start server in multi-user mode

```bash
s3-cas server \
  --fs-root=/tmp/s3/fs \
  --meta-root=/tmp/s3/meta \
  --users-config=users.toml \
  --enable-http-ui \
  --http-ui-host=localhost \
  --http-ui-port=8080
```

**Note:** In multi-user mode, `--access-key` and `--secret-key` flags are not used. Authentication is handled per-user via the S3 API credentials in `users.toml`.

#### 3. Initial Setup - Admin Account

On first startup, S3-CAS will automatically migrate users from `users.toml` to the database and generate **random initial passwords** for the HTTP UI:

```
INFO Multi-user mode enabled, loading users from "users.toml"
INFO Migrating 3 users from users.toml to database...
INFO ✓ User 'alice' created | Initial password: xK9mP2nQ7wR5tL3v
INFO   Please log in and change your password immediately.
INFO ✓ User 'bob' created | Initial password: aB8dF3jK9mN2qT7w
INFO   Please log in and change your password immediately.
INFO ✓ User 'charlie' created | Initial password: pL5xR2vN8mK4wQ9t
INFO   Please log in and change your password immediately.
INFO Migration complete! 3 users created.
```

**The first user** (alice in this example) **is automatically granted admin privileges.**

#### 4. Access the HTTP UI

1. Navigate to `http://localhost:8080`
2. Log in with the first user's credentials:
   - **Username:** `alice` (same as user_id in users.toml)
   - **Password:** The random password from the console output
3. **Immediately change your password** via the admin panel

#### 5. Admin Panel Features

After logging in as an admin, you'll see an **"⚙️ Admin"** link in the navigation bar. Click it to access the admin panel where you can:

- **View all users** and their account status
- **Create new users** with both S3 credentials and HTTP UI login
- **Reset passwords** for any user
- **Delete users** (except yourself)
- **Grant/revoke admin privileges**

**Navigation Features:**
- **Admin users** see: `Buckets | Health | ⚙️ Admin | Logout`
- **Regular users** see: `Buckets | Health | Logout`
- **Single-user mode** sees: `Buckets | Health` (basic auth, no logout needed)

All users can browse their own buckets and objects, but only admins can manage users.

### HTTP Browser Interface

When `--enable-http-ui` is enabled, you can browse your S3 storage via a web browser:

- **Browse buckets** - View all your buckets at `/buckets`
- **List objects** - Click a bucket to see all objects inside
- **View metadata** - Click an object to see size, hash, creation time, and block information
- **JSON API** - All endpoints support `?format=json` for programmatic access

#### Endpoints

- `GET /` - Redirects to `/buckets`
- `GET /buckets` - List all buckets (HTML or JSON)
- `GET /buckets/{bucket}` - List objects in bucket
- `GET /buckets/{bucket}/{key}` - View object metadata
- `GET /api/v1/buckets` - List buckets (JSON only)
- `GET /api/v1/buckets/{bucket}/objects/{key}` - Object metadata (JSON)
- `GET /health` - Health check endpoint

**Multi-user mode only:**
- `GET /login` - Login page
- `POST /logout` - Logout
- `GET /admin/users` - User management (admin only)

## Storage Backends

Choose between two storage engines:

- **`fjall`** (default) - Transactional LSM-tree storage with ACID guarantees
- **`fjall_notx`** - Non-transactional variant with better performance but no transaction support

```bash
--metadata-db fjall        # Safe, transactional (recommended)
--metadata-db fjall_notx   # Faster, but avoid in multi-user mode
```

**Warning:** Using `fjall_notx` in multi-user mode may lead to data inconsistencies. Always use `fjall` (default) for multi-user deployments.

## Durability Levels

Control fsync behavior for metadata writes:

```bash
--durability buffer      # No fsync (fastest, least durable)
--durability fdatasync   # Sync data only (default, balanced)
--durability fsync       # Sync data + metadata (slowest, most durable)
```

## Inline Metadata

Objects smaller than or equal to a configurable threshold can be stored directly in their metadata records,
improving performance for small objects.

```bash
--inline-metadata-size 4096    # Store objects ≤4KB inline
```

When inline metadata is enabled:
- Small objects are stored directly in metadata (no separate block files)
- Reduces disk I/O for small file reads
- Setting to 0 or omitting disables inlining completely

**Note:** Multipart uploads are never inlined, regardless of size.

## Metrics

Prometheus metrics are exposed on a separate port (default: 9100):

```bash
--metric-host localhost
--metric-port 9100
```

Access metrics at `http://localhost:9100/metrics`

## Known Issues and Limitations

- Only basic S3 API is implemented (no bucket policies, ACLs, versioning, etc.)
- Server-side copy between different S3-CAS instances is not implemented
- No support for S3 bucket lifecycle policies
- Multipart uploads are not inlined even if small enough
