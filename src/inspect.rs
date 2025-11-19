use anyhow::{Result, bail};
use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use crate::cas::StorageEngine;
use crate::metastore::{FjallStore, FjallStoreNotx, MetaStore, ObjectType, ObjectData};
use crate::auth::UserStore;

/// Detects if multi-user mode is enabled and returns list of user IDs
fn detect_user_databases(meta_root: &PathBuf) -> Result<Option<Vec<String>>> {
    let mut user_ids = Vec::new();

    // Read directory entries
    let entries = match fs::read_dir(meta_root) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                if dir_name.starts_with("user_") {
                    let user_id = dir_name.strip_prefix("user_").unwrap().to_string();
                    user_ids.push(user_id);
                }
            }
        }
    }

    if user_ids.is_empty() {
        Ok(None)
    } else {
        Ok(Some(user_ids))
    }
}

/// Creates a MetaStore instance for a given path
fn create_meta_store(meta_root: PathBuf, storage_engine: StorageEngine) -> MetaStore {
    match storage_engine {
        StorageEngine::Fjall => {
            let store = FjallStore::new(meta_root, None, None);
            MetaStore::new(store, None)
        }
        StorageEngine::FjallNotx => {
            let store = FjallStoreNotx::new(meta_root, None);
            MetaStore::new(store, None)
        }
    }
}

pub fn num_keys(
    meta_root: PathBuf,
    storage_engine: StorageEngine,
    users_config: Option<PathBuf>,
) -> Result<usize> {
    // Detect multi-user mode
    let is_multi_user = users_config.is_some();

    if is_multi_user {
        // Multi-user mode: aggregate across all user databases
        let user_ids = detect_user_databases(&meta_root)?.unwrap_or_default();

        let mut total_keys = 0;
        for user_id in user_ids {
            let user_meta_path = meta_root.join(format!("user_{}", user_id));
            let meta_store = create_meta_store(user_meta_path, storage_engine);
            total_keys += meta_store.num_keys();
        }

        Ok(total_keys)
    } else {
        // Single-user mode: use meta_root directly
        let meta_store = create_meta_store(meta_root, storage_engine);
        Ok(meta_store.num_keys())
    }
}

pub fn disk_space(
    meta_root: PathBuf,
    storage_engine: StorageEngine,
    users_config: Option<PathBuf>,
) -> u64 {
    // Detect multi-user mode
    let is_multi_user = users_config.is_some();

    if is_multi_user {
        // Multi-user mode: aggregate across shared DB + all user databases
        let mut total_space = 0u64;

        // Add shared database space
        let shared_meta_store = create_meta_store(meta_root.clone(), storage_engine);
        total_space += shared_meta_store.disk_space();

        // Add per-user database space
        if let Ok(Some(user_ids)) = detect_user_databases(&meta_root) {
            for user_id in user_ids {
                let user_meta_path = meta_root.join(format!("user_{}", user_id));
                let meta_store = create_meta_store(user_meta_path, storage_engine);
                total_space += meta_store.disk_space();
            }
        }

        total_space
    } else {
        // Single-user mode: use meta_root directly
        let meta_store = create_meta_store(meta_root, storage_engine);
        meta_store.disk_space()
    }
}

/// List all users (multi-user mode only)
pub fn list_users(
    meta_root: PathBuf,
    storage_engine: StorageEngine,
    users_config: Option<PathBuf>,
) -> Result<()> {
    if users_config.is_none() {
        bail!("list-users command requires multi-user mode (use --users-config)");
    }

    // Open shared database to read user store
    let shared_store = create_meta_store(meta_root, storage_engine);
    let user_store = UserStore::new(shared_store.get_underlying_store());

    let users = user_store.list_users()?;

    if users.is_empty() {
        println!("No users found");
        return Ok(());
    }

    // Print header
    println!("{:<20} {:<20} {:<30} {:<10} {:<20}",
        "User ID", "UI Login", "S3 Access Key", "Admin", "Created At");
    println!("{:-<100}", "");

    // Print each user
    for user in users {
        let created_at = UNIX_EPOCH + std::time::Duration::from_secs(user.created_at);
        let datetime = chrono::DateTime::<chrono::Utc>::from(created_at);

        println!("{:<20} {:<20} {:<30} {:<10} {:<20}",
            user.user_id,
            user.ui_login,
            user.s3_access_key,
            if user.is_admin { "Yes" } else { "No" },
            datetime.format("%Y-%m-%d %H:%M:%S"),
        );
    }

    Ok(())
}

