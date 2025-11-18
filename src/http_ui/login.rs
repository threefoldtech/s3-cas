use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, header, Request, Response, StatusCode};
use std::sync::Arc;
use tracing;

use crate::metrics::SharedMetrics;

use crate::auth::{SessionStore, UserStore};

use super::{middleware::SessionAuth, responses, templates};

/// Handles GET /login - displays login form or first-time setup
pub async fn handle_login_page(
    req: Request<Incoming>,
    user_store: Arc<UserStore>,
    session_auth: Arc<SessionAuth>,
) -> Response<Full<Bytes>> {
    // Check if already authenticated
    if session_auth.authenticate(&req).is_some() {
        // Already logged in, redirect to buckets
        return Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, "/buckets")
            .body(Full::new(Bytes::from("Redirecting")))
            .unwrap();
    }

    // Check if this is first-time setup (no users in database)
    let user_count = match user_store.count_users() {
        Ok(count) => count,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to count users");
            return responses::html_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                maud::html! {
                    "Database error. Please check server logs."
                }.into_string(),
            );
        }
    };

    if user_count == 0 {
        // First-time setup - show admin creation form
        let error_message = req.uri().query().and_then(|query| {
            for param in query.split('&') {
                if let Some(value) = param.strip_prefix("error=") {
                    return Some(urlencoding::decode(value).unwrap_or_default().to_string());
                }
            }
            None
        });

        return responses::html_response(
            StatusCode::OK,
            templates::setup_admin_page(error_message.as_deref()),
        );
    }

    // Normal login flow
    let uri = req.uri();
    let redirect_to = uri
        .query()
        .and_then(|query| {
            for param in query.split('&') {
                if let Some(value) = param.strip_prefix("redirect=") {
                    return Some(urlencoding::decode(value).unwrap_or_default().to_string());
                }
            }
            None
        })
        .unwrap_or_else(|| "/buckets".to_string());

    let error_message = uri
        .query()
        .and_then(|query| {
            for param in query.split('&') {
                if let Some(value) = param.strip_prefix("error=") {
                    return Some(urlencoding::decode(value).unwrap_or_default().to_string());
                }
            }
            None
        });

    responses::html_response(
        StatusCode::OK,
        templates::login_page(&redirect_to, error_message.as_deref()),
    )
}

/// Handles POST /login - processes login form submission
#[tracing::instrument(skip_all, fields(username, success))]
pub async fn handle_login_submit(
    req: Request<Incoming>,
    user_store: Arc<UserStore>,
    session_store: Arc<SessionStore>,
    session_auth: Arc<SessionAuth>,
    metrics: SharedMetrics,
) -> Response<Full<Bytes>> {
    // Parse form data from request body
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to read request body");
            return redirect_with_error("/login", "Invalid request");
        }
    };

    let body_str = match String::from_utf8(body_bytes.to_vec()) {
        Ok(s) => s,
        Err(_) => {
            return redirect_with_error("/login", "Invalid form data");
        }
    };

    // Parse form fields
    let mut username = None;
    let mut password = None;
    let mut redirect_to = "/buckets".to_string();

    for param in body_str.split('&') {
        if let Some((key, value)) = param.split_once('=') {
            let decoded_value = urlencoding::decode(value).unwrap_or_default().to_string();
            match key {
                "username" => username = Some(decoded_value),
                "password" => password = Some(decoded_value),
                "redirect" => redirect_to = decoded_value,
                _ => {}
            }
        }
    }

    let username = match username {
        Some(u) if !u.is_empty() => u,
        _ => return redirect_with_error("/login", "Username required"),
    };

    tracing::Span::current().record("username", &tracing::field::display(&username));

    let password = match password {
        Some(p) if !p.is_empty() => p,
        _ => return redirect_with_error("/login", "Password required"),
    };

    // Authenticate user
    match user_store.authenticate(&username, &password) {
        Ok(Some(user)) => {
            // Authentication successful - create session
            tracing::Span::current().record("success", true);
            let session_id = session_store.create_session(user.user_id.clone());
            metrics.record_login_attempt(true);
            tracing::info!(
                user_id = %user.user_id,
                username = %username,
                "User logged in successfully"
            );

            // Set session cookie and redirect
            Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, redirect_to)
                .header(header::SET_COOKIE, session_auth.create_session_cookie(&session_id))
                .body(Full::new(Bytes::from("Login successful")))
                .unwrap()
        }
        Ok(None) => {
            // Authentication failed
            tracing::Span::current().record("success", false);
            metrics.record_login_attempt(false);
            tracing::warn!(username = %username, "Login failed: invalid credentials");
            redirect_with_error("/login", "Invalid username or password")
        }
        Err(e) => {
            // Database error
            tracing::Span::current().record("success", false);
            metrics.record_login_attempt(false);
            tracing::warn!(username = %username, error = %e, "Login failed: database error");
            redirect_with_error("/login", "Login error, please try again")
        }
    }
}

