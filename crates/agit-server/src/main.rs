//! `agit-server`: hosts repositories over HTTP. See docs/API.md.

mod api;
mod error;
mod knowledge;
mod policy;
mod pulls;
mod state;
mod webhooks;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

#[derive(Parser)]
#[command(name = "agit-server", version, about = "agit hosting server")]
struct Args {
    /// Directory where repositories are stored.
    #[arg(long, env = "AGIT_DATA_DIR", default_value = "./agit-data")]
    data_dir: PathBuf,
    /// Listen address.
    #[arg(long, env = "AGIT_LISTEN", default_value = "127.0.0.1:7878")]
    listen: SocketAddr,
    /// Bearer token required on every request (unauthenticated when unset).
    #[arg(long, env = "AGIT_SERVER_TOKEN")]
    token: Option<String>,
    /// Serve the built web UI (web/dist) from `/`.
    #[arg(long, env = "AGIT_WEB_DIR")]
    web_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();
    let args = Args::parse();
    let state = Arc::new(state::AppState::open(
        args.data_dir.clone(),
        args.token.clone(),
    )?);
    let app = api::router(state, args.web_dir.as_deref());
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(addr = %args.listen, data_dir = %args.data_dir.display(), auth = args.token.is_some(), web = args.web_dir.as_ref().map(|p| p.display().to_string()), "agit-server listening");
    axum::serve(listener, app).await?;
    Ok(())
}
