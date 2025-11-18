use maud::{html, Markup, PreEscaped, DOCTYPE};

use super::handlers::{BucketInfo, ObjectListResponse, ObjectMetadata};

/// Base HTML layout
fn layout(title: &str, content: Markup) -> Markup {
    layout_with_user(title, content, None)
}

/// Base HTML layout with user context (for multi-user mode)
fn layout_with_user(title: &str, content: Markup, is_admin: Option<bool>) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                style { (PreEscaped(STYLES)) }
            }
            body {
                header {
                    h1 { "S3-CAS Browser" }
                    nav {
                        a href="/buckets" { "Buckets" }
                        " | "
                        a href="/health" { "Health" }
                        @if is_admin.is_some() {
                            " | "
                            a href="/profile" { "👤 Profile" }
                        }
                        @if let Some(true) = is_admin {
                            " | "
                            a href="/admin/users" class="admin-link" { "⚙️ Admin" }
                        }
                        @if is_admin.is_some() {
                            " | "
                            form method="post" action="/logout" style="display: inline;" {
                                button type="submit" class="logout-button" { "Logout" }
                            }
                        }
                    }
                }
                main {
                    (content)
                }
                footer {
                    p { "s3-cas v" (env!("CARGO_PKG_VERSION")) }
                }
            }
        }
    }
}

/// Bucket list page (multi-user mode)
pub fn buckets_page_with_user(buckets: &[BucketInfo], is_admin: bool) -> String {
    let content = html! {
        div class="page-header" {
            h2 { "Buckets" }
            span class="count" { (buckets.len()) " bucket(s)" }
        }

        @if buckets.is_empty() {
            p class="empty-state" { "No buckets found" }
        } @else {
            table {
                thead {
                    tr {
                        th { "Name" }
                        th { "Created" }
                    }
                }
                tbody {
                    @for bucket in buckets {
                        tr {
                            td {
                                a href={ "/buckets/" (&bucket.name) } {
                                    (&bucket.name)
                                }
                            }
                            td { (&bucket.creation_date) }
                        }
                    }
                }
            }
        }
    };

    layout_with_user("Buckets - S3-CAS", content, Some(is_admin)).into_string()
}

/// Bucket list page (single-user mode)
pub fn buckets_page(buckets: &[BucketInfo]) -> String {
    let content = html! {
        div class="page-header" {
            h2 { "Buckets" }
            span class="count" { (buckets.len()) " bucket(s)" }
        }

        @if buckets.is_empty() {
            p class="empty-state" { "No buckets found" }
        } @else {
            table {
                thead {
                    tr {
                        th { "Name" }
                        th { "Created" }
                    }
                }
                tbody {
                    @for bucket in buckets {
                        tr {
                            td {
                                a href={ "/buckets/" (&bucket.name) } {
                                    (&bucket.name)
                                }
                            }
                            td { (&bucket.creation_date) }
                        }
                    }
                }
            }
        }
    };

    layout("Buckets - S3-CAS", content).into_string()
}

