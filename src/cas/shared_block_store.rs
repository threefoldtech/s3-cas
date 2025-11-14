use std::path::PathBuf;
use std::sync::Arc;

use crate::metastore::{
    BaseMetaTree, BlockTree, Durability, FjallStore, FjallStoreNotx, MetaError, MetaStore,
};

use super::StorageEngine;

/// SharedBlockStore manages the shared block metadata (_BLOCKS and _PATHS trees)
/// that is accessed by all users for block refcounting and path allocation.
///
/// This is created once at startup and shared across all CasFS instances.
pub struct SharedBlockStore {
    meta_store: Arc<MetaStore>,
    block_tree: Arc<BlockTree>,
    path_tree: Arc<dyn BaseMetaTree>,
}

impl SharedBlockStore {
    /// Create a new SharedBlockStore
    ///
    /// # Arguments
    /// * `path` - Path to the shared block metadata DB (e.g., /meta_root/blocks/db)
    /// * `storage_engine` - Storage engine (Fjall or FjallNotx)
    /// * `inlined_metadata_size` - Maximum size for inlined metadata
    /// * `durability` - Durability level for transactions
    pub fn new(
        mut path: PathBuf,
        storage_engine: StorageEngine,
        inlined_metadata_size: Option<usize>,
        durability: Option<Durability>,
    ) -> Result<Self, MetaError> {
        path.push("db");

        let meta_store = match storage_engine {
            StorageEngine::Fjall => {
                let store = FjallStore::new(path, inlined_metadata_size, durability);
                MetaStore::new(store, inlined_metadata_size)
            }
            StorageEngine::FjallNotx => {
                let store = FjallStoreNotx::new(path, inlined_metadata_size);
                MetaStore::new(store, inlined_metadata_size)
            }
        };

        let block_tree = meta_store.get_block_tree()?;
        let path_tree = meta_store.get_path_tree()?;

        Ok(Self {
            meta_store: Arc::new(meta_store),
            block_tree: Arc::new(block_tree),
            path_tree,
        })
    }

    /// Get a reference to the shared block tree
    pub fn block_tree(&self) -> Arc<BlockTree> {
        Arc::clone(&self.block_tree)
    }

    /// Get a reference to the shared path tree
    pub fn path_tree(&self) -> Arc<dyn BaseMetaTree> {
        Arc::clone(&self.path_tree)
    }
}
