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

/// Inputs for building a presigned URL -- CLI-independent so unit
/// tests can drive it directly without going through clap or the
/// filesystem.
pub struct PresignInputs<'a> {
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub endpoint: &'a str,
    pub region: &'a str,
    pub bucket: &'a str,
    pub key: &'a str,
    pub method: &'a str,
    pub ttl: Duration,
    pub now: SystemTime,
}

pub fn build_presigned_url(inputs: &PresignInputs<'_>) -> Result<String> {
    if inputs.ttl > SIGV4_MAX_TTL {
        bail!("TTL exceeds SigV4 maximum of 7 days");
    }
    if inputs.ttl.is_zero() {
        bail!("TTL must be positive");
    }

    let mut object_url = Url::parse(inputs.endpoint).context("parsing endpoint")?;
    {
        let base = object_url.path().trim_end_matches('/').to_string();
        let path = format!("{}/{}/{}", base, inputs.bucket, inputs.key);
        object_url.set_path(&path);
    }

    let identity = Credentials::new(
        inputs.access_key,
        inputs.secret_key,
        None,
        None,
        "s3-cas",
    )
    .into();

    let mut settings = SigningSettings::default();
    settings.signature_location = SignatureLocation::QueryParams;
    settings.expires_in = Some(inputs.ttl);

    let params = SigningParams::builder()
        .identity(&identity)
        .region(inputs.region)
        .name("s3")
        .time(inputs.now)
        .settings(settings)
        .build()
        .context("building SigV4 parameters")?
        .into();

    let method_upper = inputs.method.to_uppercase();
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

    Ok(object_url.into())
}

pub fn presign(config: PresignConfig) -> Result<()> {
    let store = open_user_store(config.meta_root.clone(), config.metadata_db)?;
    let user = store
        .get_user_by_id(&config.user)?
        .ok_or_else(|| anyhow!("user '{}' not found", config.user))?;

    let url = build_presigned_url(&PresignInputs {
        access_key: &user.s3_access_key,
        secret_key: &user.s3_secret_key,
        endpoint: &config.endpoint,
        region: &config.region,
        bucket: &config.bucket,
        key: &config.key,
        method: &config.method,
        ttl: config.ttl.into(),
        now: SystemTime::now(),
    })?;

    println!("{url}");
    Ok(())
}

fn open_user_store(meta_root: PathBuf, engine: StorageEngine) -> Result<Arc<UserStore>> {
    let shared = SharedBlockStore::new(meta_root.join("blocks"), engine, None, None)?;
    let store = shared.meta_store().get_underlying_store();
    Ok(Arc::new(UserStore::new(store)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn inputs() -> PresignInputs<'static> {
        PresignInputs {
            access_key: "AKIAIOSFODNN7EXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            endpoint: "http://localhost:8014",
            region: "us-east-1",
            bucket: "mybucket",
            key: "path/to/file.txt",
            method: "GET",
            ttl: Duration::from_secs(900),
            // Fixed unix timestamp -> 2026-05-02 00:00:00 UTC. Deterministic.
            now: UNIX_EPOCH + Duration::from_secs(1_777_680_000),
        }
    }

    #[test]
    fn builds_expected_query_params() {
        let url = build_presigned_url(&inputs()).expect("signing should succeed");
        let must_contain = [
            "X-Amz-Algorithm=AWS4-HMAC-SHA256",
            "X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20260502%2Fus-east-1%2Fs3%2Faws4_request",
            "X-Amz-Date=20260502T000000Z",
            "X-Amz-Expires=900",
            "X-Amz-SignedHeaders=host",
            "X-Amz-Signature=",
        ];
        for needle in must_contain {
            assert!(
                url.contains(needle),
                "url did not contain `{}`: {}",
                needle,
                url
            );
        }
        assert!(
            url.starts_with("http://localhost:8014/mybucket/path/to/file.txt?"),
            "unexpected base URL shape: {}",
            url
        );
    }

    #[test]
    fn signature_is_deterministic_for_fixed_inputs() {
        let a = build_presigned_url(&inputs()).unwrap();
        let b = build_presigned_url(&inputs()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn different_ttl_produces_different_signature() {
        let mut i = inputs();
        let a = build_presigned_url(&i).unwrap();
        i.ttl = Duration::from_secs(3600);
        let b = build_presigned_url(&i).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn rejects_zero_ttl() {
        let mut i = inputs();
        i.ttl = Duration::ZERO;
        assert!(build_presigned_url(&i).is_err());
    }

    #[test]
    fn rejects_ttl_over_seven_days() {
        let mut i = inputs();
        i.ttl = Duration::from_secs(8 * 24 * 3600);
        assert!(build_presigned_url(&i).is_err());
    }

    #[test]
    fn method_affects_signature() {
        let mut i = inputs();
        let get_url = build_presigned_url(&i).unwrap();
        i.method = "PUT";
        let put_url = build_presigned_url(&i).unwrap();
        assert_ne!(
            extract_query(&get_url, "X-Amz-Signature"),
            extract_query(&put_url, "X-Amz-Signature"),
        );
    }

    fn extract_query(url: &str, name: &str) -> String {
        let parsed = Url::parse(url).unwrap();
        parsed
            .query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    }
}
