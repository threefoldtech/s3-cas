use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, header, Request, Response, StatusCode};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::auth::{SessionStore, UserStore};

use super::{responses, templates, SessionAuth};

/// Handles GET /profile - displays user profile with S3 credentials
pub async fn handle_profile_page(
    user_id: String,
    user_store: Arc<UserStore>,
    req: Request<Incoming>,
) -> Response<Full<Bytes>> {
    // Extract query parameters
    let query = req.uri().query();

    let error_message = query.and_then(|q| {
        q.split('&')
            .find(|p| p.starts_with("error="))
            .and_then(|p| p.strip_prefix("error="))
            .map(|e| urlencoding::decode(e).unwrap_or_default().to_string())
    });

    // Check if this is coming from first-time setup
    let is_setup = query
        .and_then(|q| q.split('&').find(|p| *p == "setup=1"))
        .is_some();

    match user_store.get_user_by_id(&user_id) {
        Ok(Some(user)) => {
            responses::html_response(
                StatusCode::OK,
                templates::profile_page(&user, error_message.as_deref(), is_setup),
            )
        }
        Ok(None) => {
            warn!("User not found: {}", user_id);
            responses::html_response(
                StatusCode::NOT_FOUND,
                templates::error_page("User not found"),
            )
        }
        Err(e) => {
            warn!("Failed to get user: {}", e);
            responses::html_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                templates::error_page("Failed to load profile"),
            )
        }
    }
}

/// Handles POST /profile/password - changes user password
pub async fn handle_change_password(
    user_id: String,
    req: Request<Incoming>,
    user_store: Arc<UserStore>,
    session_store: Arc<SessionStore>,
    session_auth: Arc<SessionAuth>,
) -> Response<Full<Bytes>> {
    // Parse form data
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            warn!("Failed to read request body: {}", e);
            return redirect_with_error("/profile", "Invalid request");
        }
    };

    let body_str = match std::str::from_utf8(&body_bytes) {
        Ok(s) => s,
        Err(e) => {
            warn!("Invalid UTF-8 in request body: {}", e);
            return redirect_with_error("/profile", "Invalid request");
        }
    };

    // Parse form fields
    let mut current_password = None;
    let mut new_password = None;
    let mut confirm_password = None;

    for pair in body_str.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let decoded_value = urlencoding::decode(value).unwrap_or_default();
            match key {
                "current_password" => current_password = Some(decoded_value.to_string()),
                "new_password" => new_password = Some(decoded_value.to_string()),
                "confirm_password" => confirm_password = Some(decoded_value.to_string()),
                _ => {}
            }
        }
    }

    let current_password = match current_password {
        Some(p) if !p.is_empty() => p,
        _ => return redirect_with_error("/profile", "Current password is required"),
    };

    let new_password = match new_password {
        Some(p) if !p.is_empty() => p,
        _ => return redirect_with_error("/profile", "New password is required"),
    };

    let confirm_password = match confirm_password {
        Some(p) => p,
        None => return redirect_with_error("/profile", "Password confirmation is required"),
    };

    // Verify passwords match
    if new_password != confirm_password {
        return redirect_with_error("/profile", "New passwords do not match");
    }

    // Verify current password
    match user_store.verify_password(&user_id, &current_password) {
        Ok(true) => {}
        Ok(false) => {
            return redirect_with_error("/profile", "Current password is incorrect");
        }
        Err(e) => {
            warn!("Failed to verify password: {}", e);
            return redirect_with_error("/profile", "Failed to verify password");
        }
    }

    // Update password
    match user_store.update_password(&user_id, &new_password) {
        Ok(()) => {
            debug!("Password changed for user: {}", user_id);

            // Invalidate all sessions for this user (force re-login)
            session_store.delete_user_sessions(&user_id);

            // Redirect to login with success message
            Response::builder()
                .status(StatusCode::SEE_OTHER)
                .header(header::LOCATION, "/login?message=password_changed")
                .header(
                    header::SET_COOKIE,
                    session_auth.clear_session_cookie(),
                )
                .body(Full::new(Bytes::new()))
                .unwrap()
        }
        Err(e) => {
            warn!("Failed to update password: {}", e);
            redirect_with_error("/profile", "Failed to update password")
        }
    }
}

fn redirect_with_error(path: &str, message: &str) -> Response<Full<Bytes>> {
    let url = format!("{}?error={}", path, urlencoding::encode(message));
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::LOCATION, url)
        .body(Full::new(Bytes::new()))
        .unwrap()
}
