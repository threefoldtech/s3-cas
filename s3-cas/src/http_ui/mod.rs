mod admin;
mod auth;
mod handlers;
mod login;
mod middleware;
mod profile;
mod responses;
mod templates;

pub use auth::BasicAuth;
pub use middleware::SessionAuth;

// Re-export the main service types
pub use HttpUiServiceEnum as HttpUiServiceWrapper;

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Method, Request, Response, StatusCode};

use cas_storage::CasFS;
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
            (&Method::GET, "/api/v1/buckets") => handlers::list_buckets(&self.casfs, false, None).await,
            (&Method::GET, "/buckets") => handlers::list_buckets(&self.casfs, wants_html, None).await,
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

use crate::auth::{SessionStore, UserRouter, UserStore};

/// HTTP UI service for multi-user mode with session-based authentication
#[derive(Clone)]
pub struct HttpUiServiceMultiUser {
    user_router: Arc<UserRouter>,
    user_store: Arc<UserStore>,
    session_store: Arc<SessionStore>,
    session_auth: Arc<SessionAuth>,
    #[allow(dead_code)]
    metrics: SharedMetrics,
}

impl HttpUiServiceMultiUser {
    pub fn new(
        user_router: Arc<UserRouter>,
        user_store: Arc<UserStore>,
        session_store: Arc<SessionStore>,
        metrics: SharedMetrics,
    ) -> Self {
        let session_auth = Arc::new(SessionAuth::new(
            session_store.clone(),
            user_store.clone(),
        ));

        Self {
            user_router,
            user_store,
            session_store,
            session_auth,
            metrics,
        }
    }

    /// Main request handler
    pub async fn handle_request(
        &self,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
        let result = self.route_request(req).await;
        Ok(result)
    }

    async fn route_request(&self, req: Request<hyper::body::Incoming>) -> Response<Full<Bytes>> {
        let path = req.uri().path().to_string();
        let method = req.method().clone();

        // Public routes (no auth required)
        if middleware::is_public_path(&path) {
            return match (&method, path.as_str()) {
                (&Method::GET, "/login") => {
                    login::handle_login_page(req, self.user_store.clone(), self.session_auth.clone()).await
                }
                (&Method::POST, "/login") => {
                    login::handle_login_submit(
                        req,
                        self.user_store.clone(),
                        self.session_store.clone(),
                        self.session_auth.clone(),
                        self.metrics.clone(),
                    )
                    .await
                }
                (&Method::POST, "/setup-admin") => {
                    login::handle_setup_admin(
                        req,
                        self.user_store.clone(),
                        self.session_store.clone(),
                        self.session_auth.clone(),
                        self.metrics.clone(),
                    )
                    .await
                }
                (&Method::POST, "/logout") => {
                    login::handle_logout(req, self.session_store.clone(), self.session_auth.clone()).await
                }
                (&Method::GET, "/health") => self.handle_health().await,
                _ => responses::not_found(true),
            };
        }

        // Protected routes - require authentication
        let auth_context = match self.session_auth.authenticate(&req) {
            Some(ctx) => ctx,
            None => {
                // Not authenticated - redirect to login
                return self.session_auth.login_redirect_response(&path);
            }
        };

        // Admin routes
        if middleware::is_admin_path(&path) {
            if !auth_context.is_admin {
                return self.session_auth.forbidden_response();
            }

            return self.handle_admin_request(req, &auth_context.user_id, &path, &method).await;
        }

        // Regular authenticated routes
        self.handle_authenticated_request(req, &auth_context.user_id, auth_context.is_admin, &path, &method)
            .await
    }

    async fn handle_admin_request(
        &self,
        req: Request<hyper::body::Incoming>,
        current_user_id: &str,
        path: &str,
        method: &Method,
    ) -> Response<Full<Bytes>> {
        match (method, path) {
            (&Method::GET, "/admin/users") => admin::handle_list_users(self.user_store.clone()).await,
            (&Method::GET, "/admin/users/new") => admin::handle_new_user_form().await,
            (&Method::POST, "/admin/users") => {
                admin::handle_create_user(req, self.user_store.clone(), self.metrics.clone()).await
            }
            (&Method::POST, path) if path.starts_with("/admin/users/") && path.ends_with("/delete") => {
                let user_id = path
                    .trim_start_matches("/admin/users/")
                    .trim_end_matches("/delete");
                admin::handle_delete_user(user_id, self.user_store.clone(), self.session_store.clone(), self.metrics.clone()).await
            }
            (&Method::POST, path) if path.starts_with("/admin/users/") && path.ends_with("/toggle-admin") => {
                let user_id = path
                    .trim_start_matches("/admin/users/")
                    .trim_end_matches("/toggle-admin");
                admin::handle_toggle_admin(user_id, current_user_id, self.user_store.clone(), self.metrics.clone()).await
            }
            (&Method::GET, path) if path.starts_with("/admin/users/") && path.ends_with("/reset-password") => {
                let user_id = path
                    .trim_start_matches("/admin/users/")
                    .trim_end_matches("/reset-password");
                admin::handle_reset_password_form(user_id, self.user_store.clone()).await
            }
            (&Method::POST, path) if path.starts_with("/admin/users/") && path.ends_with("/password") => {
                let user_id = path
                    .trim_start_matches("/admin/users/")
                    .trim_end_matches("/password");
                admin::handle_update_password(user_id, req, self.user_store.clone(), self.session_store.clone(), self.metrics.clone()).await
            }
            _ => responses::not_found(true),
        }
    }

