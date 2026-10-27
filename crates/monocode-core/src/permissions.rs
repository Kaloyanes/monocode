//! Exact MCP confirmation grants. Reload under one lock so revocations reach live turns.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

static WRITER: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub project: PathBuf,
    pub provider: String,
    pub account: String,
    pub account_home: PathBuf,
    pub binary: PathBuf,
    pub server: String,
    pub confirmation: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub key: Key,
    pub label: String,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    grants: Vec<Grant>,
}

#[derive(Clone)]
pub struct Store {
    path: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self::new(crate::store::monocode_database().with_file_name("permissions.json"))
    }
}

impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read(&self) -> Result<Vec<Grant>, String> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("Cannot read saved permissions: {e}")),
        };
        let file: File = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Cannot read saved permissions: {e}"))?;
        if file.version != 1 {
            return Err("Unsupported saved permissions version".into());
        }
        let mut grants: Vec<Grant> = Vec::with_capacity(file.grants.len());
        for mut grant in file.grants {
            stable(&mut grant.key.confirmation);
            if !grants.iter().any(|g| g.key == grant.key) {
                grants.push(grant);
            }
        }
        Ok(grants)
    }

    pub fn list(&self) -> Result<Vec<Grant>, String> {
        let _lock = WRITER
            .lock()
            .map_err(|_| "Saved permissions are unavailable")?;
        self.read()
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.list()
            .is_ok_and(|grants| grants.iter().any(|g| &g.key == key))
    }

    pub fn allow(&self, grant: Grant) -> Result<(), String> {
        self.update(|grants| {
            if !grants.iter().any(|g| g.key == grant.key) {
                grants.push(grant);
            }
        })
    }

    pub fn remove(&self, key: &Key) -> Result<(), String> {
        self.update(|grants| grants.retain(|g| &g.key != key))
    }

    fn update(&self, change: impl FnOnce(&mut Vec<Grant>)) -> Result<(), String> {
        let _lock = WRITER
            .lock()
            .map_err(|_| "Saved permissions are unavailable")?;
        let mut grants = self.read()?;
        change(&mut grants);
        let parent = self
            .path
            .parent()
            .ok_or("Saved permissions path has no parent")?;
        fs::create_dir_all(parent).map_err(|e| format!("Cannot save permission: {e}"))?;
        let mut file = tempfile::NamedTempFile::new_in(parent)
            .map_err(|e| format!("Cannot save permission: {e}"))?;
        serde_json::to_writer_pretty(&mut file, &File { version: 1, grants })
            .map_err(|e| format!("Cannot save permission: {e}"))?;
        file.as_file()
            .sync_all()
            .map_err(|e| format!("Cannot save permission: {e}"))?;
        file.persist(&self.path)
            .map_err(|e| format!("Cannot save permission: {e}"))?;
        Ok(())
    }
}

/// Drop the per-call fields Codex attaches to `_meta`, so the same confirmation
/// in a later turn or chat produces the same key.
pub fn stable(confirmation: &mut Value) {
    if let Some(meta) = confirmation.get_mut("_meta").and_then(Value::as_object_mut) {
        for field in ["callId", "x-codex-turn-metadata"] {
            meta.remove(field);
        }
    }
}

pub fn project(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn grant() -> Grant {
        Grant {
            label: "Allow Computer Use to use Monocode Native?".into(),
            key: Key {
                project: "/project".into(),
                provider: "codex".into(),
                account: "default".into(),
                account_home: "/codex".into(),
                binary: "/bin/codex".into(),
                server: "cua_repl".into(),
                confirmation: json!({"message":"Allow this app?","mode":"form","requestedSchema":{"type":"object","properties":{}}}),
            },
        }
    }

    #[test]
    fn per_call_metadata_does_not_change_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("permissions.json"));
        let mut first = json!({"message":"m","_meta":{"callId":"a","tool_name":"t","x-codex-turn-metadata":{"turn_id":"1"}}});
        let mut second = json!({"message":"m","_meta":{"callId":"b","tool_name":"t","x-codex-turn-metadata":{"turn_id":"2"}}});
        stable(&mut first);
        stable(&mut second);
        assert_eq!(first, second);
        let mut stored = grant();
        stored.key.confirmation = json!({"_meta":{"callId":"old","tool_name":"t"}});
        let mut duplicate = stored.clone();
        duplicate.key.confirmation = json!({"_meta":{"callId":"older","tool_name":"t"}});
        let raw = File {
            version: 1,
            grants: vec![stored, duplicate],
        };
        std::fs::write(
            dir.path().join("permissions.json"),
            serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap();
        let grants = store.list().unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(
            grants[0].key.confirmation,
            json!({"_meta":{"tool_name":"t"}})
        );
    }

    #[test]
    fn persists_exact_grants_and_revokes_across_store_instances() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("permissions.json");
        let store = Store::new(path.clone());
        let grant = grant();
        store.allow(grant.clone()).unwrap();
        store.allow(grant.clone()).unwrap();
        let reopened = Store::new(path);
        assert!(reopened.contains(&grant.key));
        assert_eq!(reopened.list().unwrap(), vec![grant.clone()]);
        for field in [
            "project",
            "provider",
            "account",
            "account_home",
            "binary",
            "server",
            "confirmation",
        ] {
            let mut changed = serde_json::to_value(&grant.key).unwrap();
            changed[field] = json!("different");
            let changed = serde_json::from_value(changed).unwrap();
            assert!(!reopened.contains(&changed), "{field} must match");
        }
        reopened.remove(&grant.key).unwrap();
        assert!(!store.contains(&grant.key));
    }

    #[test]
    fn malformed_or_future_files_never_grant_or_get_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("permissions.json");
        let store = Store::new(path.clone());
        for bytes in ["broken", r#"{"version":2,"grants":[]}"#] {
            fs::write(&path, bytes).unwrap();
            assert!(!store.contains(&grant().key));
            assert!(store.allow(grant()).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn concurrent_writers_preserve_both_grants() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("permissions.json"));
        std::thread::scope(|scope| {
            for server in ["first", "second"] {
                let store = store.clone();
                scope.spawn(move || {
                    let mut grant = grant();
                    grant.key.server = server.into();
                    store.allow(grant).unwrap();
                });
            }
        });
        assert_eq!(store.list().unwrap().len(), 2);
    }
}