/// Object list page
pub fn objects_page(response: &ObjectListResponse) -> String {
    // Build breadcrumb navigation from prefix
    let breadcrumb_parts = if response.prefix.is_empty() {
        vec![]
    } else {
        response.prefix.trim_end_matches('/').split('/').collect()
    };

    let content = html! {
        div class="breadcrumb" {
            a href="/buckets" { "Buckets" }
            " / "
            a href={ "/buckets/" (response.bucket) } { (response.bucket) }
            @if !breadcrumb_parts.is_empty() {
                @for (i, part) in breadcrumb_parts.iter().enumerate() {
                    " / "
                    @if i == breadcrumb_parts.len() - 1 {
                        strong { (part) }
                    } @else {
                        @let prefix = breadcrumb_parts[..=i].join("/") + "/";
                        a href={ "/buckets/" (response.bucket) "?prefix=" (urlencoding::encode(&prefix)) } {
                            (part)
                        }
                    }
                }
            }
        }

        div class="page-header" {
            h2 {
                @if response.prefix.is_empty() {
                    "Objects in \"" (response.bucket) "\""
                } @else {
                    "\"" (response.prefix.trim_end_matches('/')) "\""
                }
            }
            span class="count" { (response.total_count) " item(s)" }
        }

        @if response.directories.is_empty() && response.objects.is_empty() {
            p class="empty-state" { "No objects in this location" }
        } @else {
            table {
                thead {
                    tr {
                        th { "Name" }
                        th class="number" { "Size" }
                        th { "Type" }
                        th { "Last Modified" }
                    }
                }
                tbody {
                    // Show directories first
                    @for dir in &response.directories {
                        tr class="directory-row" {
                            td {
                                a href={ "/buckets/" (response.bucket) "?prefix=" (urlencoding::encode(&dir.prefix)) } {
                                    "📁 " (dir.name)
                                }
                            }
                            td class="number" { "—" }
                            td { span class="badge directory" { "folder" } }
                            td { "—" }
                        }
                    }
                    // Show files
                    @for obj in &response.objects {
                        tr {
                            td {
                                a href={ "/buckets/" (response.bucket) "/" (obj.key) } {
                                    "📄 " (obj.key.rsplit('/').next().unwrap_or(&obj.key))
                                }
                            }
                            td class="number" { (format_size(obj.size)) }
                            td {
                                @if obj.is_inlined {
                                    span class="badge inline" { "inline" }
                                } @else {
                                    span class="badge blocks" { "blocks" }
                                }
                            }
                            td { (obj.last_modified) }
                        }
                    }
                }
            }
        }
    };

    layout(&format!("{} - S3-CAS", response.bucket), content).into_string()
}

