use cas_storage::{
    Block, BlockID, BucketMeta, FjallStore, FjallStoreNotx, MetaStore, Object, ObjectData,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::RngExt;
use std::time::Duration;
use tempfile::TempDir;

// Helper function to create a temporary MetaStore backed by FjallStore
fn setup_fjall_store() -> (MetaStore, TempDir) {
    let dir = TempDir::new().unwrap();
    let store = FjallStore::new(
        dir.path().to_path_buf(),
        Some(1024), // inline metadata size
        None,       // default durability
    );
    let meta = MetaStore::new(store, Some(1024));
    (meta, dir)
}

// Helper function to create a temporary MetaStore backed by FjallStoreNotx
fn setup_fjall_notx_store() -> (MetaStore, TempDir) {
    let dir = TempDir::new().unwrap();
    let store = FjallStoreNotx::new(dir.path().to_path_buf(), Some(1024));
    let meta = MetaStore::new(store, Some(1024));
    (meta, dir)
}

fn create_test_bucket(name: &str) -> Vec<u8> {
    BucketMeta::new(name.to_string()).to_vec()
}

fn create_test_object(size: usize) -> Vec<u8> {
    // Fill with a deterministic pattern (size parameter kept for signature
    // consistency with older bench; contents are not load-bearing).
    let mut block_id = [0u8; 16];
    for (i, slot) in block_id.iter_mut().enumerate() {
        *slot = i as u8;
    }
    let obj = Object::new(
        size as u64,
        block_id,
        ObjectData::SinglePart { blocks: vec![] },
    );
    obj.to_vec()
}

fn create_block_id(id: u8) -> BlockID {
    let mut block_id = [0u8; 16];
    block_id[0] = id;
    block_id
}

fn create_test_block(id: u8, size: usize) -> (BlockID, Vec<u8>) {
    let block_id = create_block_id(id);
    let path = block_id.to_vec();
    let block = Block::new(size, path);
    (block_id, block.to_vec())
}

fn bench_insert_bucket(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert_bucket");
    group.measurement_time(Duration::from_secs(10));

    {
        let (meta, _dir) = setup_fjall_store();
        group.bench_function(BenchmarkId::new("FjallStore", "insert_bucket"), |b| {
            b.iter(|| {
                let bucket_name = format!("bucket-{}", rand::rng().random::<u32>());
                let bucket_data = create_test_bucket(&bucket_name);
                black_box(meta.insert_bucket(&bucket_name, bucket_data)).unwrap();
            });
        });
    }

    {
        let (meta, _dir) = setup_fjall_notx_store();
        group.bench_function(BenchmarkId::new("FjallStoreNotx", "insert_bucket"), |b| {
            b.iter(|| {
                let bucket_name = format!("bucket-{}", rand::rng().random::<u32>());
                let bucket_data = create_test_bucket(&bucket_name);
                black_box(meta.insert_bucket(&bucket_name, bucket_data)).unwrap();
            });
        });
    }

    group.finish();
}

fn bench_insert_meta(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert_meta");
    group.measurement_time(Duration::from_secs(10));

    let small_object = create_test_object(1024);
    let medium_object = create_test_object(100 * 1024);

    {
        let (meta, _dir) = setup_fjall_store();
        let bucket_name = "test-bucket";
        meta.insert_bucket(bucket_name, create_test_bucket(bucket_name)).unwrap();

        group.bench_function(BenchmarkId::new("FjallStore", "insert_small_object"), |b| {
            b.iter(|| {
                let key = format!("key-{}", rand::rng().random::<u32>());
                black_box(meta.insert_meta(bucket_name, &key, small_object.clone())).unwrap();
            });
        });
    }

    {
        let (meta, _dir) = setup_fjall_notx_store();
        let bucket_name = "test-bucket";
        meta.insert_bucket(bucket_name, create_test_bucket(bucket_name)).unwrap();

        group.bench_function(
            BenchmarkId::new("FjallStoreNotx", "insert_small_object"),
            |b| {
                b.iter(|| {
                    let key = format!("key-{}", rand::rng().random::<u32>());
                    black_box(meta.insert_meta(bucket_name, &key, small_object.clone())).unwrap();
                });
            },
        );
    }

    {
        let (meta, _dir) = setup_fjall_store();
        let bucket_name = "test-bucket";
        meta.insert_bucket(bucket_name, create_test_bucket(bucket_name)).unwrap();

        group.bench_function(BenchmarkId::new("FjallStore", "insert_medium_object"), |b| {
            b.iter(|| {
                let key = format!("key-{}", rand::rng().random::<u32>());
                black_box(meta.insert_meta(bucket_name, &key, medium_object.clone())).unwrap();
            });
        });
    }

    {
        let (meta, _dir) = setup_fjall_notx_store();
        let bucket_name = "test-bucket";
        meta.insert_bucket(bucket_name, create_test_bucket(bucket_name)).unwrap();

        group.bench_function(
            BenchmarkId::new("FjallStoreNotx", "insert_medium_object"),
            |b| {
                b.iter(|| {
                    let key = format!("key-{}", rand::rng().random::<u32>());
                    black_box(meta.insert_meta(bucket_name, &key, medium_object.clone())).unwrap();
                });
            },
        );
    }

    group.finish();
}

fn bench_get_meta(c: &mut Criterion) {
    let mut group = c.benchmark_group("get_meta");
    group.measurement_time(Duration::from_secs(10));

    {
        let (meta, _dir) = setup_fjall_store();
        let bucket_name = "test-bucket";
        meta.insert_bucket(bucket_name, create_test_bucket(bucket_name)).unwrap();
        for i in 0..100 {
            let key = format!("key-{}", i);
            meta.insert_meta(bucket_name, &key, create_test_object(1024)).unwrap();
        }

        group.bench_function(BenchmarkId::new("FjallStore", "get_meta"), |b| {
            b.iter(|| {
                let key = format!("key-{}", rand::rng().random::<u8>() % 100);
                black_box(meta.get_meta(bucket_name, &key)).unwrap();
            });
        });
    }

    {
        let (meta, _dir) = setup_fjall_notx_store();
        let bucket_name = "test-bucket";
        meta.insert_bucket(bucket_name, create_test_bucket(bucket_name)).unwrap();
        for i in 0..100 {
            let key = format!("key-{}", i);
            meta.insert_meta(bucket_name, &key, create_test_object(1024)).unwrap();
        }

        group.bench_function(BenchmarkId::new("FjallStoreNotx", "get_meta"), |b| {
            b.iter(|| {
                let key = format!("key-{}", rand::rng().random::<u8>() % 100);
                black_box(meta.get_meta(bucket_name, &key)).unwrap();
            });
        });
    }

    group.finish();
}

fn bench_list_buckets(c: &mut Criterion) {
    let mut group = c.benchmark_group("list_buckets");
    group.measurement_time(Duration::from_secs(10));

    {
        let (meta, _dir) = setup_fjall_store();
        for i in 0..50 {
            let bucket_name = format!("bucket-{}", i);
            meta.insert_bucket(&bucket_name, create_test_bucket(&bucket_name)).unwrap();
        }

        group.bench_function(BenchmarkId::new("FjallStore", "list_buckets"), |b| {
            b.iter(|| {
                black_box(meta.list_buckets()).unwrap();
            });
        });
    }

    {
        let (meta, _dir) = setup_fjall_notx_store();
        for i in 0..50 {
            let bucket_name = format!("bucket-{}", i);
            meta.insert_bucket(&bucket_name, create_test_bucket(&bucket_name)).unwrap();
        }

        group.bench_function(BenchmarkId::new("FjallStoreNotx", "list_buckets"), |b| {
            b.iter(|| {
                black_box(meta.list_buckets()).unwrap();
            });
        });
    }

    group.finish();
}

fn bench_transaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("transaction");
    group.measurement_time(Duration::from_secs(10));

    {
        let (meta, _dir) = setup_fjall_store();
        let bucket_name = "test-bucket";
        meta.insert_bucket(bucket_name, create_test_bucket(bucket_name)).unwrap();

        group.bench_function(BenchmarkId::new("FjallStore", "transaction"), |b| {
            b.iter(|| {
                let mut tx = meta.begin_transaction();
                let (block_id, _) = create_test_block(rand::rng().random::<u8>(), 1024);
                black_box(tx.write_block(block_id, 1024, false)).unwrap();
                black_box(Box::new(tx).commit()).unwrap();
            });
        });
    }

    {
        let (meta, _dir) = setup_fjall_notx_store();
        let bucket_name = "test-bucket";
        meta.insert_bucket(bucket_name, create_test_bucket(bucket_name)).unwrap();

        group.bench_function(BenchmarkId::new("FjallStoreNotx", "transaction"), |b| {
            b.iter(|| {
                let mut tx = meta.begin_transaction();
                let (block_id, _) = create_test_block(rand::rng().random::<u8>(), 1024);
                black_box(tx.write_block(block_id, 1024, false)).unwrap();
                black_box(Box::new(tx).commit()).unwrap();
            });
        });
    }

    group.finish();
}

