use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use clap::{Parser, Subcommand};
use http_body_util::Full;
use prometheus::Encoder;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use cas_storage::{CasFS, StorageEngine};
use s3_cas::check::{check_integrity, CheckConfig};
use cas_storage::Durability;
use s3_cas::retrieve::{retrieve, RetrieveConfig};

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

    #[arg(long, help = "Enable HTTP browser interface")]
    enable_http_ui: bool,

    #[arg(long, default_value = "localhost")]
    http_ui_host: String,

    #[arg(long, default_value = "8080")]
    http_ui_port: u16,

    #[arg(
        long,
        help = "HTTP UI username (enables basic auth if set with --http-ui-password)"
    )]
    http_ui_username: Option<String>,

    #[arg(
        long,
        help = "HTTP UI password (enables basic auth if set with --http-ui-username)"
    )]
    http_ui_password: Option<String>,

    #[arg(long, help = "leave empty to disable it")]
    inline_metadata_size: Option<usize>,

    #[arg(long, display_order = 1000, help = "S3 access key (required in single-user mode)")]
    access_key: Option<String>,

    #[arg(long, display_order = 1000, help = "S3 secret key (required in single-user mode)")]
    secret_key: Option<String>,

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

        #[arg(long, help = "Path to users config file for multi-user mode")]
        users_config: Option<PathBuf>,

        #[command(subcommand)]
        command: InspectCommand,
    },

    /// retrieve an object
    Retrieve(RetrieveConfig),

    /// Check object integrity
    Check(CheckConfig),

    /// Start S3-cas server
    Server(ServerConfig),
}

#[derive(Debug, Subcommand)]
pub enum InspectCommand {
    /// Number of keys (objects) in database
    NumKeys,
    /// Total disk space used by database
    DiskSpace,
    /// List all users (multi-user mode only)
    ListUsers,
    /// Show per-user storage statistics
    UserStats {
        /// Specific user ID to show stats for (optional)
        user_id: Option<String>,
    },
    /// List all buckets
    ListBuckets {
        /// Filter by user ID (multi-user mode)
        #[arg(long)]
        user: Option<String>,
    },
    /// Show statistics for a specific bucket
    BucketStats {
        /// Bucket name
        bucket: String,
        /// User ID (required in multi-user mode)
        #[arg(long)]
        user: Option<String>,
    },
    /// Show block storage statistics and deduplication ratio
    BlockStats,
    /// Show detailed information about a specific object
    ObjectInfo {
        /// Bucket name
        bucket: String,
        /// Object key
        key: String,
        /// User ID (required in multi-user mode)
        #[arg(long)]
        user: Option<String>,
    },
}

fn setup_tracing(log_level: &str) {
    // Try to use RUST_LOG env var first, fall back to CLI flag
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
    // console_subscriber::init();
    dotenv::dotenv().ok();

    let cli = Cli::parse();

    // Extract log level from Server command, or use default for other commands
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
            users_config,
        } => {
            use s3_cas::inspect::*;
            match command {
                InspectCommand::NumKeys => {
                    let num_keys = num_keys(meta_root, metadata_db, users_config)?;
                    println!("Number of keys: {num_keys}");
                }
                InspectCommand::DiskSpace => {
                    let disk_space = disk_space(meta_root, metadata_db, users_config);
                    println!("Disk space: {disk_space}");
                }
                InspectCommand::ListUsers => {
                    list_users(meta_root, metadata_db, users_config)?;
                }
                InspectCommand::UserStats { user_id } => {
                    user_stats(meta_root, metadata_db, users_config, user_id)?;
                }
                InspectCommand::ListBuckets { user } => {
                    list_buckets(meta_root, metadata_db, users_config, user)?;
                }
                InspectCommand::BucketStats { bucket, user } => {
                    bucket_stats(meta_root, metadata_db, users_config, bucket, user)?;
                }
                InspectCommand::BlockStats => {
                    block_stats(meta_root, metadata_db, users_config)?;
                }
                InspectCommand::ObjectInfo { bucket, key, user } => {
                    object_info(meta_root, metadata_db, users_config, bucket, key, user)?;
                }
            }
        }
        Command::Retrieve(config) => retrieve(config)?,
        Command::Check(config) => check_integrity(config)?,
        Command::Server(config) => {
            run(config)?;
        }
    }
    Ok(())
}

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use s3s::service::S3ServiceBuilder;

