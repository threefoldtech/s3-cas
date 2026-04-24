# ADR 001: Multi-User Authentication and UI Routing

## Status
Accepted - 2025-11-17

## Context

The S3-CAS system currently has:
- **HTTP UI**: Fully functional browser interface for exploring buckets/objects with optional HTTP Basic Auth (single username/password)
- **Multi-user S3 infrastructure**: SharedBlockStore, UserRouter, per-user CasFS instances created but not connected
- **Current limitation**: All S3 API and HTTP UI requests route to the first user's CasFS instance (main.rs:296-312)
- **User storage**: users.toml file with access_key/secret_key for S3 API only

### Problems to Solve
1. **No per-request routing**: S3 API requests don't route to the correct user's CasFS based on credentials
2. **HTTP UI not multi-user aware**: No way for users to log in and see only their buckets/objects
3. **No UI credentials**: users.toml only has S3 keys, no login/password for web interface
4. **No user management**: No way to create/delete users or manage credentials after deployment
5. **Incomplete multi-user mode**: Phase 1 infrastructure exists but isn't operational

### Requirements
- **Separate credentials**: UI login/password separate from S3 access_key/secret_key
- **Login-based routing**: Session-based authentication on single domain (not path-based or subdomain)
- **TOML → DB migration**: Support both users.toml and database, with migration path
- **Admin UI panel**: Web interface for user management
- **Backward compatibility**: Single-user mode must continue to work unchanged

## Decision

### Architecture Overview

We will implement a **dual-credential, session-based authentication system** with:
1. **Database-backed user store** in Fjall's `_USERS` partition
2. **Session management** with HTTP-only cookies
3. **Per-request routing** for both S3 API and HTTP UI
4. **Admin UI** for user management

### Component Design

#### 1. User Storage (`src/auth/user_store.rs`)

```rust
struct UserRecord {
    user_id: String,           // e.g., "delandtj" (primary key)
    ui_login: String,          // username for HTTP UI login
    ui_password_hash: String,  // bcrypt hash
    s3_access_key: String,     // for S3 API (AWS format)
    s3_secret_key: String,     // for S3 API
    is_admin: bool,            // admin privileges flag
    created_at: SystemTime,    // account creation timestamp
}
```

**Storage**: `_USERS` partition in SharedBlockStore (`/meta_root/blocks/db/`)

**Operations**:
- `create_user(user_id, ui_login, ui_password, s3_access_key, s3_secret_key, is_admin)` → Result<UserRecord>
- `get_user_by_id(user_id)` → Result<Option<UserRecord>>
- `get_user_by_ui_login(ui_login)` → Result<Option<UserRecord>>
- `get_user_by_s3_key(access_key)` → Result<Option<UserRecord>>
- `list_users()` → Result<Vec<UserRecord>>
- `delete_user(user_id)` → Result<()>
- `update_password(user_id, new_password_hash)` → Result<()>
- `verify_password(user_id, password)` → Result<bool>

**Password Hashing**: bcrypt (cost factor 12)

#### 2. Session Management (`src/auth/session.rs`)

```rust
struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, SessionData>>>,
}

struct SessionData {
    user_id: String,
    created_at: Instant,
    expires_at: Instant,
}
```

**Session ID**: 32-byte cryptographically random hex string (64 chars)

**Storage**: In-memory HashMap (sessions lost on restart - acceptable for MVP)

**Lifetime**: 24 hours (configurable)

**Cookie**: `session_id={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age=86400`

**Operations**:
- `create_session(user_id)` → String (session_id)
- `get_session(session_id)` → Option<String> (user_id)
- `delete_session(session_id)`
- `cleanup_expired()` (background task)

#### 3. HTTP UI Authentication Middleware (`src/http_ui/middleware.rs`)

**Middleware chain**:
```
Request → Cookie Parser → Session Validator → User Resolver → Handler
```

**Public routes** (no auth required):
- `/login` (GET/POST)
- `/health`

