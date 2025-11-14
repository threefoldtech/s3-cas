use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::cas::{CasFS, SharedBlockStore, StorageEngine};
use crate::metastore::Durability;
use crate::metrics::SharedMetrics;

use super::user_config::{UserAuth, UsersConfig};

/// Error types for user routing
#[derive(Debug)]
pub enum RouterError {
    UnknownUser(String),
    AuthenticationFailed,
}

impl std::fmt::Display for RouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouterError::UnknownUser(key) => write!(f, "Unknown user with access key: {}", key),
            RouterError::AuthenticationFailed => write!(f, "Authentication failed"),
        }
    }
}

impl std::error::Error for RouterError {}

/// UserRouter manages per-user CasFS instances and request routing
pub struct UserRouter {
    auth: UserAuth,
    casfs_instances: HashMap<String, Arc<CasFS>>,
}

impl UserRouter {
    /// Create a new UserRouter with pre-created CasFS instances for all users
    ///
    /// # Arguments
    /// * `users_config` - User configuration from users.toml
    /// * `shared_block_store` - Shared block store (singleton)
    /// * `fs_root` - Root directory for block storage
    /// * `meta_root` - Root directory for metadata
    /// * `metrics` - Metrics collector
    /// * `storage_engine` - Storage engine for user metadata
    /// * `inlined_metadata_size` - Maximum size for inlined metadata
    /// * `durability` - Durability level for transactions
    pub fn new(
        users_config: UsersConfig,
        shared_block_store: &SharedBlockStore,
        fs_root: PathBuf,
        meta_root: PathBuf,
        metrics: SharedMetrics,
        storage_engine: StorageEngine,
        inlined_metadata_size: Option<usize>,
        durability: Option<Durability>,
    ) -> Self {
        let auth = UserAuth::new(users_config.clone());
        let mut casfs_instances = HashMap::new();

        // Create CasFS instance for each user at startup
        for user_id in auth.user_ids() {
            let user_meta_path = meta_root.join(format!("user_{}", user_id));

            let casfs = CasFS::new_multi_user(
                fs_root.clone(),
                user_meta_path,
                shared_block_store.block_tree(),
                shared_block_store.path_tree(),
                shared_block_store.multipart_tree(),
                metrics.clone(),
                storage_engine,
                inlined_metadata_size,
                durability,
            );

            casfs_instances.insert(user_id.clone(), Arc::new(casfs));
        }

        Self {
            auth,
            casfs_instances,
        }
    }

    /// Get CasFS instance for a given access key
    ///
    /// # Arguments
    /// * `access_key` - S3 access key from request
    ///
    /// # Returns
    /// * `Result<Arc<CasFS>, RouterError>` - CasFS instance or error
    pub fn get_casfs(&self, access_key: &str) -> Result<Arc<CasFS>, RouterError> {
        let user_id = self
            .auth
            .get_user_id(access_key)
            .ok_or_else(|| RouterError::UnknownUser(access_key.to_string()))?;

        self.casfs_instances
            .get(user_id)
            .cloned()
            .ok_or(RouterError::AuthenticationFailed)
    }

    /// Get UserAuth for authentication checks
    pub fn auth(&self) -> &UserAuth {
        &self.auth
    }
}
