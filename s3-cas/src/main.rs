use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use bytes::Bytes;
use clap::{Parser, Subcommand};
use http_body_util::Full;
use prometheus::Encoder;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use cas_storage::{Durability, SharedBlockStore, StorageEngine};
use s3_cas::auth::{UserRecord, UserRouter, UserStore};
use s3_cas::check::{check_integrity, CheckConfig};
use s3_cas::presign::{presign, PresignConfig};
use s3_cas::retrieve::{retrieve, RetrieveConfig};
use s3_cas::s3_wrapper::{DynamicS3Auth, S3UserRouter};

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
pub struct ServerConfig {
    #[arg(long, default_value = ".")]
    fs_root: PathBuf,

    #[arg(long, default_value = ".")]
    meta_root: PathBuf,

    #[arg(long, default_value = "localhost")]
    host: String,

    #[arg(long, default_value = "8014")]
    port: u16,

    #[arg(long, default_value = "localhost")]
    metric_host: String,

    #[arg(long, default_value = "9100")]
    metric_port: u16,

    #[arg(long, help = "leave empty to disable it")]
    inline_metadata_size: Option<usize>,

    #[arg(
        long,
        default_value = "fjall",
        help = "Metadata DB  (fjall, fjall_notx)"
    )]
    metadata_db: StorageEngine,

    #[arg(
        long,
        default_value = "fdatasync",
        help = "Durability level (buffer, fsync, fdatasync)"
    )]
    durability: Durability,

    #[arg(
        long,
        default_value = "info",
        help = "Log level (error, warn, info, debug, trace). Can also be set via RUST_LOG env var"
    )]
    log_level: String,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect DB
    Inspect {
        #[arg(long, default_value = ".")]
        meta_root: PathBuf,

        #[arg(
            long,
            default_value = "fjall",
            help = "Metadata DB  (fjall, fjall_notx)"
        )]
        metadata_db: StorageEngine,

        #[command(subcommand)]
        command: InspectCommand,
    },

    /// retrieve an object
    Retrieve(RetrieveConfig),

    /// Check object integrity
    Check(CheckConfig),

    /// Manage users (add, list, delete, reset-password)
    User {
        #[arg(long, default_value = ".")]
        meta_root: PathBuf,

        #[arg(
            long,
            default_value = "fjall",
            help = "Metadata DB  (fjall, fjall_notx)"
        )]
        metadata_db: StorageEngine,

        #[command(subcommand)]
        command: UserCommand,
    },

    /// Generate a SigV4 presigned URL for an object
    Presign(PresignConfig),

    /// Start S3-cas server
    Server(ServerConfig),
}

#[derive(Debug, Subcommand)]
pub enum InspectCommand {
    /// Number of keys (objects) in database
    NumKeys,
    /// Total disk space used by database
    DiskSpace,
    /// List all users
    ListUsers,
    /// Show per-user storage statistics
    UserStats {
        /// Specific user ID to show stats for (optional)
        user_id: Option<String>,
    },
    /// List all buckets
    ListBuckets {
        /// Filter by user ID
        #[arg(long)]
        user: Option<String>,
    },
    /// Show statistics for a specific bucket
    BucketStats {
        /// Bucket name
        bucket: String,
        /// User ID (required)
        #[arg(long)]
        user: String,
    },
    /// Show block storage statistics and deduplication ratio
    BlockStats,
    /// Show detailed information about a specific object
    ObjectInfo {
        /// Bucket name
        bucket: String,
        /// Object key
        key: String,
        /// User ID (required)
        #[arg(long)]
        user: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Add a new user. Auto-generates S3 credentials if not provided.
    Add {
        /// User id
        user_id: String,
        /// S3 access key (auto-generated if not provided)
        #[arg(long)]
        access_key: Option<String>,
        /// S3 secret key (auto-generated if not provided)
        #[arg(long)]
        secret_key: Option<String>,
        /// Mark the user as admin
        #[arg(long)]
        admin: bool,
    },
    /// List all users
    List,
    /// Delete a user (removes indices; does not touch object data on disk)
    Delete { user_id: String },
}

fn setup_tracing(log_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(log_level))
        .unwrap_or_else(|_| {
            eprintln!("Invalid log level '{}', falling back to 'info'", log_level);
            EnvFilter::new("info")
        });

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    let cli = Cli::parse();

    let log_level = match &cli.command {
        Command::Server(config) => config.log_level.as_str(),
        _ => "info",
    };

    setup_tracing(log_level);

