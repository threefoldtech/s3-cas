use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::auth::UserStore;
use cas_storage::StorageEngine;
use cas_storage::{FjallStore, FjallStoreNotx, MetaStore, ObjectData, ObjectType};

/// In multi-user mode the on-disk layout is:
///   {meta_root}/blocks/     -- shared block metadata (also holds _USERS*)
///   {meta_root}/user_<id>/  -- per-user bucket/object metadata
fn shared_meta_path(meta_root: &Path) -> PathBuf {
    meta_root.join("blocks")
}

fn user_meta_path(meta_root: &Path, user_id: &str) -> PathBuf {
    meta_root.join(format!("user_{}", user_id))
}

fn detect_user_ids(meta_root: &Path) -> Result<Vec<String>> {
    let mut user_ids = Vec::new();
    let entries = match fs::read_dir(meta_root) {
        Ok(e) => e,
        Err(_) => return Ok(user_ids),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir()
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
            && let Some(id) = name.strip_prefix("user_")
        {
            user_ids.push(id.to_string());
        }
    }
    Ok(user_ids)
}

fn create_meta_store(path: PathBuf, engine: StorageEngine) -> MetaStore {
    match engine {
        StorageEngine::Fjall => {
            let store = FjallStore::new(path, None, None);
            MetaStore::new(store, None)
        }
        StorageEngine::FjallNotx => {
            let store = FjallStoreNotx::new(path, None);
            MetaStore::new(store, None)
        }
    }
}

pub fn num_keys(meta_root: PathBuf, engine: StorageEngine) -> Result<usize> {
    let user_ids = detect_user_ids(&meta_root)?;
    let mut total = 0;
    for uid in user_ids {
        let meta = create_meta_store(user_meta_path(&meta_root, &uid), engine);
        total += meta.num_keys();
    }
    Ok(total)
}

pub fn disk_space(meta_root: PathBuf, engine: StorageEngine) -> u64 {
    let mut total = 0u64;
    let shared = create_meta_store(shared_meta_path(&meta_root), engine);
    total += shared.disk_space();
    if let Ok(user_ids) = detect_user_ids(&meta_root) {
        for uid in user_ids {
            let meta = create_meta_store(user_meta_path(&meta_root, &uid), engine);
            total += meta.disk_space();
        }
    }
    total
}

pub fn list_users(meta_root: PathBuf, engine: StorageEngine) -> Result<()> {
    let shared = create_meta_store(shared_meta_path(&meta_root), engine);
    let user_store = UserStore::new(shared.get_underlying_store());

    let users = user_store.list_users()?;
    if users.is_empty() {
        println!("No users found");
        return Ok(());
    }

    println!(
        "{:<20} {:<30} {:<10} {:<20}",
        "User ID", "S3 Access Key", "Admin", "Created At"
    );
    println!("{:-<100}", "");
    for user in users {
        let created_at = UNIX_EPOCH + std::time::Duration::from_secs(user.created_at);
        let datetime = chrono::DateTime::<chrono::Utc>::from(created_at);
        println!(
            "{:<20} {:<30} {:<10} {:<20}",
            user.user_id,
            user.s3_access_key,
            if user.is_admin { "Yes" } else { "No" },
            datetime.format("%Y-%m-%d %H:%M:%S"),
        );
    }

    Ok(())
}