fn bench_mixed_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_workload");
    group.measurement_time(Duration::from_secs(20));
    group.sample_size(10);

    {
        let (meta, _dir) = setup_fjall_store();

        group.bench_function(BenchmarkId::new("FjallStore", "mixed_workload"), |b| {
            b.iter(|| {
                let bucket_name = format!("bucket-{}", rand::rng().random::<u16>());
                meta.insert_bucket(&bucket_name, create_test_bucket(&bucket_name)).unwrap();

                for i in 0..10 {
                    let key = format!("key-{}", i);
                    let obj = create_test_object(1024 * (i + 1));
                    meta.insert_meta(&bucket_name, &key, obj).unwrap();
                }

                for i in 0..5 {
                    let key = format!("key-{}", i);
                    black_box(meta.get_meta(&bucket_name, &key)).unwrap();
                }

                black_box(meta.list_buckets()).unwrap();

                let mut tx = meta.begin_transaction();
                let (block_id, _) = create_test_block(rand::rng().random::<u8>(), 1024);
                black_box(tx.write_block(block_id, 1024, false)).unwrap();
                black_box(Box::new(tx).commit()).unwrap();
            });
        });
    }

    {
        let (meta, _dir) = setup_fjall_notx_store();

        group.bench_function(BenchmarkId::new("FjallStoreNotx", "mixed_workload"), |b| {
            b.iter(|| {
                let bucket_name = format!("bucket-{}", rand::rng().random::<u16>());
                meta.insert_bucket(&bucket_name, create_test_bucket(&bucket_name)).unwrap();

                for i in 0..10 {
                    let key = format!("key-{}", i);
                    let obj = create_test_object(1024 * (i + 1));
                    meta.insert_meta(&bucket_name, &key, obj).unwrap();
                }

                for i in 0..5 {
                    let key = format!("key-{}", i);
                    black_box(meta.get_meta(&bucket_name, &key)).unwrap();
                }

                black_box(meta.list_buckets()).unwrap();

                let mut tx = meta.begin_transaction();
                let (block_id, _) = create_test_block(rand::rng().random::<u8>(), 1024);
                black_box(tx.write_block(block_id, 1024, false)).unwrap();
                black_box(Box::new(tx).commit()).unwrap();
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_insert_bucket,
    bench_insert_meta,
    bench_get_meta,
    bench_list_buckets,
    bench_transaction,
    bench_mixed_workload
);
criterion_main!(benches);