**Protected routes** (session required):
- `/` → `/buckets`
- `/buckets`
- `/buckets/{bucket}`
- `/buckets/{bucket}/{key}`
- `/api/v1/*`

**Admin routes** (session + is_admin required):
- `/admin/users`
- `/admin/users/new`
- `/admin/users/{id}/*`

**Error handling**:
- Missing/invalid session → 302 redirect to `/login?redirect={original_path}`
- Admin required → 403 Forbidden

#### 4. Login/Logout Handlers (`src/http_ui/login.rs`)

**GET /login**:
- Render login form template
- Support `?redirect={path}` parameter for post-login redirect
- If already authenticated, redirect to `/buckets`

**POST /login**:
- Parse form: `ui_login={username}&password={password}`
- Validate credentials via `user_store::verify_password()`
- Create session via `session_store::create_session()`
- Set session cookie
- Redirect to `redirect` param or `/buckets`

**POST /logout**:
- Extract session from cookie
- Delete session via `session_store::delete_session()`
- Clear cookie
- Redirect to `/login`

#### 5. S3 API Per-Request Routing (`src/s3_wrapper.rs`)

**Problem**: s3s library's `S3ServiceBuilder` expects a single storage backend, but we need per-request routing.

**Solution**: Implement custom wrapper around UserRouter that implements the s3s `S3` trait:

```rust
struct S3UserRouter {
    user_router: Arc<UserRouter>,
    user_store: Arc<UserStore>,
}

#[async_trait::async_trait]
impl s3s::S3 for S3UserRouter {
    async fn put_object(&self, req: S3Request<PutObjectInput>) -> S3Result<...> {
        // Extract access_key from Authorization header
        let access_key = extract_access_key(&req)?;

        // Lookup user_id
        let user = self.user_store.get_user_by_s3_key(&access_key)?;

        // Route to correct CasFS
        let casfs = self.user_router.get_casfs(&user.user_id)?;

        // Execute request
        casfs.put_object(req).await
    }

    // ... implement all S3 trait methods similarly
}
```

**Integration**: Replace `MetricFs<S3FS>` with `MetricFs<S3UserRouter>` in main.rs

#### 6. Admin UI (`src/http_ui/admin.rs`)

**Routes**:
- `GET /admin/users` → List all users (table with user_id, ui_login, s3_access_key, is_admin, actions)
- `GET /admin/users/new` → User creation form
- `POST /admin/users` → Create user (generate random S3 keys if not provided)
- `DELETE /admin/users/{id}` → Delete user (requires confirmation)
- `GET /admin/users/{id}/reset-password` → Password reset form
- `PATCH /admin/users/{id}/password` → Update password

**Security**:
- All routes protected by admin middleware (is_admin=true check)
- CSRF protection via form tokens (future enhancement)
- Audit logging for user management actions (future enhancement)

#### 7. Migration Path

**Startup sequence (multi-user mode)**:

1. Load SharedBlockStore
2. Open `_USERS` partition
3. Check if partition is empty
4. If empty and `users.toml` exists:
   - Parse users.toml
   - For each user in TOML:
     - Generate random initial password (16 chars alphanumeric)
     - Create UserRecord:
       - `user_id` = TOML key
       - `ui_login` = TOML key
       - `ui_password_hash` = bcrypt(random_password)
       - `s3_access_key` = from TOML
       - `s3_secret_key` = from TOML
       - `is_admin` = true (first user only)
     - Insert into `_USERS`
     - Log: "User {user_id} created with initial password: {password}"
   - Log: "Migration complete. Please save initial passwords and reset them via /admin/users"
5. Continue startup with database users

**Backward compatibility**:
- Single-user mode: No changes, existing `--http-ui-username`/`--http-ui-password` flags work as before
- Multi-user mode: users.toml automatically migrated to DB on first run

### Templates (`src/http_ui/templates.rs`)

**New templates**:

1. **Login page**:
   - Clean, centered form
   - Username and password fields
   - "Login" button
   - Match existing dark mode theme
   - Display error message if credentials invalid