    match cli.command {
        Command::Inspect {
            command,
            meta_root,
            metadata_db,
        } => {
            use s3_cas::inspect::*;
            match command {
                InspectCommand::NumKeys => {
                    let num_keys = num_keys(meta_root, metadata_db)?;
                    println!("Number of keys: {num_keys}");
                }
                InspectCommand::DiskSpace => {
                    let disk_space = disk_space(meta_root, metadata_db);
                    println!("Disk space: {disk_space}");
                }
                InspectCommand::ListUsers => {
                    list_users(meta_root, metadata_db)?;
                }
                InspectCommand::UserStats { user_id } => {
                    user_stats(meta_root, metadata_db, user_id)?;
                }
                InspectCommand::ListBuckets { user } => {
                    list_buckets(meta_root, metadata_db, user)?;
                }
                InspectCommand::BucketStats { bucket, user } => {
                    bucket_stats(meta_root, metadata_db, bucket, user)?;
                }
                InspectCommand::BlockStats => {
                    block_stats(meta_root, metadata_db)?;
                }
                InspectCommand::ObjectInfo { bucket, key, user } => {
                    object_info(meta_root, metadata_db, bucket, key, user)?;
                }
            }
        }
        Command::Retrieve(config) => retrieve(config)?,
        Command::Check(config) => check_integrity(config)?,
        Command::Presign(config) => presign(config)?,
        Command::User {
            meta_root,
            metadata_db,
            command,
        } => run_user_command(meta_root, metadata_db, command)?,
        Command::Server(config) => {
            run(config)?;
        }
    }
    Ok(())
}

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;

fn open_user_store(meta_root: PathBuf, engine: StorageEngine) -> Result<Arc<UserStore>> {
    let shared = SharedBlockStore::new(meta_root.join("blocks"), engine, None, None)?;
    let store = shared.meta_store().get_underlying_store();
    Ok(Arc::new(UserStore::new(store)))
}

fn generate_random_string(length: usize, charset: &[u8]) -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    (0..length)
        .map(|_| charset[rng.random_range(0..charset.len())] as char)
        .collect()
}

