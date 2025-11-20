use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, header, Request, Response, StatusCode};
use rand::Rng;
use std::sync::Arc;
use tracing;

use crate::auth::{SessionStore, UserRecord, UserStore};
use crate::metrics::SharedMetrics;

use super::{responses, templates};

/// Generates a random S3 access key (20 characters, alphanumeric uppercase)
fn generate_access_key() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..20)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Generates a random S3 secret key (40 characters, alphanumeric + special chars)
fn generate_secret_key() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut rng = rand::thread_rng();
    (0..40)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Generates a random password (16 characters, alphanumeric)
fn generate_password() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Handles GET /admin/users - lists all users
pub async fn handle_list_users(user_store: Arc<UserStore>) -> Response<Full<Bytes>> {
    match user_store.list_users() {
        Ok(users) => {
            responses::html_response(StatusCode::OK, templates::admin_users_page(&users))
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to list users");
            responses::html_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                templates::error_page("Failed to list users"),
            )
        }
    }
}

/// Handles GET /admin/users/new - displays user creation form
pub async fn handle_new_user_form() -> Response<Full<Bytes>> {
    responses::html_response(StatusCode::OK, templates::new_user_form())
}

/// Handles POST /admin/users - creates a new user
pub async fn handle_create_user(
    req: Request<Incoming>,
    user_store: Arc<UserStore>,
    metrics: SharedMetrics,
) -> Response<Full<Bytes>> {
    // Parse form data
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to read request body");
            return redirect_with_error("/admin/users", "Invalid request");
        }
    };

    let body_str = match String::from_utf8(body_bytes.to_vec()) {
        Ok(s) => s,
        Err(_) => return redirect_with_error("/admin/users", "Invalid form data"),
    };

    // Parse form fields
    let mut user_id = None;
    let mut ui_login = None;
    let mut ui_password = None;
    let mut s3_access_key = None;
    let mut s3_secret_key = None;
    let mut is_admin = false;

    for param in body_str.split('&') {
        if let Some((key, value)) = param.split_once('=') {
            let decoded_value = urlencoding::decode(value).unwrap_or_default().to_string();
            match key {
                "user_id" => user_id = Some(decoded_value),
                "ui_login" => ui_login = Some(decoded_value),
                "ui_password" => ui_password = Some(decoded_value),
                "s3_access_key" => s3_access_key = Some(decoded_value),
                "s3_secret_key" => s3_secret_key = Some(decoded_value),
                "is_admin" => is_admin = decoded_value == "on" || decoded_value == "true",
                _ => {}
            }
        }
    }

    // Validate required fields
    let user_id = match user_id {
        Some(id) if !id.is_empty() => id,
        _ => return redirect_with_error("/admin/users/new", "User ID is required"),
    };

    let ui_login = match ui_login {
        Some(login) if !login.is_empty() => login,
        _ => return redirect_with_error("/admin/users/new", "UI login is required"),
    };

    // Generate password if not provided
    let ui_password = match ui_password {
        Some(pw) if !pw.is_empty() => pw,
        _ => generate_password(),
    };

    // Generate S3 keys if not provided
    let s3_access_key = match s3_access_key {
        Some(key) if !key.is_empty() => key,
        _ => generate_access_key(),
    };

    let s3_secret_key = match s3_secret_key {
        Some(key) if !key.is_empty() => key,
        _ => generate_secret_key(),
    };

    // Create user record
    let user = match UserRecord::new(
        user_id.clone(),
        ui_login,
        &ui_password,
        s3_access_key.clone(),
        s3_secret_key.clone(),
        is_admin,
    ) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to create user record");
            return redirect_with_error("/admin/users/new", "Failed to create user");
        }
    };

    // Store user in database
    match user_store.create_user(user) {
        Ok(_) => {
            metrics.record_admin_operation("user_create");
            tracing::info!(
                user_id = %user_id,
                is_admin = is_admin,
                "User created via admin panel"
            );
            // Redirect to users list with success message showing the credentials
            let message = format!(
                "User created: {} | Password: {} | S3 Key: {} | S3 Secret: {}",
                user_id, ui_password, s3_access_key, s3_secret_key
            );
            redirect_with_success("/admin/users", &message)
        }
        Err(e) => {
            tracing::warn!(error = %e, user_id = %user_id, "Failed to store user");
            redirect_with_error("/admin/users/new", &format!("Failed to create user: {}", e))
        }
    }
}

/// Handles DELETE /admin/users/{user_id} - deletes a user
pub async fn handle_delete_user(
    user_id: &str,
    user_store: Arc<UserStore>,
    session_store: Arc<SessionStore>,
    metrics: SharedMetrics,
) -> Response<Full<Bytes>> {
    // Delete all sessions for this user
    session_store.delete_user_sessions(user_id);

    // Delete user from database
    match user_store.delete_user(user_id) {
        Ok(_) => {
            metrics.record_admin_operation("user_delete");
            tracing::info!(user_id = %user_id, "User deleted via admin panel");
            redirect_with_success("/admin/users", &format!("User '{}' deleted", user_id))
        }
        Err(e) => {
            tracing::warn!(error = %e, user_id = %user_id, "Failed to delete user");
            redirect_with_error("/admin/users", &format!("Failed to delete user: {}", e))
        }
    }
}

