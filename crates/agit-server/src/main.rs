//! `agit-server`: hosts repositories over HTTP. See docs/API.md.

mod api;
mod auth;
mod error;
mod keys;
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
    /// SSH public key with full admin rights: an `ssh-ed25519 …` line or a
    /// path to a file with one key per line (repeatable; AGIT_ADMIN_KEYS
    /// takes several separated by commas). Configuring one closes the
    /// server: every request then needs an SSH-key session or a token.
    #[arg(long = "admin-key", env = "AGIT_ADMIN_KEYS", value_delimiter = ',')]
    admin_keys: Vec<String>,
    /// Run without any authentication on a non-loopback address.
    #[arg(long, env = "AGIT_ALLOW_OPEN", value_parser = clap::builder::FalseyValueParser::new())]
    allow_open: bool,
    /// Largest accepted object upload request in MiB. A push sends objects in
    /// batches of about 8 MiB, but a file larger than that travels on its own,
    /// so this must exceed the largest file in your repositories.
    #[arg(long, env = "AGIT_MAX_UPLOAD_MB", default_value_t = 1024)]
    max_upload_mb: usize,
}

/// Parse `--admin-key` values: a key line, or a path to a file holding one
/// key per line (`#` comments and blank lines are ignored; keys of other
/// types are skipped with a warning).
fn load_admin_keys(values: &[String]) -> anyhow::Result<Vec<agit_core::sshkey::SshPublicKey>> {
    use agit_core::sshkey::SshPublicKey;
    let mut out = Vec::new();
    for v in values {
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        let looks_like_key = v.starts_with("ssh-")
            || v.starts_with("ecdsa-")
            || v.starts_with("sk-")
            || (v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()));
        if looks_like_key {
            out.push(SshPublicKey::parse(v).map_err(|e| anyhow::anyhow!("--admin-key: {e}"))?);
            continue;
        }
        let text = std::fs::read_to_string(v)
            .map_err(|e| anyhow::anyhow!("--admin-key {v}: cannot read file: {e}"))?;
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match SshPublicKey::parse(line) {
                Ok(k) => out.push(k),
                Err(e)
                    if line.starts_with("ssh-")
                        || line.starts_with("ecdsa-")
                        || line.starts_with("sk-") =>
                {
                    tracing::warn!(file = %v, line = n + 1, %e, "skipping unsupported admin key");
                }
                Err(e) => anyhow::bail!("--admin-key {v} line {}: {e}", n + 1),
            }
        }
    }
    Ok(out)
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
    let admin_keys = load_admin_keys(&args.admin_keys)?;
    let closed = args.token.is_some() || !admin_keys.is_empty();
    if !closed && !args.listen.ip().is_loopback() && !args.allow_open {
        anyhow::bail!(
            "refusing to start an open server on {}: set AGIT_SERVER_TOKEN or --admin-key \
             <ssh public key> so that only authorized clients can reach repositories, or pass \
             --allow-open to run without authentication",
            args.listen
        );
    }
    for k in &admin_keys {
        tracing::info!(fingerprint = %k.fingerprint(), comment = k.comment.as_deref().unwrap_or(""), "admin key");
    }
    let state = Arc::new(
        state::AppState::open(args.data_dir.clone(), args.token.clone())?
            .with_admin_keys(admin_keys)
            .with_max_upload_bytes(args.max_upload_mb.saturating_mul(1024 * 1024)),
    );
    if !closed {
        tracing::warn!("no authentication configured: every request acts as root (open mode)");
    }
    let app = api::router(state, args.web_dir.as_deref());
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(addr = %args.listen, data_dir = %args.data_dir.display(), auth = closed, root_token = args.token.is_some(), admin_keys = args.admin_keys.len(), max_upload_mb = args.max_upload_mb, web = args.web_dir.as_ref().map(|p| p.display().to_string()), "agit-server listening");
    axum::serve(listener, app).await?;
    Ok(())
}