pub fn user_stats(
    meta_root: PathBuf,
    engine: StorageEngine,
    user_filter: Option<String>,
) -> Result<()> {
    let user_ids = match user_filter {
        Some(uid) => vec![uid],
        None => detect_user_ids(&meta_root)?,
    };
    if user_ids.is_empty() {
        println!("No users found");
        return Ok(());
    }

    println!(
        "{:<20} {:<15} {:<15} {:<20}",
        "User ID", "Bucket Count", "Object Count", "Total Size"
    );
    println!("{:-<70}", "");

    for uid in user_ids {
        let path = user_meta_path(&meta_root, &uid);
        if !path.exists() {
            println!("{:<20} (database not found)", uid);
            continue;
        }
        let meta = create_meta_store(path, engine);
        let buckets = meta.list_buckets().unwrap_or_default();
        let bucket_count = buckets.len();
        let mut total_objects = 0usize;
        let mut total_size = 0u64;
        for b in buckets {
            let tree = match meta.get_bucket_ext(b.name()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            for (_k, obj) in tree.range_filter(None, None, None) {
                total_objects += 1;
                total_size += obj.size();
            }
        }
        println!(
            "{:<20} {:<15} {:<15} {:<20}",
            uid,
            bucket_count,
            total_objects,
            format_bytes(total_size)
        );
    }
    Ok(())
}

pub fn list_buckets(
    meta_root: PathBuf,
    engine: StorageEngine,
    user_filter: Option<String>,
) -> Result<()> {
    let user_ids = match user_filter {
        Some(uid) => vec![uid],
        None => detect_user_ids(&meta_root)?,
    };
    println!(
        "{:<20} {:<30} {:<15} {:<20}",
        "Owner", "Bucket Name", "Object Count", "Created At"
    );
    println!("{:-<85}", "");
    for uid in user_ids {
        let path = user_meta_path(&meta_root, &uid);
        if !path.exists() {
            continue;
        }
        let meta = create_meta_store(path, engine);
        let buckets = meta.list_buckets().unwrap_or_default();
        for b in buckets {
            let tree = meta.get_bucket_ext(b.name()).ok();
            let count = tree.map(|t| t.range_filter(None, None, None).count()).unwrap_or(0);
            let dt = chrono::DateTime::<chrono::Utc>::from(b.ctime());
            println!(
                "{:<20} {:<30} {:<15} {:<20}",
                uid,
                b.name(),
                count,
                dt.format("%Y-%m-%d %H:%M:%S")
            );
        }
    }
    Ok(())
}

pub fn bucket_stats(
    meta_root: PathBuf,
    engine: StorageEngine,
    bucket: String,
    user: String,
) -> Result<()> {
    let path = user_meta_path(&meta_root, &user);
    let meta = create_meta_store(path, engine);

    if !meta.bucket_exists(&bucket)? {
        bail!("Bucket '{}' not found for user '{}'", bucket, user);
    }
    let tree = meta.get_bucket_ext(&bucket)?;

    let mut object_count = 0usize;
    let mut total_size = 0u64;
    let mut unique_blocks = std::collections::HashSet::new();
    let mut multipart_count = 0usize;
    let mut inline_count = 0usize;

    for (_k, obj) in tree.range_filter(None, None, None) {
        object_count += 1;
        total_size += obj.size();
        match obj.object_type() {
            ObjectType::Multipart => multipart_count += 1,
            ObjectType::Inline => inline_count += 1,
            _ => {}
        }
        for id in obj.blocks() {
            unique_blocks.insert(*id);
        }
    }

    println!("Bucket: {} (user: {})", bucket, user);
    println!("Object count: {}", object_count);
    println!(
        "Total size: {} ({} bytes)",
        format_bytes(total_size),
        total_size
    );
    println!("Unique blocks: {}", unique_blocks.len());
    println!("Multipart objects: {}", multipart_count);
    println!("Inline objects: {}", inline_count);
    if object_count > 0 {
        println!(
            "Average object size: {}",
            format_bytes(total_size / object_count as u64)
        );
    }
    Ok(())
}

pub fn block_stats(meta_root: PathBuf, engine: StorageEngine) -> Result<()> {
    let shared = create_meta_store(shared_meta_path(&meta_root), engine);
    let block_tree = shared.get_block_tree()?;

    let mut total_blocks = 0usize;
    let mut total_block_size = 0u64;
    let mut total_ref_count = 0usize;
    let mut distribution: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();

    for item in block_tree.iter_all() {
        let (_id, block) = match item {
            Ok(t) => t,
            Err(_) => continue,
        };
        total_blocks += 1;
        total_block_size += block.size() as u64;
        let rc = block.rc();
        total_ref_count += rc;
        *distribution.entry(rc).or_insert(0) += 1;
    }

    println!("Block Statistics:");
    println!("  Total blocks: {}", total_blocks);
    println!(
        "  Total block storage: {} ({} bytes)",
        format_bytes(total_block_size),
        total_block_size
    );
    println!("  Total references: {}", total_ref_count);

    if total_blocks > 0 {
        let avg = total_ref_count as f64 / total_blocks as f64;
        println!("  Average references per block: {:.2}", avg);
        println!("  Deduplication ratio: {:.2}x", avg);
        let savings = ((avg - 1.0) / avg) * 100.0;
        println!("  Storage savings: {:.1}%", savings);
    }

    println!("\nReference count distribution:");
    let mut counts: Vec<_> = distribution.iter().collect();
    counts.sort_by_key(|(rc, _)| *rc);
    for (rc, count) in counts.iter().take(10) {
        println!("  RC={}: {} blocks", rc, count);
    }
    if counts.len() > 10 {
        println!("  ... ({} more)", counts.len() - 10);
    }
    Ok(())
}

pub fn object_info(
    meta_root: PathBuf,
    engine: StorageEngine,
    bucket: String,
    key: String,
    user: String,
) -> Result<()> {
    let path = user_meta_path(&meta_root, &user);
    let meta = create_meta_store(path, engine);

    let obj = match meta.get_meta(&bucket, &key)? {
        Some(o) => o,
        None => bail!("Object '{}' not found in bucket '{}'", key, bucket),
    };

    println!("Object: {}/{} (user: {})", bucket, key, user);
    println!("Size: {} ({} bytes)", format_bytes(obj.size()), obj.size());
    println!("Type: {:?}", obj.object_type());
    println!("Hash: {}", hex::encode(obj.hash()));

    let dt = chrono::DateTime::<chrono::Utc>::from(obj.last_modified());
    println!("Created: {}", dt.format("%Y-%m-%d %H:%M:%S"));

    if obj.is_inlined() {
        if let Some(data) = obj.inlined() {
            println!("Inline data: {} bytes", data.len());
        }
    } else {
        let blocks = obj.blocks();
        println!("Blocks: {}", blocks.len());
        let show = blocks.len().min(10);
        println!("\nBlock IDs:");
        for (i, id) in blocks.iter().take(show).enumerate() {
            println!("  {}: {}", i + 1, hex::encode(id));
        }
        if blocks.len() > show {
            println!("  ... ({} more blocks)", blocks.len() - show);
        }

        if let ObjectType::Multipart = obj.object_type()
            && let ObjectData::MultiPart { parts, .. } = obj.data()
        {
            println!("\nMultipart upload: {} parts", parts);
        }
    }
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut size = bytes as f64;
    let mut i = 0;
    while size >= 1024.0 && i < UNITS.len() - 1 {
        size /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", size, UNITS[i])
}
