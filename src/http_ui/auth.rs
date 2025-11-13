use bytes::Bytes;
use http_body_util::Full;
use hyper::{header, Request, Response, StatusCode};

/// Basic authentication handler
#[derive(Clone)]
pub struct BasicAuth {
    username: String,
    password: String,
}

impl BasicAuth {
    pub fn new(username: String, password: String) -> Self {
        Self { username, password }
    }

    /// Check if request has valid authentication
    pub fn check_auth(&self, req: &Request<hyper::body::Incoming>) -> bool {
        let auth_header = match req.headers().get(header::AUTHORIZATION) {
            Some(header) => header,
            None => return false,
        };

        let auth_str = match auth_header.to_str() {
            Ok(s) => s,
            Err(_) => return false,
        };

        if !auth_str.starts_with("Basic ") {
            return false;
        }

        let encoded = &auth_str[6..];
        let decoded = match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };

        let credentials = match String::from_utf8(decoded) {
            Ok(s) => s,
            Err(_) => return false,
        };

        let parts: Vec<&str> = credentials.splitn(2, ':').collect();
        if parts.len() != 2 {
            return false;
        }

        parts[0] == self.username && parts[1] == self.password
    }

    /// Return 401 response with WWW-Authenticate header
    pub fn auth_required_response(&self) -> Response<Full<Bytes>> {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"s3-cas\"")
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Full::new(Bytes::from("Authentication required")))
            .unwrap()
    }
}
