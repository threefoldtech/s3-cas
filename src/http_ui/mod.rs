mod auth;
mod handlers;
mod responses;
mod templates;

pub use auth::BasicAuth;

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Method, Request, Response, StatusCode};

use crate::cas::CasFS;
use crate::metrics::SharedMetrics;

/// HTTP UI service for browsing CAS storage
#[derive(Clone)]
pub struct HttpUiService {
    casfs: Arc<CasFS>,
    #[allow(dead_code)]
    metrics: Arc<SharedMetrics>,
    auth: Option<BasicAuth>,
}

impl HttpUiService {
    pub fn new(casfs: CasFS, metrics: SharedMetrics, auth: Option<BasicAuth>) -> Self {
        Self {
            casfs: Arc::new(casfs),
            metrics: Arc::new(metrics),
            auth,
        }
    }

    /// Main request handler
    pub async fn handle_request(
        &self,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
        // Check authentication if enabled
        if let Some(ref auth) = self.auth {
            if !auth.check_auth(&req) {
                return Ok(auth.auth_required_response());
            }
        }

        let result = self.route_request(req).await;
        Ok(result)
    }

    async fn route_request(&self, req: Request<hyper::body::Incoming>) -> Response<Full<Bytes>> {
        let path = req.uri().path();
        let method = req.method();
        let wants_html = self.wants_html(&req);

        match (method, path) {
            (&Method::GET, "/") => self.handle_root(wants_html).await,
            (&Method::GET, "/health") => self.handle_health().await,
            (&Method::GET, "/api/v1/buckets") => handlers::list_buckets(&self.casfs, false).await,
            (&Method::GET, "/buckets") => handlers::list_buckets(&self.casfs, wants_html).await,
            (&Method::GET, path) if path.starts_with("/buckets/") => {
                self.handle_bucket_path(path, wants_html, &req).await
            }
            (&Method::GET, path) if path.starts_with("/api/v1/buckets/") => {
                self.handle_api_path(path, &req).await
            }
            _ => responses::not_found(wants_html),
        }
    }

    fn wants_html(&self, req: &Request<hyper::body::Incoming>) -> bool {
        // Check query parameter first
        if let Some(query) = req.uri().query() {
            if query.contains("format=json") {
                return false;
            }
            if query.contains("format=html") {
                return true;
            }
        }

        // Check Accept header
        if let Some(accept) = req.headers().get("accept") {
            if let Ok(accept_str) = accept.to_str() {
                if accept_str.contains("text/html") {
                    return true;
                }
                if accept_str.contains("application/json") {
                    return false;
                }
            }
        }

        // Default: HTML for non-API paths
        !req.uri().path().starts_with("/api/")
    }

    async fn handle_root(&self, wants_html: bool) -> Response<Full<Bytes>> {
        if wants_html {
            Response::builder()
                .status(StatusCode::MOVED_PERMANENTLY)
                .header("location", "/buckets")
                .body(Full::new(Bytes::new()))
                .unwrap()
        } else {
            let info = serde_json::json!({
                "name": "s3-cas HTTP API",
                "version": env!("CARGO_PKG_VERSION"),
                "endpoints": {
                    "/buckets": "List all buckets",
                    "/buckets/{bucket}": "List objects in bucket",
                    "/buckets/{bucket}/{key}": "Get object metadata",
                    "/api/v1/buckets": "List buckets (JSON)",
                    "/api/v1/buckets/{bucket}": "List objects (JSON)",
                    "/api/v1/buckets/{bucket}/objects/{key}": "Object metadata (JSON)",
                    "/health": "Health check"
                }
            });
            responses::json_response(StatusCode::OK, &info)
        }
    }

    async fn handle_health(&self) -> Response<Full<Bytes>> {
        let health = serde_json::json!({
            "status": "healthy",
            "storage": "operational"
        });
        responses::json_response(StatusCode::OK, &health)
    }

    async fn handle_bucket_path(
        &self,
        path: &str,
        wants_html: bool,
        req: &Request<hyper::body::Incoming>,
    ) -> Response<Full<Bytes>> {
        let path_parts: Vec<&str> = path
            .trim_start_matches("/buckets/")
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        match path_parts.as_slice() {
            [bucket] => handlers::list_objects(&self.casfs, bucket, req, wants_html).await,
            [bucket, key @ ..] => {
                let object_key = key.join("/");
                handlers::object_metadata(&self.casfs, bucket, &object_key, wants_html).await
            }
            _ => responses::error_response(StatusCode::BAD_REQUEST, "Invalid path", wants_html),
        }
    }

    async fn handle_api_path(
        &self,
        path: &str,
        req: &Request<hyper::body::Incoming>,
    ) -> Response<Full<Bytes>> {
        let path_parts: Vec<&str> = path
            .trim_start_matches("/api/v1/buckets/")
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        match path_parts.as_slice() {
            [bucket] => handlers::list_objects(&self.casfs, bucket, req, false).await,
            [bucket, "objects", key @ ..] => {
                let object_key = key.join("/");
                handlers::object_metadata(&self.casfs, bucket, &object_key, false).await
            }
            _ => responses::error_response(StatusCode::BAD_REQUEST, "Invalid API path", false),
        }
    }
}
