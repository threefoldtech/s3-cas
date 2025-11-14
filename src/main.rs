use std::path::PathBuf;

use anyhow::Result;
use bytes::Bytes;
use clap::{Parser, Subcommand};
use http_body_util::Full;
use prometheus::Encoder;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

use s3_cas::cas::{CasFS, StorageEngine};
use s3_cas::check::{check_integrity, CheckConfig};
use s3_cas::inspect::{disk_space, num_keys};
use s3_cas::metastore::Durability;
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

    #[arg(long, required = true, display_order = 1000)]
    access_key: Option<String>,

    #[arg(long, required = true, display_order = 1000)]
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
        help = "Path to users.toml config file (enables multi-user mode)"
    )]
    users_config: Option<PathBuf>,
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

    /// Start S3-cas server
    Server(ServerConfig),
}

#[derive(Debug, Subcommand)]
pub enum InspectCommand {
    // number of keys
    NumKeys,
    DiskSpace,
}

fn setup_tracing() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}

fn main() -> Result<()> {
    // console_subscriber::init();
    dotenv::dotenv().ok();

    setup_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect {
            command,
            meta_root,
            metadata_db,
        } => match command {
            InspectCommand::NumKeys => {
                let num_keys = num_keys(meta_root, metadata_db)?;
                println!("Number of keys: {num_keys}");
            }
            InspectCommand::DiskSpace => {
                let disk_space = disk_space(meta_root, metadata_db);
                println!("Disk space: {disk_space}");
            }
        },
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
async fn run(args: ServerConfig) -> anyhow::Result<()> {
    let storage_engine = args.metadata_db;
    let metrics = s3_cas::metrics::SharedMetrics::new();

    // Check if multi-user mode is enabled
    let users_config_path = args.users_config.clone();
    if let Some(users_config_path) = users_config_path {
        info!("Multi-user mode enabled, loading users from {:?}", users_config_path);
        run_multi_user(args, storage_engine, metrics, users_config_path).await
    } else {
        info!("Single-user mode");
        run_single_user(args, storage_engine, metrics).await
    }
}

async fn run_single_user(
    args: ServerConfig,
    storage_engine: s3_cas::cas::StorageEngine,
    metrics: s3_cas::metrics::SharedMetrics,
) -> anyhow::Result<()> {
    // Original single-user implementation
    let casfs = CasFS::new(
        args.fs_root.clone(),
        args.meta_root.clone(),
        metrics.clone(),
        storage_engine,
        args.inline_metadata_size,
        Some(args.durability),
    );
    let s3fs = s3_cas::s3fs::S3FS::new(casfs, metrics.clone());
    let s3fs = s3_cas::metrics::MetricFs::new(s3fs, metrics.clone());

    // HTTP UI service (if enabled)
    let http_ui_service = if args.enable_http_ui {
        let http_casfs = CasFS::new(
            args.fs_root.clone(),
            args.meta_root.clone(),
            metrics.clone(),
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

        Some(s3_cas::http_ui::HttpUiService::new(
            http_casfs,
            metrics.clone(),
            auth,
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
    storage_engine: s3_cas::cas::StorageEngine,
    metrics: s3_cas::metrics::SharedMetrics,
    users_config_path: PathBuf,
) -> anyhow::Result<()> {
    use s3_cas::auth::{UsersConfig, UserRouter};
    use s3_cas::cas::SharedBlockStore;

    // Load users configuration
    let users_config = UsersConfig::load_from_file(&users_config_path)
        .map_err(|e| anyhow::anyhow!("Failed to load users config: {}", e))?;

    info!("Loaded {} users from config", users_config.users.len());

    // Create shared block store
    let shared_block_store = SharedBlockStore::new(
        args.meta_root.join("blocks"),
        storage_engine,
        args.inline_metadata_size,
        Some(args.durability),
    )?;

    // Create user router with pre-created CasFS instances
    // NOTE: Not used in Phase 1 - we create CasFS instances directly below
    let _user_router = UserRouter::new(
        users_config.clone(),
        &shared_block_store,
        args.fs_root.clone(),
        args.meta_root.clone(),
        metrics.clone(),
        storage_engine,
        args.inline_metadata_size,
        Some(args.durability),
    );

    // Build S3 auth from user config
    let mut auth_builder = s3s::auth::SimpleAuth::new();
    for (user_id, user) in &users_config.users {
        auth_builder.register(user.access_key.clone(), user.secret_key.clone().into());
        info!("Registered user: {}", user_id);
    }

    // TEMPORARY: Create a CasFS instance for the first user for S3 service
    // TODO: Proper per-request routing would require a custom S3 trait implementation
    let first_user_id = users_config.users.keys().next()
        .ok_or_else(|| anyhow::anyhow!("No users configured"))?;

    let s3_casfs = CasFS::new_multi_user(
        args.fs_root.clone(),
        args.meta_root.join(format!("user_{}", first_user_id)),
        shared_block_store.block_tree(),
        shared_block_store.path_tree(),
        shared_block_store.multipart_tree(),
        metrics.clone(),
        storage_engine,
        args.inline_metadata_size,
        Some(args.durability),
    );

    let s3fs = s3_cas::s3fs::S3FS::new(s3_casfs, metrics.clone());
    let s3fs = s3_cas::metrics::MetricFs::new(s3fs, metrics.clone());

    // HTTP UI service (if enabled) - use first user for now
    let http_ui_service = if args.enable_http_ui {
        let http_casfs = CasFS::new_multi_user(
            args.fs_root.clone(),
            args.meta_root.join(format!("user_{}", first_user_id)),
            shared_block_store.block_tree(),
            shared_block_store.path_tree(),
            shared_block_store.multipart_tree(),
            metrics.clone(),
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

        Some(s3_cas::http_ui::HttpUiService::new(
            http_casfs,
            metrics.clone(),
            auth,
        ))
    } else {
        None
    };

    // Setup S3 service with multi-user auth
    let service = {
        let mut b = S3ServiceBuilder::new(s3fs);
        b.set_auth(auth_builder);
        info!("Multi-user authentication enabled");
        b.build()
    };

    run_server(args, service, http_ui_service, metrics).await
}

async fn run_server(
    args: ServerConfig,
    service: s3s::service::S3Service,
    http_ui_service: Option<s3_cas::http_ui::HttpUiService>,
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