fn generate_access_key() -> String {
    generate_random_string(20, b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
}

fn generate_secret_key() -> String {
    generate_random_string(
        40,
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789/+",
    )
}

fn run_user_command(
    meta_root: PathBuf,
    engine: StorageEngine,
    cmd: UserCommand,
) -> Result<()> {
    let user_store = open_user_store(meta_root, engine)?;

    match cmd {
        UserCommand::Add {
            user_id,
            access_key,
            secret_key,
            admin,
        } => {
            let access_key = access_key.unwrap_or_else(generate_access_key);
            let secret_key = secret_key.unwrap_or_else(generate_secret_key);

            let record = UserRecord::new(
                user_id.clone(),
                access_key.clone(),
                secret_key.clone(),
                admin,
            )
            .map_err(|e| anyhow::anyhow!("failed to build user record: {}", e))?;

            user_store
                .create_user(record)
                .map_err(|e| anyhow::anyhow!("failed to create user: {}", e))?;

            println!("User '{}' created (admin={})", user_id, admin);
            println!("  access_key: {}", access_key);
            println!("  secret_key: {}", secret_key);
            println!("Save these credentials -- they will not be shown again.");
        }
        UserCommand::List => {
            let users = user_store
                .list_users()
                .map_err(|e| anyhow::anyhow!("failed to list users: {}", e))?;
            if users.is_empty() {
                println!("No users.");
                return Ok(());
            }
            println!("{:<20} {:<24} {:<6}", "USER_ID", "ACCESS_KEY", "ADMIN");
            for u in users {
                println!(
                    "{:<20} {:<24} {:<6}",
                    u.user_id,
                    u.s3_access_key,
                    if u.is_admin { "yes" } else { "no" }
                );
            }
        }
        UserCommand::Delete { user_id } => {
            user_store
                .delete_user(&user_id)
                .map_err(|e| anyhow::anyhow!("failed to delete user: {}", e))?;
            println!("User '{}' deleted.", user_id);
            println!(
                "Note: per-user object metadata under meta_root/user_{} is not removed by this command.",
                user_id
            );
        }
    }

    Ok(())
}

#[tokio::main]
async fn run(mut args: ServerConfig) -> Result<()> {
    // Canonicalize paths to avoid repeated getcwd() syscalls in async operations.
    args.fs_root = args.fs_root.canonicalize().unwrap_or_else(|_| {
        std::fs::create_dir_all(&args.fs_root).ok();
        args.fs_root
            .canonicalize()
            .unwrap_or_else(|_| std::env::current_dir().unwrap().join(&args.fs_root))
    });

    args.meta_root = args.meta_root.canonicalize().unwrap_or_else(|_| {
        std::fs::create_dir_all(&args.meta_root).ok();
        args.meta_root
            .canonicalize()
            .unwrap_or_else(|_| std::env::current_dir().unwrap().join(&args.meta_root))
    });

    info!("Using fs_root: {}", args.fs_root.display());
    info!("Using meta_root: {}", args.meta_root.display());

    let storage_engine = args.metadata_db;
    let metrics = s3_cas::metrics::SharedMetrics::new();

    // Shared block store (singleton for all users).
    let shared_block_store = Arc::new(SharedBlockStore::new(
        args.meta_root.join("blocks"),
        storage_engine,
        args.inline_metadata_size,
        Some(args.durability),
    )?);

    let user_store = Arc::new(UserStore::new(
        shared_block_store.meta_store().get_underlying_store(),
    ));

    let user_count = user_store
        .count_users()
        .map_err(|e| anyhow::anyhow!("failed to count users: {}", e))?;
    if user_count == 0 {
        bail!(
            "No users in database. Create one first:\n  \
             s3-cas user --meta-root {} add <user_id> --admin",
            args.meta_root.display()
        );
    }
    info!("Found {} user(s) in database", user_count);

    let user_router = Arc::new(UserRouter::new(
        shared_block_store.clone(),
        args.fs_root.clone(),
        args.meta_root.clone(),
        metrics.clone(),
        storage_engine,
        args.inline_metadata_size,
        Some(args.durability),
    ));

    let s3_user_router = S3UserRouter::new(user_router.clone(), user_store.clone());
    let s3_service = s3_cas::metrics::MetricFs::new(s3_user_router, metrics.clone());

    let service = {
        let auth = DynamicS3Auth::new(user_store.clone());
        let mut b = s3s::service::S3ServiceBuilder::new(s3_service);
        b.set_auth(auth);
        b.build()
    };

    run_server(args, service).await
}

async fn run_server(args: ServerConfig, service: s3s::service::S3Service) -> Result<()> {
    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port)).await?;
    let local_addr = listener.local_addr()?;

    let hyper_service = service;

    let metrics_listener =
        tokio::net::TcpListener::bind((args.metric_host.as_str(), args.metric_port)).await?;
    let metrics_addr = metrics_listener.local_addr()?;
    info!("metrics server is running at http://{metrics_addr}");

    let metrics_service = hyper::service::service_fn(
        move |req: hyper::Request<hyper::body::Incoming>| async move {
            match (req.method(), req.uri().path()) {
                (&hyper::Method::GET, "/metrics") => {
                    let mut buffer = Vec::new();
                    let encoder = prometheus::TextEncoder::new();
                    let metric_families = prometheus::gather();
                    encoder.encode(&metric_families, &mut buffer).unwrap();

                    Ok::<_, std::convert::Infallible>(
                        hyper::Response::builder()
                            .status(200)
                            .header(hyper::header::CONTENT_TYPE, "text/plain; version=0.0.4")
                            .body(Full::new(Bytes::from(buffer)))
                            .unwrap(),
                    )
                }
                _ => Ok::<_, std::convert::Infallible>(
                    hyper::Response::builder()
                        .status(404)
                        .body(Full::new(Bytes::from("Not Found")))
                        .unwrap(),
                ),
            }
        },
    );

    let http_server = ConnBuilder::new(TokioExecutor::new());
    let graceful = hyper_util::server::graceful::GracefulShutdown::new();

    let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());

    info!("server is running at http://{local_addr}");

    loop {
        tokio::select! {
            res = listener.accept() => {
                match res {
                    Ok((socket, _)) => {
                        let conn = http_server.serve_connection(TokioIo::new(socket), hyper_service.clone());
                        let conn = graceful.watch(conn.into_owned());
                        tokio::spawn(async move {
                            let _ = conn.await;
                        });
                        continue;
                    }
                    Err(err) => {
                        tracing::error!("error accepting connection: {err}");
                        continue;
                    }
                }
            }
            res = metrics_listener.accept() => {
                match res {
                    Ok((socket, _)) => {
                        let conn = http_server.serve_connection(TokioIo::new(socket), metrics_service);
                        let conn = graceful.watch(conn.into_owned());
                        tokio::spawn(async move {
                            let _ = conn.await;
                        });
                        continue;
                    }
                    Err(err) => {
                        tracing::error!("error accepting metrics connection: {err}");
                        continue;
                    }
                }
            }
            _ = ctrl_c.as_mut() => {
                break;
            }
        };
    }

    tokio::select! {
        () = graceful.shutdown() => {
            tracing::debug!("Gracefully shutdown!");
        },
        () = tokio::time::sleep(std::time::Duration::from_secs(10)) => {
            tracing::debug!("Waited 10 seconds for graceful shutdown, aborting...");
        }
    }

    info!("server is stopped");
    Ok(())
}
