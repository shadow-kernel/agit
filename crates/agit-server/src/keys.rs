//! Registry of agent signing keys per repository.

use std::fs;

use agit_core::events::now_ms;
use agit_core::refs::write_atomic;
use agit_core::sign::{validate_public_key, verify_commit, SignatureStatus};
use agit_core::{Commit, Error, Result};
use agit_protocol::types::{KeyEntry, SignatureInfo};

use crate::state::RepoHandle;

pub struct KeyStore<'a> {
    repo: &'a RepoHandle,
}

impl<'a> KeyStore<'a> {
    pub fn new(repo: &'a RepoHandle) -> Self {
        KeyStore { repo }
    }

    fn path(&self) -> std::path::PathBuf {
        self.repo.dir.join("keys.json")
    }

    pub fn list(&self) -> Result<Vec<KeyEntry>> {
        match fs::read(self.path()) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    pub fn add(&self, agent: &str, public_key: &str, actor: &str) -> Result<KeyEntry> {
        if agent.trim().is_empty() {
            return Err(Error::Invalid("agent is required".into()));
        }
        let key_id = validate_public_key(public_key)?;
        let _g = self.repo.write_lock.lock().unwrap();
        let mut all = self.list()?;
        if let Some(existing) = all.iter().find(|k| k.key_id == key_id) {
            if existing.agent == agent {
                return Ok(existing.clone());
            }
            return Err(Error::AlreadyExists(
                format!("key {key_id} is registered for {}", existing.agent).into(),
            ));
        }
        let entry = KeyEntry {
            agent: agent.to_string(),
            key_id: key_id.clone(),
            public_key: public_key.trim().to_lowercase(),
            added_ms: now_ms(),
            added_by: actor.to_string(),
        };
        all.push(entry.clone());
        write_atomic(&self.path(), &serde_json::to_vec_pretty(&all)?)?;
        self.repo.emit(
            "key.add",
            Some(actor),
            serde_json::json!({ "agent": agent, "key_id": key_id }),
        )?;
        Ok(entry)
    }

    pub fn remove(&self, key_id: &str, actor: &str) -> Result<()> {
        let _g = self.repo.write_lock.lock().unwrap();
        let mut all = self.list()?;
        let before = all.len();
        all.retain(|k| k.key_id != key_id);
        if all.len() == before {
            return Err(Error::NotFound(format!("key {key_id}")));
        }
        write_atomic(&self.path(), &serde_json::to_vec_pretty(&all)?)?;
        self.repo.emit(
            "key.remove",
            Some(actor),
            serde_json::json!({ "key_id": key_id }),
        )?;
        Ok(())
    }

    /// Is `key_id` registered for `agent`?
    pub fn is_trusted(&self, agent: &str, key_id: &str) -> Result<bool> {
        Ok(self
            .list()?
            .iter()
            .any(|k| k.agent == agent && k.key_id == key_id))
    }

    /// Verify a commit and look its key up in the registry.
    pub fn info(&self, commit: &Commit) -> Result<SignatureInfo> {
        let status = verify_commit(commit);
        let trusted = match &status {
            SignatureStatus::Valid { key_id, signer, .. } => self.is_trusted(signer, key_id)?,
            _ => false,
        };
        Ok(SignatureInfo { status, trusted })
    }
}