    async fn handle_authenticated_request(
        &self,
        req: Request<hyper::body::Incoming>,
        user_id: &str,
        is_admin: bool,
        path: &str,
        method: &Method,
    ) -> Response<Full<Bytes>> {
        // Get CasFS for this user
        let casfs = match self.user_router.get_casfs_by_user_id(user_id) {
            Ok(cf) => cf,
            Err(e) => {
                return responses::error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to access storage: {}", e),
                    true,
                );
            }
        };

        let wants_html = self.wants_html(&req);

        match (method, path) {
            (&Method::GET, "/") => self.handle_root(wants_html).await,
            (&Method::GET, "/profile") => {
                profile::handle_profile_page(user_id.to_string(), self.user_store.clone(), req).await
            }
            (&Method::POST, "/profile/password") => {
                profile::handle_change_password(
                    user_id.to_string(),
                    req,
                    self.user_store.clone(),
                    self.session_store.clone(),
                    self.session_auth.clone(),
                )
                .await
            }
            (&Method::GET, "/api/v1/buckets") => handlers::list_buckets(&casfs, false, Some(is_admin)).await,
            (&Method::GET, "/buckets") => handlers::list_buckets(&casfs, wants_html, Some(is_admin)).await,
            (&Method::GET, path) if path.starts_with("/buckets/") => {
                self.handle_bucket_path(&casfs, path, wants_html, &req).await
            }
            (&Method::GET, path) if path.starts_with("/api/v1/buckets/") => {
                self.handle_api_path(&casfs, path, &req).await
            }
            _ => responses::not_found(wants_html),
        }
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
                "name": "s3-cas HTTP API (Multi-User)",
                "version": env!("CARGO_PKG_VERSION"),
                "endpoints": {
                    "/login": "Login page",
                    "/logout": "Logout",
                    "/buckets": "List all buckets",
                    "/buckets/{bucket}": "List objects in bucket",
                    "/buckets/{bucket}/{key}": "Get object metadata",
                    "/admin/users": "User management (admin only)",
                    "/health": "Health check"
                }
            });
            responses::json_response(StatusCode::OK, &info)
        }
    }

    async fn handle_health(&self) -> Response<Full<Bytes>> {
        let health = serde_json::json!({
            "status": "healthy",
            "storage": "operational",
            "mode": "multi-user"
        });
        responses::json_response(StatusCode::OK, &health)
    }

    async fn handle_bucket_path(
        &self,
        casfs: &Arc<CasFS>,
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
            [bucket] => handlers::list_objects(casfs, bucket, req, wants_html).await,
            [bucket, key @ ..] => {
                let object_key = key.join("/");
                handlers::object_metadata(casfs, bucket, &object_key, wants_html).await
            }
            _ => responses::error_response(StatusCode::BAD_REQUEST, "Invalid path", wants_html),
        }
    }

    async fn handle_api_path(
        &self,
        casfs: &Arc<CasFS>,
        path: &str,
        req: &Request<hyper::body::Incoming>,
    ) -> Response<Full<Bytes>> {
        let path_parts: Vec<&str> = path
            .trim_start_matches("/api/v1/buckets/")
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        match path_parts.as_slice() {
            [bucket] => handlers::list_objects(casfs, bucket, req, false).await,
            [bucket, "objects", key @ ..] => {
                let object_key = key.join("/");
                handlers::object_metadata(casfs, bucket, &object_key, false).await
            }
            _ => responses::error_response(StatusCode::BAD_REQUEST, "Invalid API path", false),
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
}

/// Enum wrapper to support both single-user and multi-user HTTP UI services
#[derive(Clone)]
pub enum HttpUiServiceEnum {
    SingleUser(HttpUiService),
    MultiUser(HttpUiServiceMultiUser),
}

impl HttpUiServiceEnum {
    /// Handle incoming HTTP request (forwards to the underlying service)
    pub async fn handle_request(
        &self,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
        match self {
            HttpUiServiceEnum::SingleUser(service) => service.handle_request(req).await,
            HttpUiServiceEnum::MultiUser(service) => service.handle_request(req).await,
        }
    }
}