/// Handles GET /admin/users/{user_id}/reset-password - displays password reset form
pub async fn handle_reset_password_form(
    user_id: &str,
    user_store: Arc<UserStore>,
) -> Response<Full<Bytes>> {
    match user_store.get_user_by_id(user_id) {
        Ok(Some(user)) => {
            responses::html_response(StatusCode::OK, templates::reset_password_form(&user))
        }
        Ok(None) => responses::html_response(
            StatusCode::NOT_FOUND,
            templates::error_page(&format!("User '{}' not found", user_id)),
        ),
        Err(e) => {
            tracing::warn!(error = %e, user_id = %user_id, "Failed to get user");
            responses::html_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                templates::error_page("Failed to load user"),
            )
        }
    }
}

/// Handles POST /admin/users/{user_id}/password - updates user password
pub async fn handle_update_password(
    user_id: &str,
    req: Request<Incoming>,
    user_store: Arc<UserStore>,
    session_store: Arc<SessionStore>,
    metrics: SharedMetrics,
) -> Response<Full<Bytes>> {
    // Parse form data
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to read request body");
            return redirect_with_error("/admin/users", "Invalid request");
        }
    };

    let body_str = match String::from_utf8(body_bytes.to_vec()) {
        Ok(s) => s,
        Err(_) => return redirect_with_error("/admin/users", "Invalid form data"),
    };

    // Parse password field
    let mut new_password = None;
    for param in body_str.split('&') {
        if let Some((key, value)) = param.split_once('=') {
            if key == "new_password" {
                new_password = Some(urlencoding::decode(value).unwrap_or_default().to_string());
                break;
            }
        }
    }

    let new_password = match new_password {
        Some(pw) if !pw.is_empty() => pw,
        _ => return redirect_with_error(&format!("/admin/users/{}/reset-password", user_id), "Password is required"),
    };

    // Update password
    match user_store.update_password(user_id, &new_password) {
        Ok(_) => {
            metrics.record_admin_operation("password_reset");
            tracing::info!(user_id = %user_id, "Password updated via admin panel");
            // Invalidate all sessions for this user
            session_store.delete_user_sessions(user_id);
            redirect_with_success("/admin/users", &format!("Password updated for user '{}'", user_id))
        }
        Err(e) => {
            tracing::warn!(error = %e, user_id = %user_id, "Failed to update password");
            redirect_with_error("/admin/users", &format!("Failed to update password: {}", e))
        }
    }
}

/// Helper to create a redirect response with error message
fn redirect_with_error(location: &str, error: &str) -> Response<Full<Bytes>> {
    let redirect_url = format!("{}?error={}", location, urlencoding::encode(error));

    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, redirect_url)
        .body(Full::new(Bytes::from("Redirecting")))
        .unwrap()
}

/// Handles POST /admin/users/{user_id}/toggle-admin - toggles admin status
pub async fn handle_toggle_admin(
    user_id: &str,
    current_user_id: &str,
    user_store: Arc<UserStore>,
    metrics: SharedMetrics,
) -> Response<Full<Bytes>> {
    // Prevent users from removing their own admin rights
    if user_id == current_user_id {
        return redirect_with_error("/admin/users", "You cannot modify your own admin rights");
    }

    // Get current user
    let user = match user_store.get_user_by_id(user_id) {
        Ok(Some(u)) => u,
        Ok(None) => {
            return redirect_with_error("/admin/users", &format!("User '{}' not found", user_id));
        }
        Err(e) => {
            tracing::warn!(error = %e, user_id = %user_id, "Failed to get user");
            return redirect_with_error("/admin/users", "Failed to get user");
        }
    };

    // Toggle admin status
    let new_status = !user.is_admin;
    let action = if new_status { "granted" } else { "revoked" };
    let metric_operation = if new_status { "admin_grant" } else { "admin_revoke" };

    match user_store.update_admin_status(user_id, new_status) {
        Ok(_) => {
            metrics.record_admin_operation(metric_operation);
            tracing::info!(
                user_id = %user_id,
                is_admin = new_status,
                action = action,
                "Admin rights {} via admin panel", action
            );
            redirect_with_success(
                "/admin/users",
                &format!("Admin rights {} for user '{}'", action, user_id),
            )
        }
        Err(e) => {
            tracing::warn!(error = %e, user_id = %user_id, "Failed to update admin status");
            redirect_with_error("/admin/users", &format!("Failed to update admin status: {}", e))
        }
    }
}

/// Helper to create a redirect response with success message
fn redirect_with_success(location: &str, message: &str) -> Response<Full<Bytes>> {
    let redirect_url = format!("{}?success={}", location, urlencoding::encode(message));

    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, redirect_url)
        .body(Full::new(Bytes::from("Redirecting")))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_access_key() {
        let key1 = generate_access_key();
        let key2 = generate_access_key();

        assert_eq!(key1.len(), 20);
        assert_eq!(key2.len(), 20);
        assert_ne!(key1, key2); // Should be random
        assert!(key1.chars().all(|c| c.is_ascii_alphanumeric() && c.is_ascii_uppercase() || c.is_ascii_digit()));
    }

    #[test]
    fn test_generate_secret_key() {
        let key = generate_secret_key();
        assert_eq!(key.len(), 40);
    }

    #[test]
    fn test_generate_password() {
        let pw1 = generate_password();
        let pw2 = generate_password();

        assert_eq!(pw1.len(), 16);
        assert_eq!(pw2.len(), 16);
        assert_ne!(pw1, pw2); // Should be random
    }
}