#[tokio::main]
async fn run(mut args: ServerConfig) -> anyhow::Result<()> {
    // Canonicalize paths to avoid repeated getcwd() syscalls in async operations
    // This is critical for performance when using relative paths
    args.fs_root = args.fs_root.canonicalize()
        .unwrap_or_else(|_| {
            std::fs::create_dir_all(&args.fs_root).ok();
            args.fs_root.canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap().join(&args.fs_root))
        });

    args.meta_root = args.meta_root.canonicalize()
        .unwrap_or_else(|_| {
            std::fs::create_dir_all(&args.meta_root).ok();
            args.meta_root.canonicalize()
                .unwrap_or_else(|_| std::env::current_dir().unwrap().join(&args.meta_root))
        });

    info!("Using fs_root: {}", args.fs_root.display());
    info!("Using meta_root: {}", args.meta_root.display());

    let storage_engine = args.metadata_db;
    let metrics = s3_cas::metrics::SharedMetrics::new();

    // Check if single-user mode is explicitly requested
    if args.access_key.is_some() && args.secret_key.is_some() {
        info!("Single-user mode (explicit credentials provided)");
        run_single_user(args, storage_engine, metrics).await
    } else if args.access_key.is_some() || args.secret_key.is_some() {
        anyhow::bail!(
            "Single-user mode requires both --access-key and --secret-key.\n\
             Omit both for multi-user mode with database-backed authentication."
        );
    } else {
        info!("Multi-user mode (database-backed authentication)");
        run_multi_user(args, storage_engine, metrics).await
    }
}

async fn run_single_user(
    args: ServerConfig,
    storage_engine: cas_storage::StorageEngine,
    metrics: s3_cas::metrics::SharedMetrics,
) -> anyhow::Result<()> {
    // Original single-user implementation
    let casfs = CasFS::new(
        args.fs_root.clone(),
        args.meta_root.clone(),
        metrics.to_cas_metrics(),
        storage_engine,
        args.inline_metadata_size,
        Some(args.durability),
    );
    let s3fs = s3_cas::s3fs::S3FS::new(Arc::new(casfs), metrics.clone());
    let s3fs = s3_cas::metrics::MetricFs::new(s3fs, metrics.clone());

    // HTTP UI service (if enabled)
    let http_ui_service = if args.enable_http_ui {
        let http_casfs = CasFS::new(
            args.fs_root.clone(),
            args.meta_root.clone(),
            metrics.to_cas_metrics(),
            storage_engine,
            args.inline_metadata_size,
            Some(args.durability),
        );

        let http_ui_username = args.http_ui_username.clone();
        let http_ui_password = args.http_ui_password.clone();
        let auth = match (http_ui_username, http_ui_password) {
            (Some(username), Some(password)) => {
                info!("HTTP UI basic auth enabled for user: {}", username);
                Some(s3_cas::http_ui::BasicAuth::new(username, password))
            }
            _ => None,
        };

        Some(s3_cas::http_ui::HttpUiServiceWrapper::SingleUser(
            s3_cas::http_ui::HttpUiService::new(
                http_casfs,
                metrics.clone(),
                auth,
            )
        ))
    } else {
        None
    };

    // Setup S3 service
    let service = {
        let mut b = S3ServiceBuilder::new(s3fs);

        // Enable authentication
        let access_key = args.access_key.clone();
        let secret_key = args.secret_key.clone();
        if let (Some(ak), Some(sk)) = (access_key, secret_key) {
            b.set_auth(s3s::auth::SimpleAuth::from_single(ak, sk));
            info!("authentication is enabled");
        }

        b.build()
    };

    run_server(args, service, http_ui_service, metrics).await
}

