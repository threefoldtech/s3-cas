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
cargo build --release
```

OR

```bash
cargo build --release --target x86_64-unknown-linux-musl --features vendored
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

**Multi-user mode is the default** - just run the server without specifying `--access-key` or `--secret-key`:

```bash
s3-cas server \
  --fs-root=/tmp/s3/fs \
  --meta-root=/tmp/s3/meta \
  --enable-http-ui \
  --http-ui-host=localhost \
  --http-ui-port=8080
```

For listening on an IP address, or listening on all interfaces , use IP or 0.0.0.0 for --http-ui-host

**Important:** Each user's data is completely isolated. Users cannot access each other's buckets or objects.

#### First-Time Setup - Create Admin Account

When you first access the HTTP UI with no users in the database:

1. Navigate to `http://localhost:8080` (redirects to `/login`)
2. You'll see a **setup form** instead of a login form
3. Create your admin account:
   - **Username:** Choose a username for HTTP UI login
   - **Password:** Choose a secure password (minimum 8 characters)
   - **Confirm Password:** Re-enter your password
4. Click **"Create Admin Account"**

S3-CAS will automatically:

- Create your admin account with is_admin privileges
- **Auto-generate S3 credentials** (access_key and secret_key)
- Log you in immediately
- Display your S3 credentials **once** with a warning to save them

**Important:** S3 credentials are shown only once after setup! Save them in a secure location. You can view them later in your profile page.

#### Admin Panel Features

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

- `GET /login` - Login page (or setup form if no users exist)
- `POST /setup-admin` - Create first admin account
- `POST /logout` - Logout
- `GET /admin/users` - User management (admin only)
- `GET /profile` - View user profile and S3 credentials

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
