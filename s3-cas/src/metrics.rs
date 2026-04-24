use cas_storage::MetricsCollector;
use prometheus::{
    register_int_counter, register_int_counter_vec, register_int_gauge, IntCounter, IntCounterVec,
    IntGauge,
};
use s3s::dto::*;
use s3s::S3;
use s3s::{S3Request, S3Response, S3Result};
use std::{ops::Deref, sync::Arc};

const S3_API_METHODS: &[&str] = &[
    "complete_multipart_upload",
    "copy_object",
    "create_multipart_upload",
    "create_bucket",
    "delete_bucket",
    "delete_object",
    "delete_objects",
    "get_bucket_location",
    "get_object",
    "head_bucket",
    "head_object",
    "list_buckets",
    "list_objects",
    "list_objects_v2",
    "put_object",
    "upload_part",
];

#[derive(Clone, Debug)]
pub struct SharedMetrics {
    metrics: Arc<Metrics>,
}

impl SharedMetrics {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(Metrics::new()),
        }
    }

    /// Convert to cas_storage::SharedMetrics
    pub fn to_cas_metrics(&self) -> cas_storage::SharedMetrics {
        cas_storage::SharedMetrics::new(self.metrics.clone())
    }
}

impl Default for SharedMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsCollector for Metrics {
    fn block_pending(&self) {
        self.data_blocks_pending_write.inc();
    }

    fn block_written(&self) {
        self.data_blocks_pending_write.dec();
        self.data_blocks_written.inc();
    }

    fn block_write_error(&self) {
        self.data_blocks_pending_write.dec();
        self.data_blocks_write_errors.inc();
    }

    fn block_ignored(&self) {
        self.data_blocks_pending_write.dec();
        self.data_blocks_ignored.inc();
    }

    fn blocks_dropped(&self, amount: u64) {
        self.data_blocks_pending_write.sub(amount as i64);
        self.data_blocks_dropped.inc_by(amount);
    }

    fn bytes_sent(&self, amount: usize) {
        self.data_bytes_sent.inc_by(amount as u64);
    }

    fn bytes_received(&self, amount: usize) {
        self.data_bytes_received.inc_by(amount as u64);
    }
}

impl Deref for SharedMetrics {
    type Target = Metrics;

    fn deref(&self) -> &Self::Target {
        &self.metrics
    }
}
#[derive(Debug)]
pub struct Metrics {
    method_calls: IntCounterVec,
    bucket_count: IntGauge,
    data_bytes_received: IntCounter,
    data_bytes_sent: IntCounter,
    data_bytes_written: IntCounter,
    data_blocks_written: IntCounter,
    data_blocks_ignored: IntCounter,
    data_blocks_pending_write: IntGauge,
    data_blocks_write_errors: IntCounter,
    data_blocks_dropped: IntCounter,
}

// TODO: this can be improved, make sure this does not crash on multiple instances;
impl Metrics {
    pub fn new() -> Self {
        let method_calls = register_int_counter_vec!(
            "s3_api_method_invocations",
            "Amount of times a particular S3 API method has been called in the lifetime of the process",
            &["api_method"],
        ).expect("can register an int counter vec in the default registry");

        // instantiate the correct counters for api calls
        for api in S3_API_METHODS {
            method_calls.with_label_values(&[api]);
        }

        let bucket_count = register_int_gauge!(
            "s3_bucket_count",
            "Amount of active buckets in the S3 instance"
        )
        .expect("can register an int gauge in the default registry");

        let data_bytes_received = register_int_counter!(
            "s3_data_bytes_received",
            "Amount of bytes of actual data received"
        )
        .expect("can register an int counter in the default registry");

        let data_bytes_sent =
            register_int_counter!("s3_data_bytes_sent", "Amount of bytes of actual data sent")
                .expect("can register an int counter in the default registry");

        let data_bytes_written = register_int_counter!(
            "s3_data_bytes_written",
            "Amount of bytes of actual data written to block storage"
        )
        .expect("can register an int counter in the default registry");

        let data_blocks_written = register_int_counter!(
            "s3_data_blocks_written",
            "Amount of data blocks written to block storage"
        )
        .expect("can register an int counter in the default registry");

        let data_blocks_ignored = register_int_counter!(
            "s3_data_blocks_ignored",
            "Amount of data blocks not written to block storage, because a block with the same hash is already present"
        )
        .expect("can register an int counter in the default registry");

        let data_blocks_pending_write = register_int_gauge!(
            "s3_data_blocks_pending_write",
            "Amount of data blocks in memory, waiting to be written to block storage"
        )
        .expect("can register an int gauge in the default registry");

        let data_blocks_write_errors = register_int_counter!(
            "s3_data_blocks_write_errors",
            "Amount of data blocks which could not be written to block storage"
        )
        .expect("can register an int counter in the default registry");

        let data_blocks_dropped = register_int_counter!(
            "s3_data_blocks_dropped",
            "Amount of data blocks dropped due to client disconnects before the block was (fully) written to storage",
        ).expect("can register an int gauge in the default registry");

        Self {
            method_calls,
            bucket_count,
            data_bytes_received,
            data_bytes_sent,
            data_bytes_written,
            data_blocks_written,
            data_blocks_ignored,
            data_blocks_pending_write,
            data_blocks_write_errors,
            data_blocks_dropped,
        }
    }

