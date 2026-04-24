use std::io;
use std::sync::Arc;

use faster_hex::hex_string;
use futures::{
    channel::mpsc::unbounded,
    sink::SinkExt,
    stream,
    stream::{StreamExt, TryStreamExt},
};
use md5::{Digest, Md5};
use rusoto_core::ByteStream;

use super::buffered_byte_stream::BufferedByteStream;
use super::fs::CasFS;
use crate::metastore::{BlockID, MetaError, Object, ObjectData};
use crate::metrics::SharedMetrics;

pub(super) struct PendingMarker {
    metrics: SharedMetrics,
    in_flight: u64,
}

impl PendingMarker {
    pub fn new(metrics: SharedMetrics) -> Self {
        Self {
            metrics,
            in_flight: 0,
        }
    }

    pub fn block_pending(&mut self) {
        self.metrics.block_pending();
        self.in_flight += 1;
    }

    pub fn block_write_error(&mut self) {
        self.metrics.block_write_error();
        self.in_flight -= 1;
    }

    pub fn block_ignored(&mut self) {
        self.metrics.block_ignored();
    }

    pub fn block_written(&mut self, _size: usize) {
        self.metrics.block_written();
        self.in_flight -= 1;
    }
}

impl Drop for PendingMarker {
    fn drop(&mut self) {
        self.metrics.blocks_dropped(self.in_flight)
    }
}

