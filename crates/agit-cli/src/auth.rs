//! `agit auth`: sign in with an SSH key, manage authorized keys and tokens.
//!
//! The server is chosen with `--url <base>`, else the remote of the
//! repository in the current directory (`--remote`, default `origin`), else
//! `AGIT_SERVER_URL`. All logic lives in `agit_protocol::sshauth` and the
//! client; this module only parses arguments and prints.

use std::path::PathBuf;

use agit_protocol::client::RepoUrl;
use agit_protocol::sshauth;
use agit_protocol::types::*;
use agit_protocol::{sync, Client};
use anyhow::{bail, Context, Result};
use clap::Subcommand;

use crate::output::{emit, fmt_time, parse_duration};

#[derive(Subcommand)]
pub enum AuthCmd {
    /// Sign in with an SSH key (AGIT_SSH_KEY, ~/.ssh/id_ed25519, ssh-agent or
    /// an agit key) and cache the session for this server.
    Login {
        /// Private key file to use (OpenSSH format or an agit key).
        #[arg(long)]
        key: Option<PathBuf>,
        /// Session lifetime (default 8h, at most 30d).
        #[arg(long)]
        ttl: Option<String>,
        /// Also print the session token (for the web UI or an SDK).
        #[arg(long)]
        print_token: bool,
    },
    /// Revoke the session for this server and forget it locally.
    Logout,
    /// Show who the server thinks you are: principal, agent, scopes.
    Whoami,
    /// Print the cached session token for this server.
    ShowToken,
    /// Authorized SSH keys: who may sign in, with which scopes (admin).
    Key {
        #[command(subcommand)]
        cmd: AuthKeyCmd,
    },
    /// Scoped bearer tokens for agents and CI (admin).
    Token {
        #[command(subcommand)]
        cmd: AuthTokenCmd,
    },
}

#[derive(Subcommand)]
pub enum AuthKeyCmd {
    /// Authorize a public key: an `ssh-ed25519 …` line, a hex key, or a path
    /// to a `.pub` / authorized_keys file (every key in it is added).
    Add {
        key: String,
        /// Display name (default: the key's comment, then its fingerprint).
        #[arg(long)]
        name: Option<String>,
        /// Sessions opened with this key always act as this agent.
        #[arg(long)]
        agent: Option<String>,
        /// `repo:read|write|admin` or `*:read|write|admin`; repeatable.
        #[arg(long = "scope")]
        scopes: Vec<String>,
        /// Shorthand for `--scope '*:admin'`.
        #[arg(long)]
        admin: bool,
    },
    List,
    /// Remove a key; every session it opened is revoked.
    Remove {
        id: String,
    },
}

#[derive(Subcommand)]
pub enum AuthTokenCmd {
    /// Mint a token. The plaintext is printed once.
    Create {
        #[arg(long)]
        name: String,
        /// Bind the token to an agent identity.
        #[arg(long)]
        agent: Option<String>,
        /// `repo:read|write|admin` or `*:…`; repeatable.
        #[arg(long = "scope")]
        scopes: Vec<String>,
        /// Shorthand for `--scope '*:admin'`.
        #[arg(long)]
        admin: bool,
        /// Expiry, e.g. `30d` (default: never).
        #[arg(long)]
        ttl: Option<String>,
    },
    List,
    Revoke {
        id: String,
    },
}

/// Pick the server base URL (`…/api/v1`).
fn server_base(dir: &Option<PathBuf>, url: Option<&str>, remote: &str) -> Result<String> {
    if let Some(u) = url {
        let u = u.trim().trim_end_matches('/');
        if u.contains("/repos/") {
            return Ok(RepoUrl::parse(u)?.base);
        }
        return Ok(u.to_string());
    }
    let start = match dir {
        Some(d) => d.clone(),
        None => std::env::current_dir()?,
    };
    if let Ok(repo) = agit_core::Repository::discover(&start) {
        if let Ok((parsed, _)) = sync::remote(&repo, remote) {
            return Ok(parsed.base);
        }
    }
    if let Some(u) = std::env::var_os("AGIT_SERVER_URL") {
        return Ok(u.to_string_lossy().trim_end_matches('/').to_string());
    }
    bail!(
        "no server: pass --url http://host:7878/api/v1, run inside a repository with a \
         remote `{remote}`, or set AGIT_SERVER_URL"
    )
}

