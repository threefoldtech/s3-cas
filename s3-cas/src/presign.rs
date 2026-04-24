use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    sign, SignableBody, SignableRequest, SignatureLocation, SigningSettings,
};
use aws_sigv4::sign::v4::SigningParams;
use cas_storage::{SharedBlockStore, StorageEngine};
use clap::Parser;
use url::Url;

use crate::auth::UserStore;

const SIGV4_MAX_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Parser, Debug)]
pub struct PresignConfig {
    #[arg(long, default_value = ".")]
    pub meta_root: PathBuf,

    #[arg(
        long,
        default_value = "fjall",
        help = "Metadata DB (fjall, fjall_notx)"
    )]
    pub metadata_db: StorageEngine,

    /// User id whose S3 credentials sign the URL (must exist in _USERS)
    #[arg(long)]
    pub user: String,

    /// Base endpoint URL, e.g. http://localhost:8014
    #[arg(long)]
    pub endpoint: String,

    /// AWS region name
    #[arg(long, default_value = "us-east-1")]
    pub region: String,

    /// Time-to-live. Max 7 days per SigV4. Accepts "15m", "2h", "1d".
    #[arg(long, default_value = "1h")]
    pub ttl: humantime::Duration,

    /// HTTP method (GET, PUT, HEAD, DELETE)
    #[arg(long, default_value = "GET")]
    pub method: String,

    /// Bucket name
    pub bucket: String,

    /// Object key
    pub key: String,
}

pub fn presign(config: PresignConfig) -> Result<()> {
    let ttl: Duration = config.ttl.into();
    if ttl > SIGV4_MAX_TTL {
        bail!("TTL exceeds SigV4 maximum of 7 days");
    }
    if ttl.is_zero() {
        bail!("TTL must be positive");
    }

    let store = open_user_store(config.meta_root.clone(), config.metadata_db)?;
    let user = store
        .get_user_by_id(&config.user)?
        .ok_or_else(|| anyhow!("user '{}' not found", config.user))?;

    let mut object_url = Url::parse(&config.endpoint).context("parsing --endpoint")?;
    {
        let base = object_url.path().trim_end_matches('/').to_string();
        let path = format!("{}/{}/{}", base, config.bucket, config.key);
        object_url.set_path(&path);
    }

    let identity = Credentials::new(
        user.s3_access_key,
        user.s3_secret_key,
        None,
        None,
        "s3-cas",
    )
    .into();

    let mut settings = SigningSettings::default();
    settings.signature_location = SignatureLocation::QueryParams;
    settings.expires_in = Some(ttl);

    let params = SigningParams::builder()
        .identity(&identity)
        .region(&config.region)
        .name("s3")
        .time(SystemTime::now())
        .settings(settings)
        .build()
        .context("building SigV4 parameters")?
        .into();

    let method_upper = config.method.to_uppercase();
    let request = SignableRequest::new(
        &method_upper,
        object_url.as_str(),
        std::iter::empty(),
        SignableBody::UnsignedPayload,
    )
    .context("building signable request")?;

    let (instructions, _sig) = sign(request, &params)
        .context("signing request")?
        .into_parts();

    let (_headers, params_to_add) = instructions.into_parts();
    for (name, value) in params_to_add {
        object_url.query_pairs_mut().append_pair(name, &value);
    }

    println!("{}", object_url);
    Ok(())
}

fn open_user_store(meta_root: PathBuf, engine: StorageEngine) -> Result<Arc<UserStore>> {
    let shared = SharedBlockStore::new(meta_root.join("blocks"), engine, None, None)?;
    let store = shared.meta_store().get_underlying_store();
    Ok(Arc::new(UserStore::new(store)))
}