/// Show per-user storage statistics
pub fn user_stats(
    meta_root: PathBuf,
    storage_engine: StorageEngine,
    users_config: Option<PathBuf>,
    user_id_filter: Option<String>,
) -> Result<()> {
    if users_config.is_none() {
        bail!("user-stats command requires multi-user mode (use --users-config)");
    }

    let user_ids = if let Some(user_id) = user_id_filter {
        vec![user_id]
    } else {
        detect_user_databases(&meta_root)?.unwrap_or_default()
    };

    if user_ids.is_empty() {
        println!("No users found");
        return Ok(());
    }

    // Print header
    println!("{:<20} {:<15} {:<15} {:<20}",
        "User ID", "Bucket Count", "Object Count", "Total Size");
    println!("{:-<70}", "");

    for user_id in user_ids {
        let user_meta_path = meta_root.join(format!("user_{}", user_id));

        if !user_meta_path.exists() {
            println!("{:<20} (database not found)", user_id);
            continue;
        }

        let meta_store = create_meta_store(user_meta_path, storage_engine);

        // Get bucket count from _BUCKETS tree
        let buckets = meta_store.list_buckets().unwrap_or_default();
        let bucket_count = buckets.len();

        // Count objects across all buckets and sum sizes
        let mut total_objects = 0usize;
        let mut total_size = 0u64;

        for bucket in buckets {
            let bucket_tree = match meta_store.get_bucket_ext(&bucket.name()) {
                Ok(tree) => tree,
                Err(_) => continue,
            };

            for (_key, obj) in bucket_tree.range_filter(None, None, None) {
                total_objects += 1;
                total_size += obj.size();
            }
        }

        println!("{:<20} {:<15} {:<15} {:<20}",
            user_id,
            bucket_count,
            total_objects,
            format_bytes(total_size),
        );
    }

    Ok(())
}

/// List all buckets
pub fn list_buckets(
    meta_root: PathBuf,
    storage_engine: StorageEngine,
    users_config: Option<PathBuf>,
    user_filter: Option<String>,
) -> Result<()> {
    let is_multi_user = users_config.is_some();

    if is_multi_user {
        // Multi-user mode
        let user_ids = if let Some(user_id) = user_filter {
            vec![user_id]
        } else {
            detect_user_databases(&meta_root)?.unwrap_or_default()
        };

        // Print header
        println!("{:<20} {:<30} {:<15} {:<20}",
            "Owner", "Bucket Name", "Object Count", "Created At");
        println!("{:-<85}", "");

        for user_id in user_ids {
            let user_meta_path = meta_root.join(format!("user_{}", user_id));

            if !user_meta_path.exists() {
                continue;
            }

            let meta_store = create_meta_store(user_meta_path, storage_engine);
            let buckets = meta_store.list_buckets().unwrap_or_default();

            for bucket in buckets {
                // Count objects in bucket
                let bucket_tree = meta_store.get_bucket_ext(&bucket.name()).ok();
                let object_count = if let Some(tree) = bucket_tree {
                    tree.range_filter(None, None, None).count()
                } else {
                    0
                };

                let datetime = chrono::DateTime::<chrono::Utc>::from(bucket.ctime());

                println!("{:<20} {:<30} {:<15} {:<20}",
                    user_id,
                    bucket.name(),
                    object_count,
                    datetime.format("%Y-%m-%d %H:%M:%S"),
                );
            }
        }
    } else {
        // Single-user mode
        let meta_store = create_meta_store(meta_root, storage_engine);
        let buckets = meta_store.list_buckets()?;

        if buckets.is_empty() {
            println!("No buckets found");
            return Ok(());
        }

        // Print header
        println!("{:<30} {:<15} {:<20}",
            "Bucket Name", "Object Count", "Created At");
        println!("{:-<65}", "");

        for bucket in buckets {
            // Count objects in bucket
            let bucket_tree = meta_store.get_bucket_ext(&bucket.name()).ok();
            let object_count = if let Some(tree) = bucket_tree {
                tree.range_filter(None, None, None).count()
            } else {
                0
            };

            let datetime = chrono::DateTime::<chrono::Utc>::from(bucket.ctime());

            println!("{:<30} {:<15} {:<20}",
                bucket.name(),
                object_count,
                datetime.format("%Y-%m-%d %H:%M:%S"),
            );
        }
    }

    Ok(())
}

/// Show statistics for a specific bucket
pub fn bucket_stats(
    meta_root: PathBuf,
    storage_engine: StorageEngine,
    users_config: Option<PathBuf>,
    bucket: String,
    user_filter: Option<String>,
) -> Result<()> {
    let is_multi_user = users_config.is_some();

    let meta_store = if is_multi_user {
        if let Some(user_id) = user_filter {
            let user_meta_path = meta_root.join(format!("user_{}", user_id));
            create_meta_store(user_meta_path, storage_engine)
        } else {
            bail!("In multi-user mode, --user parameter is required for bucket-stats");
        }
    } else {
        create_meta_store(meta_root, storage_engine)
    };

    // Check if bucket exists
    if !meta_store.bucket_exists(&bucket)? {
        bail!("Bucket '{}' not found", bucket);
    }

    let bucket_tree = meta_store.get_bucket_ext(&bucket)?;

    let mut object_count = 0usize;
    let mut total_size = 0u64;
    let mut unique_blocks = std::collections::HashSet::new();
    let mut multipart_count = 0usize;
    let mut inline_count = 0usize;

    for (_key, obj) in bucket_tree.range_filter(None, None, None) {
        object_count += 1;
        total_size += obj.size();

        match obj.object_type() {
            ObjectType::Multipart => multipart_count += 1,
            ObjectType::Inline => inline_count += 1,
            _ => {}
        }

        // Collect unique blocks
        for block_id in obj.blocks() {
            unique_blocks.insert(*block_id);
        }
    }

    println!("Bucket: {}", bucket);
    println!("Object count: {}", object_count);
    println!("Total size: {} ({} bytes)", format_bytes(total_size), total_size);
    println!("Unique blocks: {}", unique_blocks.len());
    println!("Multipart objects: {}", multipart_count);
    println!("Inline objects: {}", inline_count);

    if object_count > 0 {
        let avg_size = total_size / object_count as u64;
        println!("Average object size: {}", format_bytes(avg_size));
    }

    Ok(())
}

