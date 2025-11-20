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

#[async_trait::async_trait]
impl S3 for S3UserRouter {
    async fn complete_multipart_upload(
        &self,
        req: S3Request<CompleteMultipartUploadInput>,
    ) -> S3Result<S3Response<CompleteMultipartUploadOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.complete_multipart_upload(req).await
    }

    async fn copy_object(
        &self,
        req: S3Request<CopyObjectInput>,
    ) -> S3Result<S3Response<CopyObjectOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.copy_object(req).await
    }

    async fn create_bucket(
        &self,
        req: S3Request<CreateBucketInput>,
    ) -> S3Result<S3Response<CreateBucketOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.create_bucket(req).await
    }

    async fn create_multipart_upload(
        &self,
        req: S3Request<CreateMultipartUploadInput>,
    ) -> S3Result<S3Response<CreateMultipartUploadOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.create_multipart_upload(req).await
    }

    async fn delete_bucket(
        &self,
        req: S3Request<DeleteBucketInput>,
    ) -> S3Result<S3Response<DeleteBucketOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.delete_bucket(req).await
    }

    async fn delete_object(
        &self,
        req: S3Request<DeleteObjectInput>,
    ) -> S3Result<S3Response<DeleteObjectOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.delete_object(req).await
    }

    async fn delete_objects(
        &self,
        req: S3Request<DeleteObjectsInput>,
    ) -> S3Result<S3Response<DeleteObjectsOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.delete_objects(req).await
    }

    async fn get_bucket_location(
        &self,
        req: S3Request<GetBucketLocationInput>,
    ) -> S3Result<S3Response<GetBucketLocationOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.get_bucket_location(req).await
    }

    async fn get_object(
        &self,
        req: S3Request<GetObjectInput>,
    ) -> S3Result<S3Response<GetObjectOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.get_object(req).await
    }

    async fn head_bucket(
        &self,
        req: S3Request<HeadBucketInput>,
    ) -> S3Result<S3Response<HeadBucketOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.head_bucket(req).await
    }

    async fn head_object(
        &self,
        req: S3Request<HeadObjectInput>,
    ) -> S3Result<S3Response<HeadObjectOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.head_object(req).await
    }

    async fn list_buckets(
        &self,
        req: S3Request<ListBucketsInput>,
    ) -> S3Result<S3Response<ListBucketsOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.list_buckets(req).await
    }

    async fn list_objects(
        &self,
        req: S3Request<ListObjectsInput>,
    ) -> S3Result<S3Response<ListObjectsOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.list_objects(req).await
    }

    async fn list_objects_v2(
        &self,
        req: S3Request<ListObjectsV2Input>,
    ) -> S3Result<S3Response<ListObjectsV2Output>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.list_objects_v2(req).await
    }

    async fn put_object(
        &self,
        req: S3Request<PutObjectInput>,
    ) -> S3Result<S3Response<PutObjectOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.put_object(req).await
    }

    async fn upload_part(
        &self,
        req: S3Request<UploadPartInput>,
    ) -> S3Result<S3Response<UploadPartOutput>> {
        let s3fs = self.get_s3fs_for_request(&req)?;
        s3fs.upload_part(req).await
    }
}