/// Object detail page
pub fn object_detail_page(metadata: &ObjectMetadata) -> String {
    let content = html! {
        div class="breadcrumb" {
            a href="/buckets" { "← Buckets" }
            " / "
            a href={ "/buckets/" (metadata.bucket) } { (metadata.bucket) }
            " / "
            strong { (metadata.key) }
        }

        h2 { "Object Metadata" }

        dl class="metadata" {
            dt { "Key" }
            dd { code { (metadata.key) } }

            dt { "Bucket" }
            dd { code { (metadata.bucket) } }

            dt { "Size" }
            dd { (format_size(metadata.size)) " (" (metadata.size) " bytes)" }

            dt { "Content Hash (MD5)" }
            dd { code class="hash-full" { (metadata.hash) } }

            dt { "Last Modified" }
            dd { (metadata.last_modified) }

            dt { "Storage Type" }
            dd {
                @if metadata.is_inlined {
                    span class="badge inline" { "Inline" }
                    " (stored in metadata)"
                } @else {
                    span class="badge blocks" { "Blocks" }
                    " (content-addressable storage)"
                }
            }

            dt { "Block Count" }
            dd { (metadata.blocks.len()) }

            @if !metadata.blocks.is_empty() {
                dt { "Blocks" }
                dd {
                    table class="blocks-table" {
                        thead {
                            tr {
                                th { "#" }
                                th { "Hash" }
                                th class="number" { "Size" }
                                th class="number" { "Refcount" }
                            }
                        }
                        tbody {
                            @for (i, block) in metadata.blocks.iter().enumerate() {
                                tr {
                                    td { (i + 1) }
                                    td { code class="hash-full" { (block.hash) } }
                                    td class="number" { (format_size(block.size as u64)) }
                                    td class="number" {
                                        (block.refcount)
                                        @if block.refcount > 1 {
                                            " "
                                            span class="dedup-badge" { "shared" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    layout(&format!("{} - S3-CAS", metadata.key), content).into_string()
}

/// Error page (simple version for admin/login)
pub fn error_page(message: &str) -> String {
    let content = html! {
        div class="error-page" {
            h2 { "Error" }
            p { (message) }
            p {
                a href="/buckets" { "← Back to buckets" }
            }
        }
    };

    layout("Error - S3-CAS", content).into_string()
}

/// Login page
pub fn login_page(redirect_to: &str, error: Option<&str>) -> String {
    let content = html! {
        div class="login-container" {
            div class="login-box" {
                h2 { "Login" }

                @if let Some(err) = error {
                    div class="alert alert-error" {
                        (err)
                    }
                }

                form method="POST" action="/login" {
                    input type="hidden" name="redirect" value=(redirect_to);

                    div class="form-group" {
                        label for="username" { "Username" }
                        input type="text" id="username" name="username" required autofocus;
                    }

                    div class="form-group" {
                        label for="password" { "Password" }
                        input type="password" id="password" name="password" required;
                    }

                    button type="submit" class="btn btn-primary" { "Login" }
                }
            }
        }
    };

    layout("Login - S3-CAS", content).into_string()
}

/// First-time setup page for creating admin account
pub fn setup_admin_page(error: Option<&str>) -> String {
    let content = html! {
        div class="login-container" {
            div class="login-box" {
                h2 { "Welcome to S3-CAS" }
                p class="setup-message" {
                    "No users found. Let's create your admin account."
                }

                @if let Some(err) = error {
                    div class="alert alert-error" {
                        (err)
                    }
                }

                form method="POST" action="/setup-admin" {
                    div class="form-group" {
                        label for="ui_login" { "Admin Username" }
                        input type="text" id="ui_login" name="ui_login" required autofocus
                            placeholder="Enter your username";
                    }

                    div class="form-group" {
                        label for="password" { "Password" }
                        input type="password" id="password" name="password" required
                            placeholder="At least 8 characters";
                        small { "Minimum 8 characters" }
                    }

                    div class="form-group" {
                        label for="confirm_password" { "Confirm Password" }
                        input type="password" id="confirm_password" name="confirm_password" required
                            placeholder="Re-enter your password";
                    }

                    p class="setup-note" {
                        "S3 credentials will be automatically generated and shown after setup."
                    }

                    button type="submit" class="btn btn-primary" { "Create Admin Account" }
                }
            }
        }
    };

    layout("Setup Admin - S3-CAS", content).into_string()
}

/// Admin users list page
pub fn admin_users_page(users: &[crate::auth::UserRecord]) -> String {
    let content = html! {
        div class="page-header" {
            h2 { "User Management" }
            a href="/admin/users/new" class="btn btn-primary" { "+ Create User" }
        }

        @if users.is_empty() {
            p class="empty-state" { "No users found" }
        } @else {
            table {
                thead {
                    tr {
                        th { "User ID" }
                        th { "UI Login" }
                        th { "S3 Access Key" }
                        th { "Admin" }
                        th { "Created" }
                        th { "Actions" }
                    }
                }
                tbody {
                    @for user in users {
                        tr {
                            td { code { (&user.user_id) } }
                            td { (&user.ui_login) }
                            td { code { (&user.s3_access_key) } }
                            td {
                                @if user.is_admin {
                                    span class="badge admin" { "Admin" }
                                } @else {
                                    span class="badge" { "User" }
                                }
                            }
                            td { (format_unix_timestamp(user.created_at)) }
                            td class="actions" {
                                a href={"/admin/users/" (&user.user_id) "/reset-password"} class="btn btn-small" {
                                    "Reset Password"
                                }
                                " "
                                form method="POST" action={"/admin/users/" (&user.user_id) "/toggle-admin"} style="display: inline;" {
                                    button type="submit" class="btn btn-small"
                                            title={@if user.is_admin { "Revoke admin rights" } @else { "Grant admin rights" }} {
                                        @if user.is_admin {
                                            "Revoke Admin"
                                        } @else {
                                            "Make Admin"
                                        }
                                    }
                                }
                                " "
                                form method="POST" action={"/admin/users/" (&user.user_id) "/delete"} style="display: inline;" {
                                    button type="submit" class="btn btn-small btn-danger"
                                            onclick={"return confirm('Delete user " (&user.user_id) "?');"} {
                                        "Delete"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        p class="help-text" {
            a href="/buckets" { "← Back to buckets" }
        }
    };

    layout("User Management - S3-CAS", content).into_string()
}

/// New user creation form
pub fn new_user_form() -> String {
    let content = html! {
        div class="form-container" {
            h2 { "Create New User" }

            form method="POST" action="/admin/users" {
                div class="form-group" {
                    label for="user_id" { "User ID" span class="required" { "*" } }
                    input type="text" id="user_id" name="user_id" required;
                    small { "Unique identifier (e.g., username)" }
                }

                div class="form-group" {
                    label for="ui_login" { "UI Login" span class="required" { "*" } }
                    input type="text" id="ui_login" name="ui_login" required;
                    small { "Login for web interface" }
                }

                div class="form-group" {
                    label for="ui_password" { "UI Password" }
                    input type="password" id="ui_password" name="ui_password";
                    small { "Leave empty to auto-generate" }
                }

                div class="form-group" {
                    label for="s3_access_key" { "S3 Access Key" }
                    input type="text" id="s3_access_key" name="s3_access_key";
                    small { "Leave empty to auto-generate" }
                }

                div class="form-group" {
                    label for="s3_secret_key" { "S3 Secret Key" }
                    input type="password" id="s3_secret_key" name="s3_secret_key";
                    small { "Leave empty to auto-generate" }
                }

                div class="form-group" {
                    label {
                        input type="checkbox" id="is_admin" name="is_admin";
                        " Admin privileges"
                    }
                }

                div class="form-actions" {
                    button type="submit" class="btn btn-primary" { "Create User" }
                    " "
                    a href="/admin/users" class="btn" { "Cancel" }
                }
            }
        }
    };

    layout("Create User - S3-CAS", content).into_string()
}

/// Password reset form
pub fn reset_password_form(user: &crate::auth::UserRecord) -> String {
    let content = html! {
        div class="form-container" {
            h2 { "Reset Password for " (&user.ui_login) }

            form method="POST" action={"/admin/users/" (&user.user_id) "/password"} {
                div class="form-group" {
                    label for="new_password" { "New Password" span class="required" { "*" } }
                    input type="password" id="new_password" name="new_password" required autofocus;
                }

                div class="alert alert-info" {
                    "Note: This will invalidate all active sessions for this user."
                }

                div class="form-actions" {
                    button type="submit" class="btn btn-primary" { "Reset Password" }
                    " "
                    a href="/admin/users" class="btn" { "Cancel" }
                }
            }
        }
    };

    layout(&format!("Reset Password - {}", user.ui_login), content).into_string()
}

/// Profile page showing S3 credentials and password change form
pub fn profile_page(user: &crate::auth::UserRecord, error_message: Option<&str>, is_setup: bool) -> String {
    let content = html! {
        h2 { "My Profile" }

        @if is_setup {
            div class="alert alert-success" style="margin-bottom: 2rem;" {
                h3 style="margin-top: 0;" { "Setup Complete!" }
                p {
                    "Your admin account has been created successfully. "
                    strong { "Please save your S3 credentials below - they cannot be retrieved later." }
                }
                p style="margin-bottom: 0;" {
                    "You can now use these credentials to connect S3 clients to this server."
                }
            }
        }

        div class="profile-section" {
            h3 { "Account Information" }
            table class="info-table" {
                tr {
                    th { "User ID" }
                    td { (&user.user_id) }
                }
                tr {
                    th { "UI Login" }
                    td { (&user.ui_login) }
                }
                @if user.is_admin {
                    tr {
                        th { "Role" }
                        td {
                            span class="badge badge-admin" { "Administrator" }
                        }
                    }
                }
            }
        }

        div class="profile-section" {
            h3 { "S3 Credentials" }
            p class="help-text" {
                "Use these credentials to configure your S3 client (e.g., aws-cli, s3cmd, rclone)"
            }

            table class="info-table credentials-table" {
                tr {
                    th { "Access Key" }
                    td {
                        code class="credential" { (&user.s3_access_key) }
                    }
                }
                tr {
                    th { "Secret Key" }
                    td {
                        code class="credential" id="secret-key" data-secret=(&user.s3_secret_key) { "••••••••••••••••••••" }
                        " "
                        button type="button" class="btn-small" onclick="toggleSecret()" { "Show" }
                    }
                }
            }

            details class="example-config" {
                summary { "Example: AWS CLI Configuration" }
                pre {
                    code class="config-code" {
                        "[profile s3cas]\n"
                        "aws_access_key_id = " (&user.s3_access_key) "\n"
                        "aws_secret_access_key = " (&user.s3_secret_key) "\n"
                        "endpoint_url = http://localhost:8014\n"
                        "region = us-east-1"
                    }
                }
            }

            details class="example-config" {
                summary { "Example: MinIO Client (mc) Configuration" }
                pre {
                    code class="config-code" {
                        "mc alias set s3cas http://localhost:8014 " (&user.s3_access_key) " " (&user.s3_secret_key)
                    }
                }
                p class="help-text" style="margin-top: 0.5rem;" {
                    "Then use: "
                    code { "mc ls s3cas/" }
                    ", "
                    code { "mc cp file.txt s3cas/mybucket/" }
                }
            }

            script {
                (PreEscaped(r#"
                    let isShown = false;
                    function toggleSecret() {
                        const el = document.getElementById('secret-key');
                        const btn = event.target;
                        if (isShown) {
                            el.textContent = '••••••••••••••••••••';
                            btn.textContent = 'Show';
                        } else {
                            el.textContent = el.dataset.secret;
                            btn.textContent = 'Hide';
                        }
                        isShown = !isShown;
                    }
                "#))
            }
        }

        div class="profile-section" {
            h3 { "Change Password" }

            // Show error messages
            @if let Some(error) = error_message {
                div class="alert alert-error" {
                    (error)
                }
            }

            form method="POST" action="/profile/password" {
                div class="form-group" {
                    label for="current_password" { "Current Password" span class="required" { "*" } }
                    input type="password" id="current_password" name="current_password" required;
                }

                div class="form-group" {
                    label for="new_password" { "New Password" span class="required" { "*" } }
                    input type="password" id="new_password" name="new_password" required;
                }

                div class="form-group" {
                    label for="confirm_password" { "Confirm New Password" span class="required" { "*" } }
                    input type="password" id="confirm_password" name="confirm_password" required;
                }

                div class="alert alert-info" {
                    "Note: Changing your password will log you out from all devices."
                }

                div class="form-actions" {
                    button type="submit" class="btn btn-primary" { "Change Password" }
                }
            }
        }
    };

    layout_with_user("My Profile - S3-CAS", content, Some(user.is_admin)).into_string()
}

// Helper functions

#[allow(dead_code)]
fn format_timestamp(time: std::time::SystemTime) -> String {
    use std::time::SystemTime;
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let datetime = chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0)
        .unwrap_or_default();
    datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.2} {}", size, UNITS[unit_idx])
    }
}

fn format_unix_timestamp(unix_seconds: u64) -> String {
    let datetime = chrono::DateTime::from_timestamp(unix_seconds as i64, 0)
        .unwrap_or_default();
    datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

// CSS Styles
const STYLES: &str = r#"
* {
    margin: 0;
    padding: 0;
    box-sizing: border-box;
}

body {
    font-family: system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    line-height: 1.6;
    color: #333;
    background: #f5f5f5;
    padding-bottom: 3rem;
}

header {
    background: #2c3e50;
    color: white;
    padding: 1rem 2rem;
    display: flex;
    justify-content: space-between;
    align-items: center;
}

header h1 {
    font-size: 1.5rem;
    font-weight: 600;
}

nav a {
    color: #ecf0f1;
    text-decoration: none;
    font-size: 0.9rem;
}

nav a:hover {
    text-decoration: underline;
}

main {
    max-width: 1400px;
    margin: 2rem auto;
    padding: 0 2rem;
    background: white;
    border-radius: 8px;
    box-shadow: 0 1px 3px rgba(0,0,0,0.1);
    padding: 2rem;
}

footer {
    text-align: center;
    color: #666;
    font-size: 0.85rem;
    margin-top: 2rem;
}

.breadcrumb {
    color: #666;
    margin-bottom: 1.5rem;
    font-size: 0.9rem;
}

.breadcrumb a {
    color: #3498db;
    text-decoration: none;
}

.breadcrumb a:hover {
    text-decoration: underline;
}

.page-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 1.5rem;
    padding-bottom: 1rem;
    border-bottom: 2px solid #ecf0f1;
}

.page-header h2 {
    font-size: 1.75rem;
    color: #2c3e50;
}

.count {
    color: #7f8c8d;
    font-size: 0.9rem;
}

table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 1rem;
}

th, td {
    text-align: left;
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid #ecf0f1;
}

th {
    background: #f8f9fa;
    font-weight: 600;
    color: #555;
    font-size: 0.9rem;
}

th.number, td.number {
    text-align: right;
}

tbody tr:hover {
    background: #f8f9fa;
}

tbody a {
    color: #3498db;
    text-decoration: none;
}

tbody a:hover {
    text-decoration: underline;
}

code {
    background: #f8f9fa;
    padding: 0.2rem 0.5rem;
    border-radius: 3px;
    font-size: 0.85rem;
    font-family: 'Courier New', monospace;
}

.hash-short {
    color: #7f8c8d;
}

.hash-full {
    word-break: break-all;
    font-size: 0.8rem;
}

.badge {
    display: inline-block;
    padding: 0.2rem 0.5rem;
    border-radius: 3px;
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
}

.badge.inline {
    background: #e8f5e9;
    color: #2e7d32;
}

.badge.blocks {
    background: #e3f2fd;
    color: #1565c0;
}

.badge.directory {
    background: #fff3e0;
    color: #e65100;
}

.directory-row {
    font-weight: 500;
}

.directory-row:hover {
    background: #fffbf5;
}

.dedup-badge {
    background: #fff3e0;
    color: #e65100;
    padding: 0.1rem 0.3rem;
    border-radius: 3px;
    font-size: 0.7rem;
    font-weight: 600;
}

.metadata {
    background: #f8f9fa;
    padding: 1.5rem;
    border-radius: 6px;
    margin: 1.5rem 0;
}

.metadata dt {
    font-weight: 600;
    color: #555;
    margin-top: 1rem;
}

.metadata dt:first-child {
    margin-top: 0;
}

.metadata dd {
    margin: 0.5rem 0 0 0;
}

.blocks-table {
    margin-top: 0.5rem;
    background: white;
}

.blocks-table th {
    background: #ecf0f1;
}

.empty-state {
    text-align: center;
    color: #95a5a6;
    padding: 3rem 0;
    font-size: 1.1rem;
}

.error-page {
    text-align: center;
    padding: 3rem 0;
}

.error-page h2 {
    color: #e74c3c;
    margin-bottom: 1rem;
}

.error-page p {
    margin: 1rem 0;
}

.error-page a {
    color: #3498db;
    text-decoration: none;
}

.error-page a:hover {
    text-decoration: underline;
}

@media (max-width: 768px) {
    main {
        margin: 1rem;
        padding: 1rem;
    }

    header {
        flex-direction: column;
        text-align: center;
    }

    .page-header {
        flex-direction: column;
        align-items: flex-start;
    }

    table {
        font-size: 0.85rem;
    }

    th, td {
        padding: 0.5rem;
    }
}

/* Login page */
.login-container {
    display: flex;
    justify-content: center;
    align-items: center;
    min-height: 60vh;
}

.login-box {
    width: 100%;
    max-width: 400px;
    padding: 2rem;
    border: 1px solid #ddd;
    border-radius: 8px;
    background: white;
}

.login-box h2 {
    margin-bottom: 1.5rem;
    text-align: center;
}

/* Forms */
.form-container {
    max-width: 600px;
    margin: 0 auto;
}

.form-group {
    margin-bottom: 1.5rem;
}

.form-group label {
    display: block;
    margin-bottom: 0.5rem;
    font-weight: 500;
}

.form-group input[type="text"],
.form-group input[type="password"] {
    width: 100%;
    padding: 0.5rem;
    border: 1px solid #ddd;
    border-radius: 4px;
    font-size: 1rem;
}

.form-group small {
    display: block;
    margin-top: 0.25rem;
    color: #666;
    font-size: 0.875rem;
}

.required {
    color: #d9534f;
}

.form-actions {
    margin-top: 2rem;
    padding-top: 1rem;
    border-top: 1px solid #ddd;
}

/* Buttons */
.btn {
    display: inline-block;
    padding: 0.5rem 1rem;
    border: 1px solid #ddd;
    border-radius: 4px;
    background: white;
    color: #333;
    text-decoration: none;
    cursor: pointer;
    font-size: 1rem;
    vertical-align: middle;
    line-height: 1.5;
    box-sizing: border-box;
}

.btn:hover {
    background: #f0f0f0;
}

.btn-primary {
    background: #007bff;
    color: white;
    border-color: #007bff;
}

.btn-primary:hover {
    background: #0056b3;
    border-color: #0056b3;
}

.btn-danger {
    background: #d9534f;
    color: white;
    border-color: #d9534f;
}

.btn-danger:hover {
    background: #c9302c;
    border-color: #c9302c;
}

.btn-small {
    padding: 0.25rem 0.5rem;
    font-size: 0.875rem;
}

/* Alerts */
.alert {
    padding: 1rem;
    margin-bottom: 1rem;
    border-radius: 4px;
}

.alert-error {
    background: #f8d7da;
    border: 1px solid #f5c6cb;
    color: #721c24;
}

.alert-info {
    background: #d1ecf1;
    border: 1px solid #bee5eb;
    color: #0c5460;
}

.alert-success {
    background: #d4edda;
    border: 1px solid #c3e6cb;
    color: #155724;
}

/* Profile Page */
.profile-section {
    background: white;
    padding: 1.5rem;
    margin-bottom: 2rem;
    border-radius: 8px;
    box-shadow: 0 1px 3px rgba(0,0,0,0.1);
}

.profile-section h3 {
    margin-bottom: 1rem;
    padding-bottom: 0.5rem;
    border-bottom: 2px solid #f0f0f0;
    color: #444;
}

.info-table {
    width: 100%;
    border-collapse: collapse;
}

.info-table th,
.info-table td {
    padding: 0.75rem;
    text-align: left;
    border-bottom: 1px solid #eee;
}

.info-table th {
    width: 200px;
    font-weight: 600;
    color: #666;
}

.credential {
    background: #f5f5f5;
    padding: 0.5rem 1rem;
    border-radius: 4px;
    font-family: 'Courier New', monospace;
    font-size: 0.9em;
    display: inline-block;
}

.btn-small {
    padding: 0.25rem 0.75rem;
    font-size: 0.85em;
    background: #007bff;
    color: white;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    vertical-align: middle;
    line-height: 1.5;
    box-sizing: border-box;
}

.btn-small:hover {
    background: #0056b3;
}

.example-config {
    margin-top: 1.5rem;
    padding: 1rem;
    background: #f8f9fa;
    border-radius: 4px;
}

.example-config summary {
    cursor: pointer;
    font-weight: 600;
    color: #007bff;
}

.example-config pre {
    margin-top: 1rem;
    background: #2d2d2d;
    padding: 1rem;
    border-radius: 4px;
    overflow-x: auto;
}

.example-config pre code {
    font-family: 'Courier New', monospace;
    font-size: 0.9em;
    color: #f8f8f2 !important;
    background: transparent !important;
    padding: 0 !important;
    display: block;
    line-height: 1.5;
}

.badge-admin {
    background: #007bff;
    color: white;
    padding: 0.25rem 0.75rem;
    border-radius: 4px;
    font-size: 0.85em;
}

/* Admin UI */
.page-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 1.5rem;
}

.actions {
    white-space: nowrap;
}

.badge.admin {
    background: #007bff;
    color: white;
}

.help-text {
    margin-top: 2rem;
    padding-top: 1rem;
    border-top: 1px solid #ddd;
    color: #666;
}

@media (prefers-color-scheme: dark) {
    body {
        background: #1a1a1a;
        color: #e0e0e0;
    }

    main {
        background: #2d2d2d;
    }

    header {
        background: #1a1a1a;
    }

    th {
        background: #3a3a3a;
        color: #e0e0e0;
    }

    tbody tr:hover {
        background: #3a3a3a;
    }

    .directory-row:hover {
        background: #3a3a3a;
    }

    code, .metadata {
        background: #3a3a3a;
    }

    .breadcrumb {
        color: #a0a0a0;
    }

    .page-header h2 {
        color: #e0e0e0;
    }

    .count {
        color: #a0a0a0;
    }

    .login-box {
        background: #2d2d2d;
        border-color: #444;
    }

    .form-group input[type="text"],
    .form-group input[type="password"] {
        background: #3a3a3a;
        border-color: #444;
        color: #e0e0e0;
    }

    .form-group small {
        color: #a0a0a0;
    }

    .form-actions {
        border-top-color: #444;
    }

    .btn {
        background: #3a3a3a;
        border-color: #444;
        color: #e0e0e0;
    }

    .btn:hover {
        background: #4a4a4a;
    }

    .alert-error {
        background: #3a1a1a;
        border-color: #6a2a2a;
        color: #f8d7da;
    }

    .alert-info {
        background: #1a2a3a;
        border-color: #2a4a6a;
        color: #d1ecf1;
    }

    .alert-success {
        background: #1a3a1a;
        border-color: #2a6a2a;
        color: #d4edda;
    }

    .help-text {
        border-top-color: #444;
        color: #a0a0a0;
    }
}
"#;
