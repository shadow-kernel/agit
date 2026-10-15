//! Webhooks: deliver repository events to HTTP endpoints.
//!
//! Deliveries run on a dedicated background thread with retries, so emitting
//! an event never waits on the network. Payloads are signed with a BLAKE3
//! keyed hash when the webhook has a secret:
//! `X-Agit-Signature: blake3=<hex(keyed_hash(derive_key("agit webhook v1", secret), body))>`.

use std::fs;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use agit_core::events::{now_ms, Event};
use agit_core::refs::write_atomic;
use agit_core::{Error, Result};
use agit_protocol::types::{Webhook, WebhookCreate};
use serde::{Deserialize, Serialize};

use crate::state::RepoHandle;

pub const KEY_CONTEXT: &str = "agit webhook v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredWebhook {
    id: String,
    url: String,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default = "yes")]
    active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret: Option<String>,
    created_ms: i64,
}

fn yes() -> bool {
    true
}

impl StoredWebhook {
    fn public(&self) -> Webhook {
        Webhook {
            id: self.id.clone(),
            url: self.url.clone(),
            kinds: self.kinds.clone(),
            active: self.active,
            has_secret: self.secret.is_some(),
            created_ms: self.created_ms,
        }
    }

    fn matches(&self, kind: &str) -> bool {
        self.active
            && (self.kinds.is_empty()
                || self.kinds.iter().any(|k| {
                    k == kind
                        || k == "*"
                        || k.strip_suffix('*').is_some_and(|p| kind.starts_with(p))
                        || k.strip_suffix('.')
                            .is_some_and(|p| kind.starts_with(&format!("{p}.")))
                }))
    }
}

pub struct Delivery {
    pub url: String,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

static SENDER: OnceLock<Mutex<Sender<Delivery>>> = OnceLock::new();

fn sender() -> &'static Mutex<Sender<Delivery>> {
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Delivery>();
        std::thread::Builder::new()
            .name("agit-webhooks".into())
            .spawn(move || {
                let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
                for d in rx {
                    let mut backoff = Duration::from_millis(200);
                    for attempt in 1..=3 {
                        let mut req = agent.post(&d.url);
                        for (k, v) in &d.headers {
                            req = req.set(k, v);
                        }
                        match req.send_bytes(&d.body) {
                            Ok(resp) if resp.status() < 300 => break,
                            Ok(resp) => tracing::warn!(url = %d.url, status = resp.status(), attempt, "webhook delivery rejected"),
                            Err(e) => tracing::warn!(url = %d.url, error = %e, attempt, "webhook delivery failed"),
                        }
                        std::thread::sleep(backoff);
                        backoff *= 4;
                    }
                }
            })
            .expect("spawn webhook thread");
        Mutex::new(tx)
    })
}

pub fn sign(secret: &str, body: &[u8]) -> String {
    let key = blake3::derive_key(KEY_CONTEXT, secret.as_bytes());
    format!("blake3={}", blake3::keyed_hash(&key, body).to_hex())
}

pub struct WebhookStore<'a> {
    repo: &'a RepoHandle,
}

/// Process-wide cache of webhook lists keyed by file path, invalidated by
/// the file's mtime, so emitting an event does not re-read the file.
type HookCache = std::collections::HashMap<
    std::path::PathBuf,
    (Option<std::time::SystemTime>, Vec<StoredWebhook>),
>;
static CACHE: OnceLock<Mutex<HookCache>> = OnceLock::new();

impl<'a> WebhookStore<'a> {
    pub fn new(repo: &'a RepoHandle) -> Self {
        WebhookStore { repo }
    }

    fn path(&self) -> std::path::PathBuf {
        self.repo.dir.join("webhooks.json")
    }

    fn load(&self) -> Result<Vec<StoredWebhook>> {
        match fs::read(self.path()) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, v: &[StoredWebhook]) -> Result<()> {
        write_atomic(&self.path(), &serde_json::to_vec_pretty(v)?)
    }

    pub fn list(&self) -> Result<Vec<Webhook>> {
        Ok(self.load()?.iter().map(StoredWebhook::public).collect())
    }

