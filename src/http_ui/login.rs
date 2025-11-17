use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, header, Request, Response, StatusCode};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::auth::{SessionStore, UserStore};

use super::{middleware::SessionAuth, responses, templates};

/// Handles GET /login - displays login form
pub async fn handle_login_page(
    req: Request<Incoming>,
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

    // Extract redirect parameter from query string
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
pub async fn handle_login_submit(
    req: Request<Incoming>,
    user_store: Arc<UserStore>,
    session_store: Arc<SessionStore>,
    session_auth: Arc<SessionAuth>,
) -> Response<Full<Bytes>> {
    // Parse form data from request body
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            warn!("Failed to read request body: {}", e);
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

    let password = match password {
        Some(p) if !p.is_empty() => p,
        _ => return redirect_with_error("/login", "Password required"),
    };

    // Authenticate user
    match user_store.authenticate(&username, &password) {
        Ok(Some(user)) => {
            // Authentication successful - create session
            let session_id = session_store.create_session(user.user_id.clone());
            debug!("User {} logged in successfully", user.user_id);

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
            warn!("Login failed for user: {}", username);
            redirect_with_error("/login", "Invalid username or password")
        }
        Err(e) => {
            // Database error
            warn!("Login error for user {}: {}", username, e);
            redirect_with_error("/login", "Login error, please try again")
        }
    }
}

/// Handles POST /logout - destroys session and redirects to login
pub async fn handle_logout(
    req: Request<Incoming>,
    session_store: Arc<SessionStore>,
    session_auth: Arc<SessionAuth>,
) -> Response<Full<Bytes>> {
    // Extract session ID from cookie
    if let Some(session_id) = extract_session_id_from_request(&req) {
        session_store.delete_session(&session_id);
        debug!("Session {} logged out", session_id);
    }

    // Clear cookie and redirect to login
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, "/login")
        .header(header::SET_COOKIE, session_auth.clear_session_cookie())
        .body(Full::new(Bytes::from("Logged out")))
        .unwrap()
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