    pub fn add_method_call(&self, call_name: &str) {
        self.method_calls.with_label_values(&[call_name]).inc();
    }

    pub fn set_bucket_count(&self, count: usize) {
        self.bucket_count.set(count as i64)
    }

    pub fn inc_bucket_count(&self) {
        self.bucket_count.inc()
    }

    pub fn dec_bucket_count(&self) {
        self.bucket_count.dec()
    }

    pub fn bytes_received(&self, amount: usize) {
        self.data_bytes_received.inc_by(amount as u64)
    }

    pub fn bytes_sent(&self, amount: usize) {
        self.data_bytes_sent.inc_by(amount as u64)
    }

    pub fn bytes_written(&self, amount: usize) {
        self.data_bytes_written.inc_by(amount as u64)
    }

    pub fn block_pending(&self) {
        self.data_blocks_pending_write.inc()
    }

    pub fn block_written(&self, block_size: usize) {
        self.data_bytes_written.inc_by(block_size as u64);
        self.data_blocks_pending_write.dec();
        self.data_blocks_written.inc()
    }

    pub fn block_write_error(&self) {
        self.data_blocks_pending_write.dec();
        self.data_blocks_write_errors.inc()
    }

    pub fn block_ignored(&self) {
        self.data_blocks_ignored.inc()
    }

    pub fn blocks_dropped(&self, amount: u64) {
        self.data_blocks_pending_write.sub(amount as i64);
        self.data_blocks_dropped.inc_by(amount)
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MetricFs<T> {
    storage: T,
    metrics: SharedMetrics,
}

impl<T> MetricFs<T> {
    pub fn new(storage: T, metrics: SharedMetrics) -> Self {
        Self { storage, metrics }
    }
}

/// Emit a method that bumps the method-call counter and forwards to
/// `self.storage`. Expands to the hand-rolled form `#[async_trait]` would
/// generate, because `macro_rules!` cannot be nested inside an impl block
/// that carries `#[async_trait]` (the attribute runs before the declarative
/// macro expands, so it would not see the generated methods). One line per
/// s3s::S3 method.
macro_rules! metric_fwd {
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
                self.metrics.add_method_call(stringify!($method));
                self.storage.$method(req).await
            })
        }
    };
}

impl<T> S3 for MetricFs<T>
where
    T: S3 + Sync + Send,
{
    metric_fwd!(complete_multipart_upload, CompleteMultipartUploadInput, CompleteMultipartUploadOutput);
    metric_fwd!(copy_object, CopyObjectInput, CopyObjectOutput);
    metric_fwd!(create_multipart_upload, CreateMultipartUploadInput, CreateMultipartUploadOutput);
    metric_fwd!(create_bucket, CreateBucketInput, CreateBucketOutput);
    metric_fwd!(delete_bucket, DeleteBucketInput, DeleteBucketOutput);
    metric_fwd!(delete_object, DeleteObjectInput, DeleteObjectOutput);
    metric_fwd!(delete_objects, DeleteObjectsInput, DeleteObjectsOutput);
    metric_fwd!(get_bucket_location, GetBucketLocationInput, GetBucketLocationOutput);
    metric_fwd!(get_object, GetObjectInput, GetObjectOutput);
    metric_fwd!(head_bucket, HeadBucketInput, HeadBucketOutput);
    metric_fwd!(head_object, HeadObjectInput, HeadObjectOutput);
    metric_fwd!(list_buckets, ListBucketsInput, ListBucketsOutput);
    metric_fwd!(list_objects, ListObjectsInput, ListObjectsOutput);
    metric_fwd!(list_objects_v2, ListObjectsV2Input, ListObjectsV2Output);
    metric_fwd!(put_object, PutObjectInput, PutObjectOutput);
    metric_fwd!(upload_part, UploadPartInput, UploadPartOutput);
}
