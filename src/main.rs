use s3_cas::cas::{CasFS, StorageEngine};

use std::path::PathBuf;

use anyhow::Result;

use structopt::StructOpt;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(StructOpt)]
struct Args {
    #[structopt(long, default_value = ".")]
    fs_root: PathBuf,

    #[structopt(long, default_value = ".")]
    meta_root: PathBuf,

    #[structopt(long, default_value = "localhost")]
    host: String,

    #[structopt(long, default_value = "8014")]
    port: u16,

    #[structopt(long, requires("secret-key"), display_order = 1000)]
    access_key: Option<String>,

    #[structopt(long, requires("access-key"), display_order = 1000)]
    secret_key: Option<String>,

    #[structopt(long = "sled", parse(from_flag = std::ops::Not::not))]
    sled: bool,
}

fn setup_tracing() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    setup_tracing();

    let args: Args = Args::from_args();

    run(args)
}

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use s3s::service::S3ServiceBuilder;

#[tokio::main]
async fn run(args: Args) -> anyhow::Result<()> {
    info!("args fjall = {}", args.sled);
    let storage_engine = if args.sled {
        StorageEngine::Sled
    } else {
        StorageEngine::Fjall
    };
    // provider
    let metrics = s3_cas::metrics::SharedMetrics::new();
    let casfs = CasFS::new(
        args.fs_root.clone(),
        args.meta_root.clone(),
        metrics.clone(),
        storage_engine,
    );
    let s3fs = s3_cas::s3fs::S3FS::new(args.fs_root, args.meta_root, casfs, metrics.clone());

    // Setup S3 service
    let service = {
        let mut b = S3ServiceBuilder::new(s3fs);

        // Enable authentication
        if let (Some(ak), Some(sk)) = (args.access_key, args.secret_key) {
            b.set_auth(s3s::auth::SimpleAuth::from_single(ak, sk));
            info!("authentication is enabled");
        }

        b.build()
    };

    // Run server
    // Update TcpListener bind with proper error context
    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port)).await?;
    let local_addr = listener.local_addr()?;

    let hyper_service = service.into_shared();

    let http_server = ConnBuilder::new(TokioExecutor::new());
    let graceful = hyper_util::server::graceful::GracefulShutdown::new();

    let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());

    info!("server is running at http://{local_addr}");

    loop {
        let (socket, _) = tokio::select! {
            res =  listener.accept() => {
                match res {
                    Ok(conn) => conn,
                    Err(err) => {
                        tracing::error!("error accepting connection: {err}");
                        continue;
                    }
                }
            }
            _ = ctrl_c.as_mut() => {
                break;
            }
        };

        let conn = http_server.serve_connection(TokioIo::new(socket), hyper_service.clone());
        let conn = graceful.watch(conn.into_owned());
        tokio::spawn(async move {
            let _ = conn.await;
        });
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