    pub fn create(&self, req: WebhookCreate, actor: &str) -> Result<Webhook> {
        if !(req.url.starts_with("http://") || req.url.starts_with("https://")) {
            return Err(Error::Invalid("webhook url must be http(s)".into()));
        }
        let _g = self.repo.write_lock.lock().unwrap();
        let mut all = self.load()?;
        let w = StoredWebhook {
            id: format!("wh_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
            url: req.url,
            kinds: req.kinds,
            active: true,
            secret: req.secret.filter(|s| !s.is_empty()),
            created_ms: now_ms(),
        };
        all.push(w.clone());
        self.save(&all)?;
        self.repo.emit(
            "webhook.create",
            Some(actor),
            serde_json::json!({ "id": w.id, "url": w.url, "kinds": w.kinds }),
        )?;
        Ok(w.public())
    }

    pub fn delete(&self, id: &str, actor: &str) -> Result<()> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut all = self.load()?;
        let before = all.len();
        all.retain(|w| w.id != id);
        if all.len() == before {
            return Err(Error::NotFound(format!("webhook {id}")));
        }
        self.save(&all)?;
        self.repo.emit(
            "webhook.delete",
            Some(actor),
            serde_json::json!({ "id": id }),
        )?;
        Ok(())
    }

    /// Queue deliveries of `event` to every matching webhook.
    fn load_cached(&self) -> Vec<StoredWebhook> {
        let path = self.path();
        let mtime = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
        let cache = CACHE.get_or_init(|| Mutex::new(Default::default()));
        let mut map = cache.lock().unwrap();
        if let Some((seen, hooks)) = map.get(&path) {
            if *seen == mtime {
                return hooks.clone();
            }
        }
        let hooks = self.load().unwrap_or_default();
        map.insert(path, (mtime, hooks.clone()));
        hooks
    }

    pub fn dispatch(&self, event: &Event) {
        let hooks = self.load_cached();
        if hooks.is_empty() {
            return;
        }
        let matching: Vec<&StoredWebhook> =
            hooks.iter().filter(|w| w.matches(&event.kind)).collect();
        if matching.is_empty() {
            return;
        }
        let body = match serde_json::to_vec(
            &serde_json::json!({ "repo": self.repo.meta.name, "event": event }),
        ) {
            Ok(b) => b,
            Err(_) => return,
        };
        for w in matching {
            let mut headers = vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                (
                    "User-Agent".to_string(),
                    format!("agit-server/{}", env!("CARGO_PKG_VERSION")),
                ),
                ("X-Agit-Event".to_string(), event.kind.clone()),
                ("X-Agit-Repo".to_string(), self.repo.meta.name.clone()),
                (
                    "X-Agit-Delivery".to_string(),
                    uuid::Uuid::new_v4().simple().to_string(),
                ),
                ("X-Agit-Webhook".to_string(), w.id.clone()),
            ];
            if let Some(secret) = &w.secret {
                headers.push(("X-Agit-Signature".to_string(), sign(secret, &body)));
            }
            let _ = sender().lock().unwrap().send(Delivery {
                url: w.url.clone(),
                body: body.clone(),
                headers,
            });
        }
    }

    /// Send a synthetic `ping` event to one webhook.
    pub fn test(&self, id: &str) -> Result<()> {
        let ev = Event {
            seq: 0,
            time_ms: now_ms(),
            kind: "ping".into(),
            workspace: None,
            actor: None,
            data: serde_json::json!({ "webhook": id }),
        };
        let hooks = self.load()?;
        let w = hooks
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| Error::NotFound(format!("webhook {id}")))?;
        let body =
            serde_json::to_vec(&serde_json::json!({ "repo": self.repo.meta.name, "event": ev }))?;
        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("X-Agit-Event".to_string(), "ping".to_string()),
            ("X-Agit-Repo".to_string(), self.repo.meta.name.clone()),
            (
                "X-Agit-Delivery".to_string(),
                uuid::Uuid::new_v4().simple().to_string(),
            ),
            ("X-Agit-Webhook".to_string(), w.id.clone()),
        ];
        if let Some(secret) = &w.secret {
            headers.push(("X-Agit-Signature".to_string(), sign(secret, &body)));
        }
        let _ = sender().lock().unwrap().send(Delivery {
            url: w.url.clone(),
            body,
            headers,
        });
        Ok(())
    }
}
