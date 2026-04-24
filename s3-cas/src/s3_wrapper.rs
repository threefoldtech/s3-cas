use std::sync::Arc;
use tracing::{debug, warn};

use s3s::dto::*;
use s3s::{s3_error, S3Request, S3Response, S3Result, S3};
use s3s::auth::S3Auth;

use crate::auth::{UserRouter, UserStore};
use crate::s3fs::S3FS;

/// DynamicS3Auth provides S3 authentication by querying UserStore dynamically
/// instead of storing credentials in memory
pub struct DynamicS3Auth {
    user_store: Arc<UserStore>,
}

impl DynamicS3Auth {
    pub fn new(user_store: Arc<UserStore>) -> Self {
        Self { user_store }
    }
}

#[async_trait::async_trait]
impl S3Auth for DynamicS3Auth {
    async fn get_secret_key(&self, access_key: &str) -> Result<s3s::auth::SecretKey, s3s::S3Error> {
        debug!("Looking up secret key for access_key: {}", access_key);

        // Look up user by S3 access key
        match self.user_store.get_user_by_s3_key(access_key) {
            Ok(Some(user)) => {
                debug!("Found user {} for access_key: {}", user.user_id, access_key);
                Ok(user.s3_secret_key.into())
            }
            Ok(None) => {
                warn!("Unknown access_key: {}", access_key);
                Err(s3_error!(InvalidAccessKeyId))
            }
            Err(e) => {
                warn!("Database error looking up access_key {}: {}", access_key, e);
                Err(s3_error!(InternalError))
            }
        }
    }
}

/// S3UserRouter wraps UserRouter to provide per-request S3 routing
/// based on the access_key in the request credentials
pub struct S3UserRouter {
    user_router: Arc<UserRouter>,
    user_store: Arc<UserStore>,
}

impl S3UserRouter {
    pub fn new(user_router: Arc<UserRouter>, user_store: Arc<UserStore>) -> Self {
        Self {
            user_router,
            user_store,
        }
    }

    /// Extracts access_key from request and routes to the correct user's S3FS
    fn get_s3fs_for_request<T>(&self, req: &S3Request<T>) -> S3Result<Arc<S3FS>> {
        // Extract access_key from credentials
        let access_key = match &req.credentials {
            Some(creds) => &creds.access_key,
            None => {
                warn!("Request missing credentials");
                return Err(s3_error!(AccessDenied, "Missing credentials"));
            }
        };

        // Look up user by S3 access key
        let user = match self.user_store.get_user_by_s3_key(access_key) {
            Ok(Some(u)) => u,
            Ok(None) => {
                warn!("Unknown access_key: {}", access_key);
                return Err(s3_error!(InvalidAccessKeyId, "Invalid access key"));
            }
            Err(e) => {
                warn!("Database error looking up access_key {}: {}", access_key, e);
                return Err(s3_error!(InternalError, "Database error"));
            }
        };

        debug!("Routing S3 request to user: {}", user.user_id);

        // Get CasFS instance for this user (lazy initialization)
        let casfs = match self.user_router.get_casfs_by_user_id(&user.user_id) {
            Ok(cf) => cf,
            Err(e) => {
                warn!("Failed to get CasFS for user {}: {}", user.user_id, e);
                return Err(s3_error!(InternalError, "Failed to route request"));
            }
        };

        // Create S3FS wrapper around CasFS
        // Note: We create a new S3FS each time, but it's just a thin wrapper with minimal overhead
        let s3fs = crate::s3fs::S3FS::new(casfs, self.user_router.metrics().clone());
        Ok(Arc::new(s3fs))
    }
}

/// Emit a method that resolves the per-user S3FS for this request and
/// forwards. Expands to the hand-rolled form `#[async_trait]` would
/// generate, because `macro_rules!` cannot be nested inside an impl block
/// that carries `#[async_trait]` (the attribute runs before the declarative
/// macro expands). One line per s3s::S3 method.
macro_rules! route_fwd {
    ($method:ident, $input:ty, $output:ty) => {
        fn $method<'life0, 'async_trait>(
            &'life0 self,
            req: S3Request<$input>,
        ) -> ::core::pin::Pin<Box<
            dyn ::core::future::Future<Output = S3Result<S3Response<$output>>>
                + ::core::marker::Send + 'async_trait
        >>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async move {
                let s3fs = self.get_s3fs_for_request(&req)?;
                s3fs.$method(req).await
            })
        }
    };
}

impl S3 for S3UserRouter {
    route_fwd!(complete_multipart_upload, CompleteMultipartUploadInput, CompleteMultipartUploadOutput);
    route_fwd!(copy_object, CopyObjectInput, CopyObjectOutput);
    route_fwd!(create_bucket, CreateBucketInput, CreateBucketOutput);
    route_fwd!(create_multipart_upload, CreateMultipartUploadInput, CreateMultipartUploadOutput);
    route_fwd!(delete_bucket, DeleteBucketInput, DeleteBucketOutput);
    route_fwd!(delete_object, DeleteObjectInput, DeleteObjectOutput);
    route_fwd!(delete_objects, DeleteObjectsInput, DeleteObjectsOutput);
    route_fwd!(get_bucket_location, GetBucketLocationInput, GetBucketLocationOutput);
    route_fwd!(get_object, GetObjectInput, GetObjectOutput);
    route_fwd!(head_bucket, HeadBucketInput, HeadBucketOutput);
    route_fwd!(head_object, HeadObjectInput, HeadObjectOutput);
    route_fwd!(list_buckets, ListBucketsInput, ListBucketsOutput);
    route_fwd!(list_objects, ListObjectsInput, ListObjectsOutput);
    route_fwd!(list_objects_v2, ListObjectsV2Input, ListObjectsV2Output);
    route_fwd!(put_object, PutObjectInput, PutObjectOutput);
    route_fwd!(upload_part, UploadPartInput, UploadPartOutput);
}
