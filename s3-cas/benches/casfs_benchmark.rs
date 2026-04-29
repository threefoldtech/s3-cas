use bytes::Bytes;
use cas_storage::{AsyncByteStream, CasFS, Durability, StorageEngine};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use futures::stream;
use once_cell::sync::Lazy;
use rand::RngExt;
use s3_cas::metrics::SharedMetrics;
use std::time::Duration;
use tempfile::TempDir;
use tokio::runtime::Runtime;

// Create a single shared metrics instance to avoid registry conflicts
static METRICS: Lazy<SharedMetrics> = Lazy::new(SharedMetrics::new);

fn get_shared_metrics() -> SharedMetrics {
    METRICS.clone()
}

// Helper function to create a temporary CasFS with FjallNoTx
fn setup_casfs() -> (CasFS, TempDir) {
    let dir = TempDir::new().unwrap();
    let root_path = dir.path().to_path_buf();
    let meta_path = root_path.clone();

    let metrics = get_shared_metrics();
    let storage_engine = StorageEngine::FjallNotx;
    let inlined_metadata_size = Some(1024);
    let durability = Some(Durability::Buffer);

    let fs = CasFS::single_namespace(
        root_path,
        meta_path,
        metrics.to_cas_metrics(),
        storage_engine,
        inlined_metadata_size,
        durability,
    )
    .expect("open CasFS");

    (fs, dir)
}

fn create_test_bucket(fs: &CasFS, name: &str) {
    fs.create_bucket(name).unwrap();
}

fn create_random_data(size: usize) -> Vec<u8> {
    let mut rng = rand::rng();
    let mut data = vec![0u8; size];
    rng.fill(&mut data[..]);
    data
}

fn vec_to_bytestream(data: Vec<u8>) -> AsyncByteStream {
    AsyncByteStream::new(stream::once(async move { Ok(Bytes::from(data)) }))
}

fn bench_store_methods(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("store_methods");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    let sizes = [128, 512, 1024, 4096, 8192, 16384];

    for &size in &sizes {
        let (fs, _dir) = setup_casfs();
        let bucket_name = "test-bucket";
        create_test_bucket(&fs, bucket_name);

        group.bench_function(BenchmarkId::new("store_inlined_object", size), |b| {
            b.iter(|| {
                let data = create_random_data(size);
                let key = format!("inline-key-{}", rand::rng().random::<u32>());
                black_box(fs.store_inlined_object(bucket_name, &key, data)).unwrap()
            })
        });

        group.bench_function(
            BenchmarkId::new("store_single_object_and_meta", size),
            |b| {
                b.iter(|| {
                    let data = create_random_data(size);
                    let len = data.len();
                    let key = format!("single-key-{}", rand::rng().random::<u32>());
                    let stream = vec_to_bytestream(data);
                    black_box(rt.block_on(fs.store_single_object_and_meta(
                        bucket_name,
                        &key,
                        stream,
                        len,
                    )))
                    .unwrap()
                })
            },
        );
    }

    group.finish();
}

fn bench_inlined_object_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_inlined_object_sizes");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    let (fs, _dir) = setup_casfs();
    let bucket_name = "test-bucket";
    create_test_bucket(&fs, bucket_name);

    let max_inlined = fs.max_inlined_data_length();
    let percentages = [25, 50, 75, 90];

    for &percentage in &percentages {
        let size = (max_inlined * percentage) / 100;

        group.bench_function(BenchmarkId::new("percentage_of_max", percentage), |b| {
            b.iter(|| {
                let data = create_random_data(size);
                let key = format!("key-{}", rand::rng().random::<u32>());
                black_box(fs.store_inlined_object(bucket_name, &key, data)).unwrap()
            })
        });
    }

    group.finish();
}

fn bench_store_methods_overhead(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("store_methods_overhead");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    let size = 10; // 10 bytes

    let (fs, _dir) = setup_casfs();
    let bucket_name = "test-bucket";
    create_test_bucket(&fs, bucket_name);

    group.bench_function("store_inlined_object_overhead", |b| {
        b.iter(|| {
            let data = create_random_data(size);
            let key = format!("inline-key-{}", rand::rng().random::<u32>());
            black_box(fs.store_inlined_object(bucket_name, &key, data)).unwrap()
        })
    });

    group.bench_function("store_single_object_and_meta_overhead", |b| {
        b.iter(|| {
            let data = create_random_data(size);
            let len = data.len();
            let key = format!("single-key-{}", rand::rng().random::<u32>());
            let stream = vec_to_bytestream(data);
            black_box(rt.block_on(
                fs.store_single_object_and_meta(bucket_name, &key, stream, len),
            ))
            .unwrap()
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_store_methods,
    bench_inlined_object_sizes,
    bench_store_methods_overhead
);
criterion_main!(benches);
