use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tracing::debug;

use cas_storage::{CasFS, SharedBlockStore, StorageEngine};
use cas_storage::Durability;
use crate::metrics::SharedMetrics;

/// Error types for user routing
#[derive(Debug)]
pub enum RouterError {
    UnknownUser(String),
    AuthenticationFailed,
    CreationFailed(String),
}

impl std::fmt::Display for RouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouterError::UnknownUser(key) => write!(f, "Unknown user with access key: {}", key),
            RouterError::AuthenticationFailed => write!(f, "Authentication failed"),
            RouterError::CreationFailed(msg) => write!(f, "Failed to create CasFS: {}", msg),
        }
    }
}

impl std::error::Error for RouterError {}

/// UserRouter manages per-user CasFS instances with lazy initialization
pub struct UserRouter {
    shared_block_store: Arc<SharedBlockStore>,
    casfs_cache: Arc<RwLock<HashMap<String, Arc<CasFS>>>>,
    fs_root: PathBuf,
    meta_root: PathBuf,
    metrics: SharedMetrics,
    storage_engine: StorageEngine,
    inlined_metadata_size: Option<usize>,
    durability: Option<Durability>,
}

impl UserRouter {
    /// Create a new UserRouter with lazy CasFS initialization
    ///
    /// # Arguments
    /// * `shared_block_store` - Shared block store (singleton)
    /// * `fs_root` - Root directory for block storage
    /// * `meta_root` - Root directory for metadata
    /// * `metrics` - Metrics collector
    /// * `storage_engine` - Storage engine for user metadata
    /// * `inlined_metadata_size` - Maximum size for inlined metadata
    /// * `durability` - Durability level for transactions
    pub fn new(
        shared_block_store: Arc<SharedBlockStore>,
        fs_root: PathBuf,
        meta_root: PathBuf,
        metrics: SharedMetrics,
        storage_engine: StorageEngine,
        inlined_metadata_size: Option<usize>,
        durability: Option<Durability>,
    ) -> Self {
        Self {
            shared_block_store,
            casfs_cache: Arc::new(RwLock::new(HashMap::new())),
            fs_root,
            meta_root,
            metrics,
            storage_engine,
            inlined_metadata_size,
            durability,
        }
    }

    /// Creates a new CasFS instance for a user (called internally on cache miss)
    fn create_casfs_for_user(&self, user_id: &str) -> Arc<CasFS> {
        debug!("Creating new CasFS instance for user: {}", user_id);

        let user_meta_path = self.meta_root.join(format!("user_{}", user_id));

        let casfs = CasFS::new_multi_user(
            self.fs_root.clone(),
            user_meta_path,
            self.shared_block_store.block_tree(),
            self.shared_block_store.path_tree(),
            self.shared_block_store.multipart_tree(),
            self.shared_block_store.meta_store(),
            self.metrics.to_cas_metrics(),
            self.storage_engine,
            self.inlined_metadata_size,
            self.durability,
        );

        Arc::new(casfs)
    }

    /// Get CasFS instance by user_id with lazy initialization
    ///
    /// # Arguments
    /// * `user_id` - User identifier
    ///
    /// # Returns
    /// * `Result<Arc<CasFS>, RouterError>` - CasFS instance or error
    pub fn get_casfs_by_user_id(&self, user_id: &str) -> Result<Arc<CasFS>, RouterError> {
        // First try with read lock (fast path)
        {
            let cache = self.casfs_cache.read().unwrap();
            if let Some(casfs) = cache.get(user_id) {
                return Ok(casfs.clone());
            }
        }

        // Cache miss - create new instance with write lock
        let mut cache = self.casfs_cache.write().unwrap();

        // Double-check after acquiring write lock (another thread might have created it)
        if let Some(casfs) = cache.get(user_id) {
            return Ok(casfs.clone());
        }

        // Create new CasFS for this user
        let casfs = self.create_casfs_for_user(user_id);
        cache.insert(user_id.to_string(), casfs.clone());

        Ok(casfs)
    }

    /// Get SharedMetrics for metrics collection
    pub fn metrics(&self) -> &SharedMetrics {
        &self.metrics
    }
}
