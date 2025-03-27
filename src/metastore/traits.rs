use super::{
    MetaError,
    block::{Block, BlockID},
    bucket_meta::BucketMeta,
    object::Object,
};

use std::fmt::Debug;
use std::str::FromStr;

/// MetaStore is the interface that defines the methods to interact with the metadata store.
///
/// It provides methods for managing buckets, objects, and blocks in the metadata store.
/// Implementations should ensure thread safety and proper error handling.
pub trait MetaStore: Send + Sync + Debug + 'static {
    /// Returns the maximum length of the data that can be inlined in the metadata object
    fn max_inlined_data_length(&self) -> usize;

    /// Returns tree which contains all the buckets.
    /// This tree is used to store the bucket lists and provide
    /// the CRUD for the bucket list.
    fn get_allbuckets_tree(&self) -> Result<Box<dyn AllBucketsTree>, MetaError>;

    /// Returns the tree for specific bucket with the extended methods
    /// Used to provide additional methods for the bucket like the range and list methods.
    fn get_bucket_ext(&self, name: &str)
    -> Result<Box<dyn BucketTreeExt + Send + Sync>, MetaError>;

    /// Returns the block meta tree.
    /// This tree is used to store the data block metadata.
    fn get_block_tree(&self) -> Result<Box<dyn BlockTree>, MetaError>;

    /// Returns the tree with the given name.
    /// It is usually used if the app needs to store some metadata for a specific purpose.
    fn get_tree(&self, name: &str) -> Result<Box<dyn BaseMetaTree>, MetaError>;

    /// Returns the path meta tree
    /// This tree is used to store the file path metadata.
    fn get_path_tree(&self) -> Result<Box<dyn BaseMetaTree>, MetaError>;

    /// Checks if a bucket with the given name exists.
    fn bucket_exists(&self, bucket_name: &str) -> Result<bool, MetaError>;

    /// Drops the bucket with the given name.
    fn drop_bucket(&self, name: &str) -> Result<(), MetaError>;

    /// Inserts raw representation of the bucket into the meta store.
    fn insert_bucket(&self, bucket_name: &str, raw_bucket: Vec<u8>) -> Result<(), MetaError>;

    /// Creates a new bucket with the given metadata.
    ///
    /// This method checks if the bucket already exists, and if not, creates it.
    fn create_bucket(&self, bucket: &BucketMeta) -> Result<(), MetaError> {
        // Default implementation that uses insert_bucket
        if self.bucket_exists(bucket.name())? {
            return Err(MetaError::BucketAlreadyExists(bucket.name().to_string()));
        }
        let raw_bucket = bucket.to_vec();
        self.insert_bucket(bucket.name(), raw_bucket)?;
        // Create the bucket tree
        self.get_bucket_ext(bucket.name())?;
        Ok(())
    }

    /// Gets a list of all buckets in the system.
    /// TODO: this should be paginated and return a stream.
    fn list_buckets(&self) -> Result<Vec<BucketMeta>, MetaError>;

    /// Inserts a metadata Object into the bucket
    fn insert_meta(&self, bucket_name: &str, key: &str, raw_obj: Vec<u8>) -> Result<(), MetaError>;

    /// Gets the Object metadata for the given bucket and key.
    fn get_meta(&self, bucket_name: &str, key: &str) -> Result<Option<Object>, MetaError>;

    /// Gets an object from the bucket with the given key.
    ///
    /// This is a convenience method that uses get_meta.
    fn get_object(&self, bucket_name: &str, key: &str) -> Result<Option<Object>, MetaError> {
        // Default implementation that uses get_meta
        self.get_meta(bucket_name, key)
    }

    /// Puts an object into the bucket with the given key.
    ///
    /// This is a convenience method that uses insert_meta.
    fn put_object(&self, bucket_name: &str, key: &str, obj: &Object) -> Result<(), MetaError> {
        // Default implementation that uses insert_meta
        self.insert_meta(bucket_name, key, obj.to_vec())
    }

    /// Deletes an object in a bucket for the given key.
    ///
    /// It should do at least the following:
    /// - get all the blocks from the object
    /// - decrements the refcount of all blocks, then removes blocks which are no longer referenced.
    /// - and return the deleted block IDs, so that the caller can remove the blocks from the storage.
    fn delete_object(&self, bucket: &str, key: &str) -> Result<Vec<BlockID>, MetaError>;

    /// Begins a new transaction.
    ///
    /// Returns a Result containing either a boxed Transaction or an error.
    fn begin_transaction(&self) -> Result<Box<dyn Transaction>, MetaError>;

    /// Returns the number of keys of the bucket, block, and path trees.
    fn num_keys(&self) -> (usize, usize, usize);

    /// Returns the disk space used by the metadata store.
    fn disk_space(&self) -> u64;
}

/// Transaction represents a database transaction.
///
/// Transactions allow for atomic operations on the metadata store.
/// They must be either committed or rolled back.
pub trait Transaction: Send + Sync {
    /// Commits the transaction, making all changes permanent.
    fn commit(self: Box<Self>) -> Result<(), MetaError>;

    /// Rolls back the transaction, discarding all changes.
    fn rollback(self: Box<Self>);

    /// Writes a block to the transaction.
    ///
    /// Returns a tuple containing:
    /// - A boolean indicating whether the block was newly created
    /// - The Block object
    fn write_block(
        &mut self,
        block_hash: BlockID,
        data_len: usize,
        key_has_block: bool,
    ) -> Result<(bool, Block), MetaError>;
}

/// BaseMetaTree provides basic tree operations for metadata storage.
pub trait BaseMetaTree: Send + Sync {
    /// Inserts a key-value pair into the tree.
    fn insert(&self, key: &[u8], value: Vec<u8>) -> Result<(), MetaError>;

    /// Removes a key from the tree.
    fn remove(&self, key: &[u8]) -> Result<(), MetaError>;

    /// Checks if the tree contains the given key.
    fn contains_key(&self, key: &[u8]) -> Result<bool, MetaError>;

    /// Gets the value associated with the given key.
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, MetaError>;
}

/// AllBucketsTree represents a tree that stores all buckets.
pub trait AllBucketsTree: BaseMetaTree {}

impl<T: BaseMetaTree> AllBucketsTree for T {}

/// BlockTree provides operations for managing block metadata.
pub trait BlockTree: Send + Sync {
    /// Gets the Block for the given key.
    fn get_block(&self, key: &[u8]) -> Result<Option<Block>, MetaError>;

    #[cfg(test)]
    fn len(&self) -> Result<usize, MetaError>;
}

/// BucketTreeExt provides extended operations for bucket trees.
pub trait BucketTreeExt: BaseMetaTree {
    /// Gets all keys of the bucket.
    /// TODO: make it paginated
    fn get_bucket_keys(&self) -> Box<dyn Iterator<Item = Result<Vec<u8>, MetaError>> + Send>;

    /// Filters objects in the bucket based on prefix, start_after, and continuation_token.
    fn range_filter<'a>(
        &'a self,
        start_after: Option<String>,
        prefix: Option<String>,
        continuation_token: Option<String>,
    ) -> Box<(dyn Iterator<Item = (String, Object)> + 'a)>;
}

#[derive(Debug, Clone, Copy)]
pub enum Durability {
    Buffer,
    Fsync,
    Fdatasync,
}

impl FromStr for Durability {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "buffer" => Ok(Durability::Buffer),
            "fsync" => Ok(Durability::Fsync),
            "fdatasync" => Ok(Durability::Fdatasync),
            _ => Err(format!("Unknown durability option: {}", s)),
        }
    }
}