#[tracing::instrument(skip(fs, data), fields(bucket = %bucket_name, key = %key, size, blocks))]
pub(super) async fn store_object(
    fs: &CasFS,
    bucket_name: &str,
    key: &str,
    data: ByteStream,
) -> io::Result<(Vec<BlockID>, BlockID, u64)> {
    let old_obj_meta = match fs.get_object_meta(bucket_name, key) {
        Ok(Some(obj_meta)) => Some(obj_meta),
        _ => None,
    };
    let old_obj_meta = Arc::new(old_obj_meta);

    let (tx, rx) = unbounded();
    let mut content_hash = Md5::new();
    let data = BufferedByteStream::new(data);
    let mut size = 0;
    data.map(|res| match res {
        Ok(buffers) => buffers.into_iter().map(Ok).collect(),
        Err(e) => vec![Err(e)],
    })
    .map(stream::iter)
    .flatten()
    .inspect(|maybe_bytes| {
        if let Ok(bytes) = maybe_bytes {
            content_hash.update(bytes);
            size += bytes.len() as u64;
            fs.metrics.bytes_received(bytes.len());
        }
    })
    .zip(stream::repeat((tx, old_obj_meta)))
    .enumerate()
    .for_each(
        |(idx, (maybe_chunk, (mut tx, old_obj_meta)))| async move {
            if let Err(e) = maybe_chunk {
                if let Err(e) = tx
                    .send(Err(std::io::Error::new(e.kind(), e.to_string())))
                    .await
                {
                    tracing::error!(error = %e, "Could not convey result");
                }
                return;
            }
            // unwrap is safe as we checked that there is no error above
            let bytes: Vec<u8> = maybe_chunk.unwrap();
            let mut hasher = Md5::new();
            hasher.update(&bytes);
            let block_hash: BlockID = hasher.finalize().into();
            let data_len = bytes.len();

            // check if this key already has this block
            let key_has_block = if let Some(obj) = old_obj_meta.as_ref() {
                obj.has_block(&block_hash)
            } else {
                false
            };

            // begin the transaction
            // there are two main things we need to do here:
            // 1. write the meta to the database
            //      - if the block already exists, we don't need to write it to the storage
            //      - if the block does not exist, we need to write it to the storage
            // 2. write the actual block to disk
            //
            // we commit the meta database transaction BEFORE writing the block to disk
            // to avoid holding the lock during slow I/O operations.
            //
            // IMPORTANT: In multi-user mode, use shared MetaStore for block transactions
            // to ensure blocks are written to the shared _BLOCKS tree, not user-specific tree
            let mut store_tx = fs.shared.meta_store().begin_transaction();
            let write_meta_result = store_tx.write_block(block_hash, data_len, key_has_block);

            let mut pm = PendingMarker::new(fs.metrics.clone());

            let block = match write_meta_result {
                Err(e) => {
                    if let Err(e) = tx.unbounded_send(Err(e.into())) {
                        tracing::error!(error = %e, "Could not send transaction error");
                    }
                    return;
                }
                Ok((false, _)) => {
                    // the block already exists, no need to write it to the storage
                    pm.block_ignored();

                    tracing::debug!(target: "cas_storage::locks", "Committing metadata transaction (block exists)");
                    Box::new(store_tx).commit().unwrap();

                    if let Err(e) = tx.unbounded_send(Ok((idx, block_hash))) {
                        tracing::error!(error = %e, "Could not send block id");
                    }
                    return;
                }
                Ok((true, block)) => {
                    // the block does not exist, we need to write it to the storage
                    pm.block_pending();

                    // COMMIT IMMEDIATELY to release lock
                    tracing::debug!(target: "cas_storage::locks", "Committing metadata transaction (new block)");
                    Box::new(store_tx).commit().unwrap();

                    block
                }
            };

            // write the actual block to disk
            // if the disk operation fails, we must manually rollback (compensating transaction)
            let block_path = block.disk_path(fs.fs_root().clone());

            // Helper to cleanup on failure
            let cleanup_on_failure = || {
                // We need to delete the block we just added.
                // Since we just added it with rc=1, we can just delete it.
                // We accept potential data leakage here if this cleanup fails,
                // as per the design principles (leakage is better than data loss).
                let tree = fs.shared.block_tree();
                if let Err(e) = tree.remove(&block_hash) {
                    tracing::warn!(block = %hex_string(&block_hash), error = %e, "Failed to cleanup orphan block metadata");
                } else {
                    tracing::debug!(block = %hex_string(&block_hash), "Cleaned up orphan block metadata");
                }
            };

            if let Err(e) = fs.async_fs.create_dir_all(block_path.parent().unwrap()) {
                cleanup_on_failure();

                if let Err(e) = tx.unbounded_send(Err(e)) {
                    pm.block_write_error();
                    tracing::error!(error = %e, "Could not send path create error");
                    return;
                }
            }
            if let Err(e) = fs.async_fs.write(&block_path, &bytes) {
                cleanup_on_failure();

                if let Err(e) = tx.unbounded_send(Err(e)) {
                    pm.block_write_error();
                    tracing::error!(error = %e, "Could not send block write error");
                    return;
                }
            }

            pm.block_written(bytes.len());

            if let Err(e) = tx.unbounded_send(Ok((idx, block_hash))) {
                tracing::error!(error = %e, "Could not send block id");
            }
        },
    )
    .await;

    let mut ids = rx.try_collect::<Vec<(usize, BlockID)>>().await?;
    // Make sure the chunks are in the proper order
    ids.sort_by_key(|a| a.0);

    let blocks: Vec<BlockID> = ids.into_iter().map(|(_, id)| id).collect();

    tracing::Span::current().record("size", size);
    tracing::Span::current().record("blocks", blocks.len());

    Ok((blocks, content_hash.finalize().into(), size))
}

pub(super) async fn store_single_object_and_meta(
    fs: &CasFS,
    bucket_name: &str,
    key: &str,
    data: ByteStream,
    len: usize,
) -> io::Result<Object> {
    let (blocks, content_hash, size) = if len > 0 {
        store_object(fs, bucket_name, key, data).await?
    } else {
        tracing::warn!(%key, "Skipping store for empty blob");
        (Vec::new(), [0; 16], 0)
    };
    let obj = fs
        .create_object_meta(
            bucket_name,
            key,
            size,
            content_hash,
            ObjectData::SinglePart { blocks },
        )
        .unwrap();
    Ok(obj)
}

pub(super) fn store_inlined_object(
    fs: &CasFS,
    bucket_name: &str,
    key: &str,
    data: Vec<u8>,
) -> Result<Object, MetaError> {
    let content_hash = Md5::digest(&data).into();
    let size = data.len() as u64;
    fs.create_object_meta(
        bucket_name,
        key,
        size,
        content_hash,
        ObjectData::Inline { data },
    )
}
