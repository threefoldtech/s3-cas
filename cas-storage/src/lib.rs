//! # CAS Storage Library
//!
//! A content-addressable storage library with block-level deduplication,
//! reference counting, and multi-user support.
//!
//! ## Features
//!
//! - **Content-Addressable Storage**: Objects chunked into 1 MiB blocks, identified by MD5 hash
//! - **Block Deduplication**: Duplicate blocks stored only once with reference counting
//! - **Multi-User Support**: Shared block storage with isolated metadata per user
//! - **Pluggable Backends**: Support for Fjall (transactional) and FjallNotx (non-transactional)
//! - **Inline Data**: Small objects can be stored directly in metadata
//! - **Streaming I/O**: Efficient streaming reads and writes
//!
//! ## Example: Single-User Storage
//!
//! ```no_run
//! use cas_storage::{CasFS, StorageEngine, Durability};
//! use std::path::PathBuf;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create storage instance
//! let casfs = CasFS::new(
//!     PathBuf::from("./data/blocks"),
//!     PathBuf::from("./data/meta"),
//!     Default::default(),  // metrics
//!     StorageEngine::Fjall,
//!     None,  // inline_metadata_size
//!     Some(Durability::Immediate),
//! );
//!
//! // Create bucket
//! casfs.create_bucket("my-bucket")?;
//!
//! // Store object
//! // let data_stream = ...; // ByteStream
//! // let object = casfs.store_single_object_and_meta(
//! //     "my-bucket",
//! //     "file.txt",
//! //     data_stream,
//! // ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Example: Multi-User Storage
//!
//! ```no_run
//! use cas_storage::{SharedBlockStore, CasFS, StorageEngine, Durability};
//! use std::path::PathBuf;
//! use std::sync::Arc;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create shared block store (once)
//! let shared = Arc::new(SharedBlockStore::new(
//!     PathBuf::from("./data/meta"),
//!     StorageEngine::Fjall,
//!     None,
//!     Some(Durability::Immediate),
//! )?);
//!
//! // Create per-user CasFS instances
//! let user1_casfs = CasFS::new_multi_user(
//!     PathBuf::from("./data/blocks"),
//!     PathBuf::from("./data/meta/user_alice"),
//!     shared.block_tree(),
//!     shared.path_tree(),
//!     shared.multipart_tree(),
//!     shared.meta_store(),
//!     Default::default(),
//!     StorageEngine::Fjall,
//!     None,
//!     Some(Durability::Immediate),
//! );
//! # Ok(())
//! # }
//! ```

pub mod cas;
pub mod metastore;
pub mod metrics;

// Re-export main types from metastore
pub use metastore::{
    // Metadata structures
    Block, BlockID, BucketMeta, Object, ObjectData, ObjectType,
    // Storage abstractions
    BaseMetaTree, BlockTree, MetaError, MetaStore, MetaTreeExt, Store, Transaction,
    // Storage backends
    Durability, FjallStore, FjallStoreNotx,
};

// Re-export main types from cas
pub use cas::{
    // Core storage
    CasFS, SharedBlockStore, StorageEngine,
    // Multipart support
    multipart::{MultiPart, MultiPartTree},
    // Streaming and utilities
    block_stream::BlockStream,
    range_request::{RangeRequest, parse_range_request},
};

// Re-export metrics types
pub use metrics::{MetricsCollector, NoOpMetrics, SharedMetrics};