fn parse_scopes(raw: &[String], admin: bool) -> Result<Vec<Scope>> {
    let mut out = Vec::new();
    for s in raw {
        let (repo, access) = s
            .rsplit_once(':')
            .with_context(|| format!("scope {s:?} must be <repo>:<read|write|admin>"))?;
        let access = match access.trim() {
            "read" | "r" => Access::Read,
            "write" | "w" => Access::Write,
            "admin" | "a" => Access::Admin,
            other => bail!("unknown access {other:?} in scope {s:?} (read, write or admin)"),
        };
        let repo = repo.trim();
        if repo.is_empty() {
            bail!("scope {s:?} has an empty repository (use * for all)");
        }
        out.push(Scope {
            repo: repo.to_string(),
            access,
        });
    }
    if admin {
        out.push(Scope {
            repo: "*".into(),
            access: Access::Admin,
        });
    }
    if out.is_empty() {
        bail!("at least one --scope (or --admin) is required");
    }
    Ok(out)
}

fn fmt_scopes(scopes: &[Scope]) -> String {
    if scopes.is_empty() {
        return "-".into();
    }
    scopes
        .iter()
        .map(|s| {
            format!(
                "{}:{}",
                s.repo,
                match s.access {
                    Access::Read => "read",
                    Access::Write => "write",
                    Access::Admin => "admin",
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn ttl_secs(ttl: Option<&str>) -> Result<Option<u64>> {
    ttl.map(|t| parse_duration(t).map(|d| d.as_secs()))
        .transpose()
}

pub fn run_auth(
    dir: &Option<PathBuf>,
    json: bool,
    url: Option<&str>,
    remote: &str,
    cmd: AuthCmd,
) -> Result<()> {
    let base = server_base(dir, url, remote)?;
    match cmd {
        AuthCmd::Login {
            key,
            ttl,
            print_token,
        } => {
            let client = Client::new(&base).with_auto_login(false);
            let (created, id) =
                client.login_with_ssh_key(key.as_deref(), ttl_secs(ttl.as_deref())?)?;
            let who = client.whoami()?;
            let fp = id.public.fingerprint();
            let mut result = serde_json::json!({
                "server": base,
                "principal": who.principal,
                "agent": who.agent,
                "scopes": who.scopes,
                "key": created.info.key,
                "fingerprint": fp,
                "source": id.source,
                "expires_ms": created.info.expires_ms,
            });
            if print_token {
                result["token"] = serde_json::Value::String(created.token.clone());
            }
            emit(json, &result, || {
                println!(
                    "signed in to {base} as {} with key {fp} ({})",
                    who.agent.clone().unwrap_or_else(|| who.principal.clone()),
                    id.source
                );
                println!("scopes: {}", fmt_scopes(&who.scopes));
                if let Some(e) = created.info.expires_ms {
                    println!("session expires {}", fmt_time(e));
                }
                if print_token {
                    println!("{}", created.token);
                }
            });
        }
        AuthCmd::Logout => {
            let client = Client::new(&base).with_auto_login(false);
            if client.token().is_none() {
                bail!("no session for {base}");
            }
            let r = client.logout()?;
            emit(json, &r, || println!("signed out of {base}"));
        }
        AuthCmd::Whoami => {
            let who = Client::new(&base).whoami()?;
            emit(json, &who, || {
                println!("server:    {base}");
                println!("principal: {}", who.principal);
                println!(
                    "agent:     {}",
                    who.agent
                        .as_deref()
                        .unwrap_or("- (from X-Agit-Agent / AGIT_AGENT)")
                );
                println!(
                    "access:    {}",
                    if who.root {
                        "root (everything)".to_string()
                    } else {
                        fmt_scopes(&who.scopes)
                    }
                );
                if let Some(k) = &who.key {
                    println!("key:       {k}");
                }
            });
        }
        AuthCmd::ShowToken => {
            let Some(s) = sshauth::cached_session(&base) else {
                bail!("no session for {base}; run `agit auth login`");
            };
            emit(
                json,
                &serde_json::json!({ "server": base, "token": s.token, "expires_ms": s.expires_ms, "agent": s.agent }),
                || println!("{}", s.token),
            );
        }
        AuthCmd::Key { cmd } => run_key(&base, json, cmd)?,
        AuthCmd::Token { cmd } => run_token(&base, json, cmd)?,
    }
    Ok(())
}

fn run_key(base: &str, json: bool, cmd: AuthKeyCmd) -> Result<()> {
    let client = Client::new(base);
    match cmd {
        AuthKeyCmd::Add {
            key,
            name,
            agent,
            scopes,
            admin,
        } => {
            let scopes = parse_scopes(&scopes, admin)?;
            let path = PathBuf::from(&key);
            let lines: Vec<String> = if path.is_file() {
                std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(str::to_string)
                    .collect()
            } else {
                vec![key.trim().to_string()]
            };
            if lines.is_empty() {
                bail!("{} holds no keys", path.display());
            }
            let mut added = Vec::new();
            for line in lines {
                added.push(client.add_authorized_key(&AuthorizedKeyCreate {
                    public_key: line,
                    name: name.clone(),
                    agent: agent.clone(),
                    scopes: scopes.clone(),
                })?);
            }
            emit(json, &added, || {
                for k in &added {
                    println!(
                        "authorized {} ({}) as {} with scopes {}{}",
                        k.name,
                        k.fingerprint,
                        k.id,
                        fmt_scopes(&k.scopes),
                        k.agent
                            .as_deref()
                            .map(|a| format!(", acting as agent {a}"))
                            .unwrap_or_default()
                    );
                }
            });
        }
        AuthKeyCmd::List => {
            let keys = client.authorized_keys()?;
            emit(json, &keys, || {
                if keys.is_empty() {
                    println!("no authorized keys (admin keys from --admin-key are not listed)");
                }
                for k in &keys {
                    println!(
                        "{}  {:<20} {}  agent={}  scopes={}  last used {}",
                        k.id,
                        k.name,
                        k.fingerprint,
                        k.agent.as_deref().unwrap_or("-"),
                        fmt_scopes(&k.scopes),
                        k.last_used_ms
                            .map(fmt_time)
                            .unwrap_or_else(|| "never".into())
                    );
                }
            });
        }
        AuthKeyCmd::Remove { id } => {
            let r = client.delete_authorized_key(&id)?;
            emit(json, &r, || {
                println!(
                    "removed key {id} ({} session(s) revoked)",
                    r["revoked_sessions"].as_u64().unwrap_or(0)
                )
            });
        }
    }
    Ok(())
}

fn run_token(base: &str, json: bool, cmd: AuthTokenCmd) -> Result<()> {
    let client = Client::new(base);
    match cmd {
        AuthTokenCmd::Create {
            name,
            agent,
            scopes,
            admin,
            ttl,
        } => {
            let created = client.create_token(&TokenCreate {
                name,
                agent,
                scopes: parse_scopes(&scopes, admin)?,
                ttl_secs: ttl_secs(ttl.as_deref())?,
            })?;
            emit(json, &created, || {
                println!(
                    "created token {} ({}) with scopes {}{}",
                    created.info.name,
                    created.info.id,
                    fmt_scopes(&created.info.scopes),
                    created
                        .info
                        .expires_ms
                        .map(|e| format!(", expires {}", fmt_time(e)))
                        .unwrap_or_default()
                );
                println!("{}", created.token);
                println!("(shown once; pass it as AGIT_TOKEN)");
            });
        }
        AuthTokenCmd::List => {
            let tokens = client.tokens()?;
            emit(json, &tokens, || {
                for t in &tokens {
                    println!(
                        "{}  {:<24} agent={}  scopes={}  expires {}{}",
                        t.id,
                        t.name,
                        t.agent.as_deref().unwrap_or("-"),
                        fmt_scopes(&t.scopes),
                        t.expires_ms.map(fmt_time).unwrap_or_else(|| "never".into()),
                        t.key
                            .as_deref()
                            .map(|k| format!("  key={k}"))
                            .unwrap_or_default()
                    );
                }
            });
        }
        AuthTokenCmd::Revoke { id } => {
            let r = client.delete_token(&id)?;
            emit(json, &r, || println!("revoked token {id}"));
        }
    }
    Ok(())
}
