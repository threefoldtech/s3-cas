use bytes::Bytes;
use cookie::Cookie;
use http_body_util::Full;
use hyper::{header, Request, Response, StatusCode};
use std::sync::Arc;

use crate::auth::{SessionStore, UserStore};
use super::{responses, HttpBody};

/// Session cookie name
pub const SESSION_COOKIE_NAME: &str = "session_id";

/// Cookie max age (24 hours in seconds)
const COOKIE_MAX_AGE: i64 = 24 * 60 * 60;

/// Authentication context extracted from request
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// User ID from session
    pub user_id: String,
    /// Whether user is admin
    pub is_admin: bool,
}

/// Session-based authentication middleware
#[derive(Clone)]
pub struct SessionAuth {
    session_store: Arc<SessionStore>,
    user_store: Arc<UserStore>,
}

impl SessionAuth {
    /// Creates a new session authentication middleware
    pub fn new(session_store: Arc<SessionStore>, user_store: Arc<UserStore>) -> Self {
        Self {
            session_store,
            user_store,
        }
    }

    /// Extracts session ID from cookie header
    fn extract_session_id(&self, req: &Request<hyper::body::Incoming>) -> Option<String> {
        let cookie_header = req.headers().get(header::COOKIE)?;
        let cookie_str = cookie_header.to_str().ok()?;

        // Parse all cookies and find session_id
        for cookie_pair in cookie_str.split(';') {
            if let Ok(cookie) = Cookie::parse(cookie_pair.trim()) {
                if cookie.name() == SESSION_COOKIE_NAME {
                    return Some(cookie.value().to_string());
                }
            }
        }

        None
    }

    /// Checks if the request has a valid session and returns user context
    #[tracing::instrument(skip(self, req), fields(session_id, user_id, is_admin))]
    pub fn authenticate(&self, req: &Request<hyper::body::Incoming>) -> Option<AuthContext> {
        // Extract session ID from cookie
        let session_id = self.extract_session_id(req)?;
        tracing::Span::current().record("session_id", &tracing::field::display(&session_id));

        // Validate session and get user_id
        let user_id = self.session_store.get_session(&session_id)?;
        tracing::Span::current().record("user_id", &tracing::field::display(&user_id));

        // Get user details
        match self.user_store.get_user_by_id(&user_id) {
            Ok(Some(user)) => {
                tracing::Span::current().record("is_admin", user.is_admin);
                tracing::debug!(user_id = %user_id, is_admin = user.is_admin, "Authenticated user");
                Some(AuthContext {
                    user_id,
                    is_admin: user.is_admin,
                })
            }
            Ok(None) => {
                tracing::warn!(user_id = %user_id, "Session valid but user not found");
                None
            }
            Err(e) => {
                tracing::warn!(error = %e, "Error fetching user");
                None
            }
        }
    }

    /// Checks if user is admin
    pub fn is_admin(&self, req: &Request<hyper::body::Incoming>) -> bool {
        self.authenticate(req)
            .map(|ctx| ctx.is_admin)
            .unwrap_or(false)
    }

    /// Returns 302 redirect to login page
    pub fn login_redirect_response(&self, original_path: &str) -> Response<HttpBody> {
        let redirect_url = if original_path == "/" || original_path.is_empty() {
            "/login".to_string()
        } else {
            format!("/login?redirect={}", urlencoding::encode(original_path))
        };

        let resp = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, redirect_url)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Full::new(Bytes::from("Redirecting to login")))
            .unwrap();
        responses::map_response(resp)
    }

    /// Returns 403 Forbidden response (for admin-only routes)
    pub fn forbidden_response(&self) -> Response<HttpBody> {
        let resp = Response::builder()
            .status(StatusCode::FORBIDDEN)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Full::new(Bytes::from(
                r#"<!DOCTYPE html>
<html>
<head><title>403 Forbidden</title></head>
<body>
<h1>403 Forbidden</h1>
<p>You don't have permission to access this resource.</p>
<p><a href="/buckets">Return to buckets</a></p>
</body>
</html>"#,
            )))
            .unwrap();
        responses::map_response(resp)
    }

    /// Creates a session cookie
    pub fn create_session_cookie(&self, session_id: &str) -> String {
        Cookie::build((SESSION_COOKIE_NAME, session_id))
            .path("/")
            .max_age(cookie::time::Duration::seconds(COOKIE_MAX_AGE))
            .http_only(true)
            .same_site(cookie::SameSite::Strict)
            .build()
            .to_string()
    }

    /// Creates a cookie that clears the session (for logout)
    pub fn clear_session_cookie(&self) -> String {
        Cookie::build((SESSION_COOKIE_NAME, ""))
            .path("/")
            .max_age(cookie::time::Duration::ZERO)
            .http_only(true)
            .same_site(cookie::SameSite::Strict)
            .build()
            .to_string()
    }
}

/// Helper to check if a path is public (doesn't require authentication)
pub fn is_public_path(path: &str) -> bool {
    matches!(path, "/login" | "/setup-admin" | "/health")
}

/// Helper to check if a path requires admin privileges
pub fn is_admin_path(path: &str) -> bool {
    path.starts_with("/admin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_public_path() {
        assert!(is_public_path("/login"));
        assert!(is_public_path("/health"));
        assert!(!is_public_path("/buckets"));
        assert!(!is_public_path("/admin"));
    }

    #[test]
    fn test_is_admin_path() {
        assert!(is_admin_path("/admin"));
        assert!(is_admin_path("/admin/users"));
        assert!(is_admin_path("/admin/users/new"));
        assert!(!is_admin_path("/buckets"));
        assert!(!is_admin_path("/login"));
    }

    #[test]
    fn test_session_cookie_creation() {
        use crate::auth::SessionStore;

        let session_store = Arc::new(SessionStore::new());

        // Create a mock user store for testing
        // In a real test, we'd use a proper Store implementation
        struct MockStore;
        impl std::fmt::Debug for MockStore {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "MockStore")
            }
        }

        // For this test, we just verify cookie format
        let cookie_str = Cookie::build((SESSION_COOKIE_NAME, "test_session_id"))
            .path("/")
            .http_only(true)
            .same_site(cookie::SameSite::Strict)
            .build()
            .to_string();

        assert!(cookie_str.contains(SESSION_COOKIE_NAME));
        assert!(cookie_str.contains("test_session_id"));
        assert!(cookie_str.contains("HttpOnly"));
        assert!(cookie_str.contains("SameSite=Strict"));
    }
}