2. **Admin panel**:
   - User list table with sortable columns
   - "Create User" button → modal or new page
   - Per-user actions: "Reset Password", "Delete"
   - Visual indicator for admin users (badge)

3. **Navigation updates**:
   - Add "Admin" link to nav bar when `is_admin=true`
   - Add "Logout" link/button

## Consequences

### Positive
- ✅ **Complete multi-user isolation**: Each user sees only their buckets/objects
- ✅ **Proper S3 routing**: S3 API requests route to correct user's CasFS
- ✅ **Self-service UI**: Users can log in and browse via web interface
- ✅ **Admin tools**: User management without restarting server
- ✅ **Security**: Separate UI and S3 credentials, bcrypt password hashing
- ✅ **Smooth migration**: Existing users.toml automatically migrated
- ✅ **Backward compatible**: Single-user mode unchanged

### Negative
- ⚠️ **Sessions lost on restart**: In-memory session store (acceptable for MVP, can migrate to DB later)
- ⚠️ **Increased complexity**: More code to maintain (auth, sessions, middleware)
- ⚠️ **Breaking change for multi-user**: users.toml format extended (but auto-migrated)
- ⚠️ **No CSRF protection yet**: Deferred to future iteration

### Neutral
- 📝 **New dependencies**: bcrypt, cookie (small, well-maintained crates)
- 📝 **Admin bootstrap**: First user becomes admin automatically

## Implementation Plan

### Phase 1: Core Infrastructure
1. Add dependencies: `cargo add bcrypt cookie`
2. Create `src/auth/user_store.rs` with UserRecord and CRUD
3. Create `src/auth/session.rs` with session management
4. Update `src/auth/mod.rs` to export new modules

### Phase 2: HTTP UI Authentication
5. Create `src/http_ui/middleware.rs` for session validation
6. Create `src/http_ui/login.rs` for login/logout handlers
7. Modify `src/http_ui/templates.rs` to add login page
8. Modify `src/http_ui/mod.rs` to integrate middleware and routes

### Phase 3: S3 API Routing
9. Create `src/s3_wrapper.rs` implementing S3 trait with per-request routing
10. Modify `src/main.rs` to use S3UserRouter instead of direct CasFS

### Phase 4: Migration & Admin UI
11. Implement migration logic in `src/main.rs` (users.toml → DB)
12. Create `src/http_ui/admin.rs` for user management
13. Modify `src/http_ui/templates.rs` to add admin UI
14. Add admin routes to `src/http_ui/mod.rs`

### Phase 5: Testing & Documentation
15. Test single-user mode (ensure no regression)
16. Test multi-user mode (migration, login, S3 routing, admin UI)
17. Update README with multi-user setup instructions
18. Document password reset procedures

## Alternatives Considered

### 1. Path-based routing (`/users/{username}/buckets`)
**Rejected**: Less intuitive UX, exposes usernames in URLs, complicates navigation

### 2. JWT tokens instead of sessions
**Rejected**: Can't revoke tokens without additional infrastructure, overkill for this use case

### 3. Reuse S3 credentials for UI login
**Rejected**: S3 secret keys are long and not user-friendly, security best practice is separate credentials

### 4. HTTP Basic Auth for multi-user
**Rejected**: No proper session management, credentials sent with every request, no logout mechanism

### 5. External auth provider (OAuth, LDAP)
**Rejected**: Too complex for initial implementation, can be added later

### 6. Keep users.toml only
**Rejected**: No way to change passwords without restarting server, no runtime user management

## References
- [Multi-User PRD](../multi-user-prd.md) - Phase 1 specification
- [Refcount Documentation](../refcount.md) - Block deduplication architecture
- [CLAUDE.md](../../CLAUDE.md) - Complete codebase function map

## Notes
- Session store can be migrated to Fjall partition in future for persistence
- CSRF protection should be added before production deployment
- Rate limiting on login endpoint should be added
- Audit logging for admin actions should be added
- Email/notification system for password resets can be added later