/// Handles POST /logout - destroys session and redirects to login
#[tracing::instrument(skip_all, fields(session_id))]
pub async fn handle_logout(
    req: Request<Incoming>,
    session_store: Arc<SessionStore>,
    session_auth: Arc<SessionAuth>,
) -> Response<Full<Bytes>> {
    // Extract session ID from cookie
    if let Some(session_id) = extract_session_id_from_request(&req) {
        tracing::Span::current().record("session_id", &tracing::field::display(&session_id));
        session_store.delete_session(&session_id);
        tracing::info!(session_id = %session_id, "User logged out");
    }

    // Clear cookie and redirect to login
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, "/login")
        .header(header::SET_COOKIE, session_auth.clear_session_cookie())
        .body(Full::new(Bytes::from("Logged out")))
        .unwrap()
}

/// Handles POST /setup-admin - creates the first admin user
pub async fn handle_setup_admin(
    req: Request<Incoming>,
    user_store: Arc<UserStore>,
    session_store: Arc<SessionStore>,
    session_auth: Arc<SessionAuth>,
    metrics: SharedMetrics,
) -> Response<Full<Bytes>> {
    use crate::auth::UserRecord;

    // Check if database already has users (prevent duplicate setup)
    match user_store.count_users() {
        Ok(count) if count > 0 => {
            tracing::warn!("Attempted to run setup when users already exist");
            return redirect_with_error("/login", "Setup already completed");
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to count users during setup");
            return redirect_with_error("/login", "Database error");
        }
        _ => {}
    }

    // Parse form data
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to read setup form body");
            return redirect_with_error("/login", "Invalid request");
        }
    };

    let body_str = match String::from_utf8(body_bytes.to_vec()) {
        Ok(s) => s,
        Err(_) => {
            return redirect_with_error("/login", "Invalid form data");
        }
    };

    // Parse form fields
    let mut ui_login = None;
    let mut password = None;
    let mut confirm_password = None;

    for param in body_str.split('&') {
        if let Some((key, value)) = param.split_once('=') {
            let decoded_value = urlencoding::decode(value).unwrap_or_default().to_string();
            match key {
                "ui_login" => ui_login = Some(decoded_value),
                "password" => password = Some(decoded_value),
                "confirm_password" => confirm_password = Some(decoded_value),
                _ => {}
            }
        }
    }

    // Validate inputs
    let ui_login = match ui_login {
        Some(u) if !u.is_empty() => u,
        _ => return redirect_with_error("/login", "Username required"),
    };

    let password = match password {
        Some(p) if !p.is_empty() => p,
        _ => return redirect_with_error("/login", "Password required"),
    };

    let confirm_password = match confirm_password {
        Some(p) => p,
        None => return redirect_with_error("/login", "Password confirmation required"),
    };

    if password != confirm_password {
        return redirect_with_error("/login", "Passwords do not match");
    }

    if password.len() < 8 {
        return redirect_with_error("/login", "Password must be at least 8 characters");
    }

    // Generate S3 credentials
    let s3_access_key = generate_access_key();
    let s3_secret_key = generate_secret_key();

    // Create admin user
    let user_id = ui_login.clone();  // user_id = ui_login for simplicity
    let user_record = match UserRecord::new(
        user_id.clone(),
        ui_login.clone(),
        &password,
        s3_access_key.clone(),
        s3_secret_key.clone(),
        true,  // is_admin = true
    ) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to create user record");
            return redirect_with_error("/login", "Failed to create admin account");
        }
    };

    // Store user in database
    if let Err(e) = user_store.create_user(user_record) {
        tracing::warn!(error = %e, "Failed to store admin user");
        return redirect_with_error("/login", "Failed to create admin account");
    }

    metrics.record_admin_operation("user_create");
    tracing::info!(
        user_id = %ui_login,
        is_admin = true,
        "Admin user created successfully during first-time setup"
    );

    // Create session for immediate login
    let session_id = session_store.create_session(user_id.clone());

    // Redirect to profile page to view S3 credentials
    // Store credentials in query params (shown once)
    let redirect_url = format!(
        "/profile?setup=1&access_key={}&secret_key={}",
        urlencoding::encode(&s3_access_key),
        urlencoding::encode(&s3_secret_key)
    );

    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, redirect_url)
        .header(header::SET_COOKIE, session_auth.create_session_cookie(&session_id))
        .body(Full::new(Bytes::from("Setup complete")))
        .unwrap()
}

/// Generates a random S3 access key (20 characters, uppercase alphanumeric)
fn generate_access_key() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..20)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

/// Generates a random S3 secret key (40 characters, alphanumeric + +/)
fn generate_secret_key() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut rng = rand::thread_rng();
    (0..40)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

/// Helper to extract session ID from request cookies
fn extract_session_id_from_request(req: &Request<Incoming>) -> Option<String> {
    use cookie::Cookie;

    let cookie_header = req.headers().get(header::COOKIE)?;
    let cookie_str = cookie_header.to_str().ok()?;

    for cookie_pair in cookie_str.split(';') {
        if let Ok(cookie) = Cookie::parse(cookie_pair.trim()) {
            if cookie.name() == super::middleware::SESSION_COOKIE_NAME {
                return Some(cookie.value().to_string());
            }
        }
    }

    None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redirect_with_error() {
        let response = redirect_with_error("/login", "Invalid credentials");
        assert_eq!(response.status(), StatusCode::FOUND);

        let location = response.headers().get(header::LOCATION).unwrap();
        assert!(location.to_str().unwrap().contains("error="));
        assert!(location.to_str().unwrap().contains("Invalid"));
    }
}