async fn run_multi_user(
    args: ServerConfig,
    storage_engine: cas_storage::StorageEngine,
    metrics: s3_cas::metrics::SharedMetrics,
) -> anyhow::Result<()> {
    use s3_cas::auth::UserRouter;
    use cas_storage::SharedBlockStore;
    use s3_cas::s3_wrapper::DynamicS3Auth;

    info!("Starting multi-user mode with dynamic authentication");

    // Create shared block store (singleton for all users)
    let shared_block_store = Arc::new(SharedBlockStore::new(
        args.meta_root.join("blocks"),
        storage_engine,
        args.inline_metadata_size,
        Some(args.durability),
    )?);

    // Create UserStore using the same storage backend as SharedBlockStore
    let user_store = Arc::new(s3_cas::auth::UserStore::new(
        shared_block_store.meta_store().get_underlying_store()
    ));

    // Create SessionStore for HTTP UI authentication
    let session_store = Arc::new(s3_cas::auth::SessionStore::new());

    // Create user router with lazy CasFS initialization
    let user_router = Arc::new(UserRouter::new(
        shared_block_store.clone(),
        args.fs_root.clone(),
        args.meta_root.clone(),
        metrics.clone(),
        storage_engine,
        args.inline_metadata_size,
        Some(args.durability),
    ));

    let user_count = user_store.count_users()?;
    if user_count == 0 {
        info!("No users found in database. First user will be created through HTTP UI setup.");
    } else {
        info!("Found {} user(s) in database", user_count);
    }

    // Create S3UserRouter for per-request routing
    info!("Setting up S3UserRouter with dynamic authentication");
    let s3_user_router = s3_cas::s3_wrapper::S3UserRouter::new(
        user_router.clone(),
        user_store.clone(),
    );
    let s3_service = s3_cas::metrics::MetricFs::new(s3_user_router, metrics.clone());

    // HTTP UI service (if enabled) - multi-user with session-based auth
    let http_ui_service = if args.enable_http_ui {
        info!("HTTP UI enabled with session-based authentication");
        Some(s3_cas::http_ui::HttpUiServiceWrapper::MultiUser(
            s3_cas::http_ui::HttpUiServiceMultiUser::new(
                user_router.clone(),
                user_store.clone(),
                session_store.clone(),
                metrics.clone(),
            )
        ))
    } else {
        None
    };

    // Setup S3 service with dynamic authentication
    let service = {
        let auth = DynamicS3Auth::new(user_store.clone());
        let mut b = s3s::service::S3ServiceBuilder::new(s3_service);
        b.set_auth(auth);
        info!("Multi-user S3 service enabled with dynamic authentication");
        b.build()
    };

    // Spawn background task for session cleanup and metrics
    {
        let session_store_clone = session_store.clone();
        let metrics_clone = metrics.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;

                // Clean up expired sessions
                let removed = session_store_clone.cleanup_expired();
                if removed > 0 {
                    tracing::debug!(removed = removed, "Cleaned up expired sessions");
                }

                // Update active session count metric
                let active_count = session_store_clone.active_session_count();
                metrics_clone.set_active_sessions(active_count);
                tracing::trace!(active_sessions = active_count, "Updated session metrics");
            }
        });
        info!("Started background session cleanup and metrics task");
    }

    run_server(args, service, http_ui_service, metrics).await
}

async fn run_server(
    args: ServerConfig,
    service: s3s::service::S3Service,
    http_ui_service: Option<s3_cas::http_ui::HttpUiServiceWrapper>,
    _metrics: s3_cas::metrics::SharedMetrics,
) -> anyhow::Result<()> {

    // Run server
    // S3 listener
    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port)).await?;
    let local_addr = listener.local_addr()?;

    let hyper_service = service.into_shared();

    // metrics server
    // Add after the main listener setup
    let metrics_listener =
        tokio::net::TcpListener::bind((args.metric_host.as_str(), args.metric_port)).await?;
    let metrics_addr = metrics_listener.local_addr()?;

    info!("metrics server is running at http://{metrics_addr}");

    // HTTP UI server (optional)
    let http_ui_listener = if args.enable_http_ui {
        let listener =
            tokio::net::TcpListener::bind((args.http_ui_host.as_str(), args.http_ui_port)).await?;
        let addr = listener.local_addr()?;
        info!("HTTP UI server is running at http://{addr}");
        Some(listener)
    } else {
        None
    };

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
                    Ok((socket,_)) => {
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
                    Ok((socket, _)) =>{
                        let conn = http_server.serve_connection(TokioIo::new(socket), metrics_service);
                        let conn = graceful.watch(conn.into_owned());
                        tokio::spawn(async move {
                            let _ = conn.await;
                        });
                        continue;

                    }// (socket, metrics_service.clone()),
                    Err(err) => {
                        tracing::error!("error accepting metrics connection: {err}");
                        continue;
                    }
                }
            }
            res = async {
                match &http_ui_listener {
                    Some(listener) => listener.accept().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(ref service) = http_ui_service {
                    match res {
                        Ok((socket, _)) => {
                            let service_clone = service.clone();
                            let http_ui_handler = hyper::service::service_fn(move |req| {
                                let service = service_clone.clone();
                                async move { service.handle_request(req).await }
                            });
                            let conn = http_server.serve_connection(TokioIo::new(socket), http_ui_handler);
                            let conn = graceful.watch(conn.into_owned());
                            tokio::spawn(async move {
                                let _ = conn.await;
                            });
                            continue;
                        }
                        Err(err) => {
                            tracing::error!("error accepting HTTP UI connection: {err}");
                            continue;
                        }
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
