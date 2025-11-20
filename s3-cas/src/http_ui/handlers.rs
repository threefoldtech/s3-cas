use std::collections::HashSet;

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, StatusCode};
use serde::Serialize;

use cas_storage::CasFS;
use cas_storage::BucketMeta;

use super::{responses, templates};

#[derive(Serialize)]
pub struct BucketInfo {
    pub name: String,
    pub creation_date: String,
}

impl From<&BucketMeta> for BucketInfo {
    fn from(meta: &BucketMeta) -> Self {
        Self {
            name: meta.name().to_string(),
            creation_date: format_timestamp(meta.ctime()),
        }
    }
}

#[derive(Serialize, Hash, Eq, PartialEq, Clone)]
pub struct DirectoryInfo {
    pub name: String,
    pub prefix: String,
}

#[derive(Serialize)]
pub struct ObjectInfo {
    pub key: String,
    pub size: u64,
    pub hash: String,
    pub last_modified: String,
    pub is_inlined: bool,
    pub block_count: usize,
}

#[derive(Serialize)]
pub struct ObjectListResponse {
    pub bucket: String,
    pub prefix: String,
    pub directories: Vec<DirectoryInfo>,
    pub objects: Vec<ObjectInfo>,
    pub total_count: usize,
    pub has_more: bool,
    pub next_token: Option<String>,
}

#[derive(Serialize)]
pub struct ObjectMetadata {
    pub key: String,
    pub bucket: String,
    pub size: u64,
    pub hash: String,
    pub last_modified: String,
    pub is_inlined: bool,
    pub blocks: Vec<BlockInfo>,
}

#[derive(Serialize)]
pub struct BlockInfo {
    pub hash: String,
    pub size: usize,
    pub refcount: usize,
}

pub async fn list_buckets(
    casfs: &CasFS,
    wants_html: bool,
    is_admin: Option<bool>,
) -> Response<Full<Bytes>> {
    match casfs.list_buckets() {
        Ok(buckets) => {
            let bucket_infos: Vec<BucketInfo> = buckets.iter().map(BucketInfo::from).collect();
            if wants_html {
                let page = match is_admin {
                    Some(admin) => templates::buckets_page_with_user(&bucket_infos, admin),
                    None => templates::buckets_page(&bucket_infos),
                };
                responses::html_response(StatusCode::OK, page)
            } else {
                responses::json_response(StatusCode::OK, &bucket_infos)
            }
        }
        Err(e) => responses::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Error listing buckets: {e}"),
            wants_html,
        ),
    }
}

