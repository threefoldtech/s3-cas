use std::convert::TryFrom;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;

use fjall;

use crate::metastore::{
    AllBucketsTree, BaseMetaTree, Block, BlockID, BlockTree, BucketMeta, BucketTreeExt, MetaError,
    MetaStore, Object, Transaction, BLOCKID_SIZE,
};

#[derive(Clone)]
pub struct FjallStoreNotx {
    keyspace: Arc<fjall::Keyspace>,
    bucket_partition: Arc<fjall::PartitionHandle>,
    block_partition: Arc<fjall::PartitionHandle>,
    path_partition: Arc<fjall::PartitionHandle>,
    inlined_metadata_size: usize,
}

impl std::fmt::Debug for FjallStoreNotx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FjallStoreNotx")
            .field("keyspace", &"<fjall::Keyspace>")
            .finish()
    }
}

impl FjallStoreNotx {
    pub fn new(path: PathBuf, inlined_metadata_size: Option<usize>) -> Self {
        tracing::info!("Opening fjall store at {:?}", path);
        const BUCKET_META_PARTITION: &str = "_BUCKETS";
        const BLOCK_PARTITION: &str = "_BLOCKS";
        const PATH_PARTITION: &str = "_PATHS";

        let keyspace = fjall::Config::new(path).open().unwrap();
        let bucket_partition = keyspace
            .open_partition(BUCKET_META_PARTITION, Default::default())
            .unwrap();
        let block_partition = keyspace
            .open_partition(BLOCK_PARTITION, Default::default())
            .unwrap();
        let path_partition = keyspace
            .open_partition(PATH_PARTITION, Default::default())
            .unwrap();
        // setting very low will practically disable it by default
        let inlined_metadata_size = inlined_metadata_size.unwrap_or(1);

        Self {
            keyspace: Arc::new(keyspace),
            bucket_partition: Arc::new(bucket_partition),
            block_partition: Arc::new(block_partition),
            path_partition: Arc::new(path_partition),
            inlined_metadata_size,
        }
    }

    fn get_partition(&self, name: &str) -> Result<fjall::PartitionHandle, MetaError> {
        match self.keyspace.open_partition(name, Default::default()) {
            Ok(partition) => Ok(partition),
            Err(e) => Err(MetaError::OtherDBError(e.to_string())),
        }
    }
}

impl MetaStore for FjallStoreNotx {
    fn max_inlined_data_length(&self) -> usize {
        if self.inlined_metadata_size < Object::minimum_inline_metadata_size() {
            return 0;
        }
        self.inlined_metadata_size - Object::minimum_inline_metadata_size()
    }

    fn get_bucket_ext(
        &self,
        name: &str,
    ) -> Result<Box<dyn BucketTreeExt + Send + Sync>, MetaError> {
        let bucket = self.get_partition(name)?;
        Ok(Box::new(FjallTreeNotx::new(Arc::new(bucket))))
    }

    fn get_allbuckets_tree(&self) -> Result<Box<dyn AllBucketsTree>, MetaError> {
        Ok(Box::new(FjallTreeNotx::new(self.bucket_partition.clone())))
    }

    fn get_block_tree(&self) -> Result<Box<dyn BlockTree>, MetaError> {
        Ok(Box::new(FjallTreeNotx::new(self.block_partition.clone())))
    }

    fn get_tree(&self, name: &str) -> Result<Box<dyn BaseMetaTree>, MetaError> {
        let partition = self.get_partition(name)?;
        Ok(Box::new(FjallTreeNotx::new(Arc::new(partition))))
    }

    fn get_path_tree(&self) -> Result<Box<dyn BaseMetaTree>, MetaError> {
        Ok(Box::new(FjallTreeNotx::new(self.path_partition.clone())))
    }

    fn drop_bucket(&self, name: &str) -> Result<(), MetaError> {
        let partition = self.get_partition(name)?;
        match self.keyspace.delete_partition(partition) {
            Ok(_) => Ok(()),
            Err(e) => Err(MetaError::OtherDBError(e.to_string())),
        }
    }