/// Show block storage statistics and deduplication ratio
pub fn block_stats(
    meta_root: PathBuf,
    storage_engine: StorageEngine,
    _users_config: Option<PathBuf>,
) -> Result<()> {
    // Block storage is always in the shared database
    let shared_store = create_meta_store(meta_root, storage_engine);
    let block_tree = shared_store.get_block_tree()?;

    let mut total_blocks = 0usize;
    let mut total_block_size = 0u64;
    let mut total_ref_count = 0usize;
    let mut ref_count_distribution: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();

    for item in block_tree.iter_all() {
        let (_block_id, block) = match item {
            Ok((id, b)) => (id, b),
            Err(_) => continue,
        };
        total_blocks += 1;
        total_block_size += block.size() as u64;
        let rc = block.rc();
        total_ref_count += rc;
        *ref_count_distribution.entry(rc).or_insert(0) += 1;
    }

    println!("Block Statistics:");
    println!("  Total blocks: {}", total_blocks);
    println!("  Total block storage: {} ({} bytes)", format_bytes(total_block_size), total_block_size);
    println!("  Total references: {}", total_ref_count);

    if total_blocks > 0 {
        let avg_refs = total_ref_count as f64 / total_blocks as f64;
        println!("  Average references per block: {:.2}", avg_refs);

        // Deduplication ratio: how much storage is saved
        let dedupe_ratio = total_ref_count as f64 / total_blocks as f64;
        println!("  Deduplication ratio: {:.2}x", dedupe_ratio);

        let savings_pct = ((dedupe_ratio - 1.0) / dedupe_ratio) * 100.0;
        println!("  Storage savings: {:.1}%", savings_pct);
    }

    println!("\nReference count distribution:");
    let mut counts: Vec<_> = ref_count_distribution.iter().collect();
    counts.sort_by_key(|(rc, _)| *rc);

    for (rc, count) in counts.iter().take(10) {
        println!("  RC={}: {} blocks", rc, count);
    }

    if counts.len() > 10 {
        println!("  ... ({} more)", counts.len() - 10);
    }

    Ok(())
}

/// Show detailed information about a specific object
pub fn object_info(
    meta_root: PathBuf,
    storage_engine: StorageEngine,
    users_config: Option<PathBuf>,
    bucket: String,
    key: String,
    user_filter: Option<String>,
) -> Result<()> {
    let is_multi_user = users_config.is_some();

    let meta_store = if is_multi_user {
        if let Some(user_id) = user_filter {
            let user_meta_path = meta_root.join(format!("user_{}", user_id));
            create_meta_store(user_meta_path, storage_engine)
        } else {
            bail!("In multi-user mode, --user parameter is required for object-info");
        }
    } else {
        create_meta_store(meta_root, storage_engine)
    };

    // Get object metadata
    let obj = match meta_store.get_meta(&bucket, &key)? {
        Some(o) => o,
        None => bail!("Object '{}' not found in bucket '{}'", key, bucket),
    };

    println!("Object: {}/{}", bucket, key);
    println!("Size: {} ({} bytes)", format_bytes(obj.size()), obj.size());
    println!("Type: {:?}", obj.object_type());
    println!("Hash: {}", hex::encode(obj.hash()));

    let created_at = obj.last_modified();
    let datetime = chrono::DateTime::<chrono::Utc>::from(created_at);
    println!("Created: {}", datetime.format("%Y-%m-%d %H:%M:%S"));

    if obj.is_inlined() {
        if let Some(data) = obj.inlined() {
            println!("Inline data: {} bytes", data.len());
        }
    } else {
        let blocks = obj.blocks();
        println!("Blocks: {}", blocks.len());

        if blocks.len() <= 10 {
            println!("\nBlock IDs:");
            for (i, block_id) in blocks.iter().enumerate() {
                println!("  {}: {}", i + 1, hex::encode(block_id));
            }
        } else {
            println!("\nFirst 10 block IDs:");
            for (i, block_id) in blocks.iter().take(10).enumerate() {
                println!("  {}: {}", i + 1, hex::encode(block_id));
            }
            println!("  ... ({} more blocks)", blocks.len() - 10);
        }

        if let ObjectType::Multipart = obj.object_type() {
            // Extract part count from ObjectData
            if let ObjectData::MultiPart { parts, .. } = obj.data() {
                println!("\nMultipart upload: {} parts", parts);
            }
        }
    }

    Ok(())
}

/// Format bytes in human-readable format
fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];

    if bytes == 0 {
        return "0 B".to_string();
    }

    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_index])
}