pub async fn list_objects(
    casfs: &CasFS,
    bucket: &str,
    req: &Request<hyper::body::Incoming>,
    wants_html: bool,
) -> Response<Full<Bytes>> {
    // Check if bucket exists
    match casfs.bucket_exists(bucket) {
        Ok(false) => {
            return responses::error_response(StatusCode::NOT_FOUND, "Bucket not found", wants_html)
        }
        Err(e) => {
            return responses::error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Error checking bucket: {e}"),
                wants_html,
            )
        }
        Ok(true) => {}
    }

    // Parse query parameters
    let query_params = req.uri().query().unwrap_or("");

    let prefix = query_params
        .split('&')
        .find(|p| p.starts_with("prefix="))
        .and_then(|p| p.strip_prefix("prefix="))
        .map(|p| urlencoding::decode(p).unwrap_or_default().to_string())
        .unwrap_or_default();

    let limit: usize = query_params
        .split('&')
        .find(|p| p.starts_with("limit="))
        .and_then(|p| p.strip_prefix("limit="))
        .and_then(|p| p.parse().ok())
        .unwrap_or(100); // Default limit of 100 items

    let start_after = query_params
        .split('&')
        .find(|p| p.starts_with("token="))
        .and_then(|p| p.strip_prefix("token="))
        .map(|p| urlencoding::decode(p).unwrap_or_default().to_string());

    // Get bucket tree and list objects
    match casfs.get_bucket(bucket) {
        Ok(tree) => {
            let mut directories = HashSet::new();
            let mut objects = Vec::new();
            let mut last_key: Option<String> = None;
            let mut item_count = 0;
            let mut has_more = false;

            // Use range_filter to get objects with the given prefix
            for (key, obj) in tree.range_filter(start_after.clone(), Some(prefix.clone()), None) {
                // Check if we've hit the limit
                if item_count >= limit {
                    has_more = true;
                    break;
                }

                // Check if this key has subdirectories after the prefix
                let relative_key = if prefix.is_empty() {
                    key.as_str()
                } else {
                    key.strip_prefix(&prefix).unwrap_or(&key)
                };

                if let Some(slash_pos) = relative_key.find('/') {
                    // This is a subdirectory
                    let dir_name = &relative_key[..slash_pos + 1];
                    let full_prefix = format!("{}{}", prefix, dir_name);
                    let dir_info = DirectoryInfo {
                        name: dir_name.to_string(),
                        prefix: full_prefix,
                    };

                    // Only count unique directories toward the limit
                    if directories.insert(dir_info) {
                        item_count += 1;
                        last_key = Some(key.clone());
                    }
                } else {
                    // This is a file at the current level
                    objects.push(ObjectInfo {
                        key: key.clone(),
                        size: obj.size(),
                        hash: faster_hex::hex_string(obj.hash()),
                        last_modified: format_timestamp(obj.last_modified()),
                        is_inlined: obj.is_inlined(),
                        block_count: obj.blocks().len(),
                    });
                    item_count += 1;
                    last_key = Some(key.clone());
                }
            }

            let mut directories: Vec<DirectoryInfo> = directories.into_iter().collect();
            directories.sort_by(|a, b| a.name.cmp(&b.name));

            objects.sort_by(|a, b| a.key.cmp(&b.key));

            let total_count = directories.len() + objects.len();

            let next_token = if has_more { last_key } else { None };

            let response = ObjectListResponse {
                bucket: bucket.to_string(),
                prefix,
                directories,
                objects,
                total_count,
                has_more,
                next_token,
            };

            if wants_html {
                responses::html_response(StatusCode::OK, templates::objects_page(&response))
            } else {
                responses::json_response(StatusCode::OK, &response)
            }
        }
        Err(e) => responses::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Error listing objects: {e}"),
            wants_html,
        ),
    }
}

pub async fn object_metadata(
    casfs: &CasFS,
    bucket: &str,
    key: &str,
    wants_html: bool,
) -> Response<Full<Bytes>> {
    match casfs.get_object_meta(bucket, key) {
        Ok(Some(obj)) => {
            // Get block details
            let block_tree = match casfs.block_tree() {
                Ok(tree) => tree,
                Err(e) => {
                    return responses::error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("Error accessing block tree: {e}"),
                        wants_html,
                    )
                }
            };

            let blocks: Vec<BlockInfo> = obj
                .blocks()
                .iter()
                .filter_map(|block_id| {
                    block_tree
                        .get_block(block_id)
                        .ok()
                        .flatten()
                        .map(|block| BlockInfo {
                            hash: faster_hex::hex_string(block_id),
                            size: block.size(),
                            refcount: block.rc(),
                        })
                })
                .collect();

            let metadata = ObjectMetadata {
                key: key.to_string(),
                bucket: bucket.to_string(),
                size: obj.size(),
                hash: faster_hex::hex_string(obj.hash()),
                last_modified: format_timestamp(obj.last_modified()),
                is_inlined: obj.is_inlined(),
                blocks,
            };

            if wants_html {
                responses::html_response(StatusCode::OK, templates::object_detail_page(&metadata))
            } else {
                responses::json_response(StatusCode::OK, &metadata)
            }
        }
        Ok(None) => responses::error_response(StatusCode::NOT_FOUND, "Object not found", wants_html),
        Err(e) => responses::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Error getting object: {e}"),
            wants_html,
        ),
    }
}

fn format_timestamp(time: std::time::SystemTime) -> String {
    use std::time::SystemTime;
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let datetime = chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0)
        .unwrap_or_default();
    datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}
