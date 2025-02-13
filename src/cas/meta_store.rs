use super::{
    block::{Block, BlockID},
    bucket_meta::BucketMeta,
    meta_errors::MetaError,
    multipart::MultiPart,
    object::Object,
};
use std::fmt::Debug;

/// MetaStore is the interface that defines the methods to interact with the metadata store.
///
/// Current implementation of the bucket, block, path, and multipart trees are the same,
/// the difference is only the partition/tree name.
/// But we separate the API to make it easier to extend in the future,
/// and give flexibility to the implementer to have different implementations for each tree.
pub trait MetaStore: Send + Sync + Debug + 'static {
    /// returns tree which contains all the buckets.
    /// This tree is used to store the bucket lists and provide
    /// the CRUD for the bucket list.
    fn get_allbuckets_tree(&self) -> Result<Box<dyn AllBucketsTree>, MetaError>;

    fn get_bucket_tree(&self, bucket_name: &str) -> Result<Box<dyn BucketTree>, MetaError>;

    /// get_bucket_ext returns the tree for specific bucket with the extended methods.
    fn get_bucket_ext(&self, name: &str)
        -> Result<Box<dyn BucketTreeExt + Send + Sync>, MetaError>;

    /// get_block_tree returns the block meta tree.
    /// This tree is used to store the data block metadata.
    fn get_block_tree(&self) -> Result<Box<dyn BlockTree>, MetaError>;

    /// get_path_tree returns the path meta tree
    /// This tree is used to store the file path metadata.
    fn get_path_tree(&self) -> Result<Box<dyn BaseMetaTree>, MetaError>;

    /// get_multipart_tree returns the multipart meta tree
    fn get_multipart_tree(&self) -> Result<Box<dyn MultiPartTree>, MetaError>;

    /// bucket_exists returns true if the bucket exists.
    fn bucket_exists(&self, bucket_name: &str) -> Result<bool, MetaError>;

    /// drop_bucket drops the bucket with the given name.
    fn drop_bucket(&self, name: &str) -> Result<(), MetaError>;

    /// insert_bucket inserts raw representation of the bucket into the meta store.
    fn insert_bucket(&self, bucket_name: &str, raw_bucket: Vec<u8>) -> Result<(), MetaError>;

    /// Get a list of all buckets in the system.
    /// TODO: this should be paginated and return a stream.
    fn list_buckets(&self) -> Result<Vec<BucketMeta>, MetaError>;

    /// delete all objects in a bucket for the given key.
    /// it returns a list of blocks that were deleted.
    fn delete_objects(&self, bucket: &str, key: &str) -> Result<Vec<Block>, MetaError>;

    // Check if the hash is present in the block map. If it is not, try to find a path, and
    // insert it.
    // it returns true if the block was not exists
    fn write_block_and_path_meta(
        &self,
        block_hash: BlockID,
        data_len: usize,
        key_has_block: bool,
    ) -> Result<bool, MetaError>;
}

pub trait BaseMetaTree: Send + Sync {
    /// insert inserts a key value pair into the tree.
    fn insert(&self, key: &[u8], value: Vec<u8>) -> Result<(), MetaError>;

    /// remove removes a key from the tree.
    fn remove(&self, key: &[u8]) -> Result<(), MetaError>;

    fn contains_key(&self, key: &[u8]) -> Result<bool, MetaError>;
}

pub trait AllBucketsTree: BaseMetaTree {}

impl<T: BaseMetaTree> AllBucketsTree for T {}

pub trait BucketTree: BaseMetaTree {
    /// insert_meta inserts a metadata Object into the bucket
    fn insert_meta(&self, key: &str, raw_obj: Vec<u8>) -> Result<(), MetaError>;

    /// get_meta returns the Object metadata for the given bucket and key.
    /// We return the Object struct instead of the raw bytes for performance reason.
    fn get_meta(&self, key: &str) -> Result<Object, MetaError>;
}

pub trait BlockTree: Send + Sync {
    /// get_block_obj returns the `Object` for the given key.
    fn get_block(&self, key: &[u8]) -> Result<Block, MetaError>;
}

pub trait MultiPartTree: BaseMetaTree {
    /// get_multipart_part_obj returns the `MultiPart` for the given key.
    fn get_multipart_part(&self, key: &[u8]) -> Result<MultiPart, MetaError>;
}

pub trait BucketTreeExt: BaseMetaTree {
    // get all keys of the bucket
    // TODO : make it paginated
    fn get_bucket_keys(&self) -> Box<dyn Iterator<Item = Result<Vec<u8>, MetaError>> + Send>;

    fn range_filter<'a>(
        &'a self,
        start_after: Option<String>,
        prefix: Option<String>,
        continuation_token: Option<String>,
    ) -> Box<(dyn Iterator<Item = (String, Object)> + 'a)>;
}

//pub trait BucketTreeExt: BaseMetaTree + MetaTreeExt {}