    fn insert_bucket(&self, bucket_name: &str, raw_bucket: Vec<u8>) -> Result<(), MetaError> {
        self.bucket_partition
            .insert(bucket_name, raw_bucket)
            .map_err(|e| MetaError::InsertError(e.to_string()))?;

        match self.get_partition(bucket_name) {
            // get partition to create it
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn bucket_exists(&self, bucket_name: &str) -> Result<bool, MetaError> {
        let exists = self.keyspace.partition_exists(bucket_name);
        Ok(exists)
    }

    /// Get a list of all buckets in the system.
    fn list_buckets(&self) -> Result<Vec<BucketMeta>, MetaError> {
        let buckets = self
            .bucket_partition
            .range::<Vec<u8>, _>(std::ops::RangeFull) // Specify type parameter for range
            .filter_map(|raw_value| {
                let value = match raw_value {
                    Err(_) => return None,
                    Ok((_, value)) => value,
                };
                // unwrap here is fine as it means the db is corrupt
                let bucket_meta = BucketMeta::try_from(&*value).expect("Corrupted bucket metadata");
                Some(bucket_meta)
            })
            .collect();
        Ok(buckets)
    }

    fn insert_meta(&self, bucket_name: &str, key: &str, raw_obj: Vec<u8>) -> Result<(), MetaError> {
        let bucket = self.get_partition(bucket_name)?;
        bucket
            .insert(key, raw_obj)
            .map_err(|e| MetaError::InsertError(e.to_string()))
    }

    fn get_meta(&self, bucket_name: &str, key: &str) -> Result<Option<Object>, MetaError> {
        let bucket = self.get_partition(bucket_name)?;
        let raw_object = match bucket.get(key) {
            Ok(Some(o)) => o,
            Ok(None) => return Ok(None),
            Err(e) => return Err(MetaError::OtherDBError(e.to_string())),
        };

        let obj = Object::try_from(&*raw_object).expect("Malformed object");
        Ok(Some(obj))
    }

    fn delete_object(&self, bucket: &str, key: &str) -> Result<Vec<Block>, MetaError> {
        let bucket = self.get_partition(bucket)?;

        let raw_object = match bucket.get(key) {
            Ok(Some(o)) => o,
            Ok(None) => return Ok(vec![]),
            Err(e) => return Err(MetaError::OtherDBError(e.to_string())),
        };

        let obj = Object::try_from(&*raw_object).expect("Malformed object");
        let mut to_delete: Vec<Block> = Vec::with_capacity(obj.blocks().len());

        // delete the object in the database, we have it in memory to remove the
        // blocks as needed.
        bucket
            .remove(key)
            .map_err(|e| MetaError::RemoveError(e.to_string()))?;

        for block_id in obj.blocks() {
            match self.block_partition.get(block_id) {
                Err(e) => return Err(MetaError::OtherDBError(e.to_string())),
                Ok(None) => continue,
                Ok(Some(block_data)) => {
                    let mut block = Block::try_from(&*block_data).expect("corrupt block data");
                    // We are deleting the last reference to the block, delete the
                    // whole block.
                    // Importantly, we don't remove the path yet from the path map.
                    // Leaving this path dangling in the database ensures it is not
                    // filled in by another block, before we properly delete the
                    // path from disk.
                    if block.rc() == 1 {
                        self.block_partition
                            .remove(block_id)
                            .map_err(|e| MetaError::RemoveError(e.to_string()))?;
                        to_delete.push(block);
                    } else {
                        block.decrement_refcount();
                        self.block_partition
                            .insert(block_id, block.to_vec())
                            .map_err(|e| MetaError::InsertError(e.to_string()))?;
                    }
                }
            }
        }
        Ok(to_delete)
    }

    fn begin_transaction(&self) -> Box<dyn Transaction> {
        Box::new(FjallNoTransaction::new(Arc::new(self.clone())))
    }
}

// FjallNoTransaction is fjall without real transaction support.
// the transaction is not really reliable because we support it by ourself,
// not provided by the underlying database.
pub struct FjallNoTransaction {
    store: Arc<FjallStoreNotx>,
    inserted_blocks: Vec<BlockID>,
    inserted_paths: Vec<Vec<u8>>,
}

impl FjallNoTransaction {
    pub fn new(store: Arc<FjallStoreNotx>) -> Self {
        Self {
            store,
            inserted_blocks: Vec::new(),
            inserted_paths: Vec::new(),
        }
    }
}

unsafe impl Send for FjallNoTransaction {}
unsafe impl Sync for FjallNoTransaction {}

impl Transaction for FjallNoTransaction {
    fn commit(self: Box<Self>) -> Result<(), MetaError> {
        Ok(())
    }

    fn rollback(self: Box<Self>) {
        for block_id in self.inserted_blocks {
            let _ = self.store.block_partition.remove(block_id);
        }

        for path in self.inserted_paths {
            let _ = self.store.path_partition.remove(&path);
        }
    }

    fn write_block(
        &mut self,
        block_hash: BlockID,
        data_len: usize,
        key_has_block: bool,
    ) -> Result<(bool, Block), MetaError> {
        let blocks = self.store.block_partition.clone();
        let paths = self.store.path_partition.clone();

        match blocks.get(block_hash) {
            Ok(Some(block_data)) => {
                let mut block =
                    Block::try_from(&*block_data).expect("Only valid blocks are stored");

                if !key_has_block {
                    block.increment_refcount();
                    blocks
                        .insert(block_hash, block.to_vec())
                        .map_err(|e| MetaError::InsertError(e.to_string()))?;
                    self.inserted_blocks.push(block_hash);
                }
                Ok((false, block))
            }
            Ok(None) => {
                let mut idx = 0;
                for index in 1..BLOCKID_SIZE {
                    match paths.get(&block_hash[..index]) {
                        Ok(Some(_)) => continue,
                        Ok(None) => {
                            idx = index;
                            break;
                        }
                        Err(e) => return Err(MetaError::OtherDBError(e.to_string())),
                    }
                }

                paths
                    .insert(&block_hash[..idx], block_hash)
                    .map_err(|e| MetaError::InsertError(e.to_string()))?;
                self.inserted_paths.push(block_hash[..idx].to_vec());

                let block = Block::new(data_len, block_hash[..idx].to_vec());
                blocks
                    .insert(block_hash, block.to_vec())
                    .map_err(|e| MetaError::InsertError(e.to_string()))?;
                self.inserted_blocks.push(block_hash);
                Ok((true, block))
            }
            Err(e) => Err(MetaError::OtherDBError(e.to_string())),
        }
    }
}

pub struct FjallTreeNotx {
    partition: Arc<fjall::PartitionHandle>,
}

impl FjallTreeNotx {
    pub fn new(partition: Arc<fjall::PartitionHandle>) -> Self {
        Self { partition }
    }

    fn get(&self, key: &[u8]) -> Result<Option<fjall::Slice>, MetaError> {
        match self.partition.get(key) {
            Ok(Some(v)) => Ok(Some(v)),
            Ok(None) => Ok(None),
            Err(e) => Err(MetaError::OtherDBError(e.to_string())),
        }
    }
}

impl BaseMetaTree for FjallTreeNotx {
    fn insert(&self, key: &[u8], value: Vec<u8>) -> Result<(), MetaError> {
        match self.partition.insert(key, value) {
            Ok(_) => Ok(()),
            Err(e) => Err(MetaError::OtherDBError(e.to_string())),
        }
    }

    fn remove(&self, key: &[u8]) -> Result<(), MetaError> {
        match self.partition.remove(key) {
            Ok(_) => Ok(()),
            Err(e) => Err(MetaError::OtherDBError(e.to_string())),
        }
    }

    fn contains_key(&self, key: &[u8]) -> Result<bool, MetaError> {
        match self.partition.contains_key(key) {
            Ok(v) => Ok(v),
            Err(_) => Err(MetaError::KeyNotFound),
        }
    }

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, MetaError> {
        match self.get(key) {
            Ok(Some(v)) => Ok(Some(v.to_vec())),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl BucketTreeExt for FjallTreeNotx {
    fn get_bucket_keys(&self) -> Box<dyn Iterator<Item = Result<Vec<u8>, MetaError>> + Send> {
        let partition = self.partition.clone();
        let mut last_key: Option<Vec<u8>> = None;

        Box::new(std::iter::from_fn(move || {
            let range = match &last_key {
                Some(k) => {
                    let mut next = k.clone();
                    next.push(0);
                    next..
                }
                None => Vec::new()..,
            };

            partition
                .range::<Vec<u8>, _>(range)
                .next()
                .map(|res| match res {
                    Ok((k, _)) => {
                        last_key = Some(k.to_vec());
                        Ok(k.to_vec())
                    }
                    Err(e) => {
                        tracing::error!("Error reading key: {}", e);
                        Err(MetaError::OtherDBError(e.to_string()))
                    }
                })
        }))
    }

    // rules:
    // 1. continuation_token and start_after exists: use the one with the highest lexicographical order
    //    -> call it: ctsa
    // 2. if prefix exists
    //    -> ctsa > the prefix && doesn't have prefix: return zero results
    //    -> ctsa < prefix: ignore it
    //    -> ctsa has the prefix: use it as start_after
    //          In kv store like fjall & Sled: we process it in the Rust code
    fn range_filter<'a>(
        &'a self,
        start_after: Option<String>,
        prefix: Option<String>,
        continuation_token: Option<String>,
    ) -> Box<(dyn Iterator<Item = (String, Object)> + 'a)> {
        let mut ctsa = match (continuation_token, start_after) {
            (Some(token), Some(start)) => Some(std::cmp::max(token, start)),
            (Some(token), None) => Some(token),
            (None, start) => start,
        };

        let partition = self.partition.clone();

        let base_iter: Box<
            dyn Iterator<Item = Result<(fjall::Slice, fjall::Slice), fjall::Error>>,
        > = match (prefix.as_ref(), ctsa.as_ref()) {
            (Some(prefix), Some(ctsa)) if (ctsa > prefix && !ctsa.starts_with(prefix)) => {
                //Return empty iterator if ctsa is after prefix
                Box::new(std::iter::empty())
            }
            (Some(prefix), Some(ctsa_local)) if ctsa_local < prefix => {
                // If ctsa is before prefix, ignore ctsa
                ctsa = None;
                Box::new(partition.prefix(prefix.as_bytes()))
            }
            (Some(prefix), _) => Box::new(partition.prefix(prefix.as_bytes())),
            (None, Some(ctsa)) => {
                let mut next_key = ctsa.as_bytes().to_vec();
                next_key.push(0);
                Box::new(partition.range(next_key..))
            }
            (None, None) => Box::new(partition.range::<Vec<u8>, _>(..)),
        };

        let filtered = base_iter.filter_map(|res| res.ok());

        let skip_filtered = if prefix.is_some() && ctsa.is_some() {
            let ctsa_bytes = ctsa.unwrap().into_bytes();
            Box::new(
                filtered.skip_while(move |(raw_key, _)| raw_key.deref() <= ctsa_bytes.as_slice()),
            ) as Box<dyn Iterator<Item = _>>
        } else {
            Box::new(filtered)
        };

        Box::new(skip_filtered.map(|(raw_key, raw_value)| {
            let key = unsafe { String::from_utf8_unchecked(raw_key.to_vec()) };
            let obj = Object::try_from(&*raw_value).unwrap();
            (key, obj)
        }))
    }
}

impl BlockTree for FjallTreeNotx {
    fn get_block(&self, key: &[u8]) -> Result<Option<Block>, MetaError> {
        let block_data = match self.get(key) {
            Ok(Some(b)) => b,
            Ok(None) => return Ok(None),
            Err(e) => return Err(MetaError::OtherDBError(e.to_string())),
        };

        let block = match Block::try_from(&*block_data) {
            Ok(b) => b,
            Err(e) => return Err(MetaError::OtherDBError(e.to_string())),
        };
        Ok(Some(block))
    }

    #[cfg(test)]
    fn len(&self) -> Result<usize, MetaError> {
        let len = self
            .partition
            .len()
            .map_err(|e| MetaError::OtherDBError(e.to_string()))?;
        Ok(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metastore::stores::test_utils;
    use tempfile::tempdir;

    impl test_utils::TestStore for FjallStoreNotx {
        fn insert_bucket(&self, bucket_name: &str, raw_bucket: Vec<u8>) -> Result<(), MetaError> {
            <FjallStoreNotx as MetaStore>::insert_bucket(self, bucket_name, raw_bucket)
        }

        fn bucket_exists(&self, bucket_name: &str) -> Result<bool, MetaError> {
            <FjallStoreNotx as MetaStore>::bucket_exists(self, bucket_name)
        }

        fn list_buckets(&self) -> Result<Vec<BucketMeta>, MetaError> {
            <FjallStoreNotx as MetaStore>::list_buckets(self)
        }

        fn insert_meta(
            &self,
            bucket_name: &str,
            key: &str,
            raw_obj: Vec<u8>,
        ) -> Result<(), MetaError> {
            <FjallStoreNotx as MetaStore>::insert_meta(self, bucket_name, key, raw_obj)
        }

        fn get_meta(&self, bucket_name: &str, key: &str) -> Result<Option<Object>, MetaError> {
            <FjallStoreNotx as MetaStore>::get_meta(self, bucket_name, key)
        }

        fn get_bucket_ext(
            &self,
            name: &str,
        ) -> Result<Box<dyn BucketTreeExt + Send + Sync>, MetaError> {
            <FjallStoreNotx as MetaStore>::get_bucket_ext(self, name)
        }
    }

    fn setup_store() -> (FjallStoreNotx, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let store = FjallStoreNotx::new(dir.path().to_path_buf(), Some(1));
        (store, dir)
    }

    #[test]
    fn test_errors() {
        let (store, _dir) = setup_store();
        test_utils::test_errors(&store);
    }

    #[test]
    fn test_bucket_operations() {
        let (store, _dir) = setup_store();
        test_utils::test_bucket_operations(&store);
    }

    #[test]
    fn test_object_operations() {
        let (store, _dir) = setup_store();
        test_utils::test_object_operations(&store);
    }

    #[test]
    fn test_get_bucket_keys() {
        let (store, _dir) = setup_store();
        test_utils::test_get_bucket_keys(&store);
    }

    #[test]
    fn test_range_filter() {
        let (store, _dir) = setup_store();
        test_utils::test_range_filter(&store);
    }
}
