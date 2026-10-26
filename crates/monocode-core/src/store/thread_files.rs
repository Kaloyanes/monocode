//! Files the app writes on a conversation's behalf, such as pasted images, live
//! in `<data dir>/threads/<session>/` so deleting the conversation removes them.
//! Files the user attached from elsewhere on disk are only referenced, never
//! copied here, and never deleted.
use super::Store;
use crate::transcript::images::{self, ImageDocument};
use std::path::{Path, PathBuf};

const ROOT: &str = "threads";
const ATTACHMENTS: &str = "attachments";
const IMAGES: &str = "images";
const REMOTE_PREFIX: &str = "remote:";
/// Encoded names start with this so they never collide with a plain id.
const ENCODED_PREFIX: &str = "x-";

/// Folder name for a persisted session id. Plain ids are used verbatim; ids
/// with path-unsafe characters, such as remote ids, are hex-encoded.
fn folder_name(id: &str) -> String {
    let plain = !id.is_empty()
        && !id.starts_with('.')
        && !id.starts_with(ENCODED_PREFIX)
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if plain {
        id.to_owned()
    } else {
        let hex: String = id.bytes().map(|b| format!("{b:02x}")).collect();
        format!("{ENCODED_PREFIX}{hex}")
    }
}

fn session_id(folder: &str) -> Option<String> {
    let Some(hex) = folder.strip_prefix(ENCODED_PREFIX) else {
        return Some(folder.to_owned());
    };
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

impl Store {
    fn threads_root(&self) -> Option<PathBuf> {
        self.data_dir().map(|dir| dir.join(ROOT))
    }
    /// The conversation's own folder, or `None` for in-memory stores.
    pub fn thread_dir(&self, id: i64) -> Result<Option<PathBuf>, String> {
        let Some(root) = self.threads_root() else {
            return Ok(None);
        };
        Ok(Some(
            root.join(folder_name(&self.persisted_session_id(id)?)),
        ))
    }
    /// Where pasted images and other app-made attachments for `id` are written.
    pub fn thread_attachments_dir(&self, id: i64) -> Result<Option<PathBuf>, String> {
        Ok(self.thread_dir(id)?.map(|dir| dir.join(ATTACHMENTS)))
    }
    /// Writes an image the agent produced or published into the conversation's
    /// folder, named by content so a repeated image is stored once.
    pub fn save_thread_image(
        &self,
        id: i64,
        bytes: &[u8],
        title: &str,
    ) -> Result<ImageDocument, String> {
        use sha2::{Digest, Sha256};
        let found = images::inspect(bytes)?;
        let dir = match self.thread_dir(id)? {
            Some(dir) => dir.join(IMAGES),
            None => std::env::temp_dir().join("monocode-images"),
        };
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let hash: String = Sha256::digest(bytes)
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let path = dir.join(format!("{hash}.{}", found.extension));
        if !path.exists() {
            // Write beside the target first so a crash never leaves half an image.
            let partial = dir.join(format!(".{hash}.partial"));
            std::fs::write(&partial, bytes)
                .and_then(|_| std::fs::rename(&partial, &path))
                .map_err(|e| e.to_string())?;
        }
        Ok(ImageDocument {
            title: images::title(title),
            path,
            mime_type: found.mime_type.into(),
            width: found.width,
            height: found.height,
        })
    }
    /// Moves files from `staging` into the conversation's attachment folder and
    /// returns the updated paths. Paths outside `staging` belong to the user and
    /// are returned unchanged.
    pub fn adopt_attachments(
        &self,
        id: i64,
        staging: &Path,
        paths: Vec<PathBuf>,
    ) -> Result<Vec<PathBuf>, String> {
        let Some(dir) = self.thread_attachments_dir(id)? else {
            return Ok(paths);
        };
        let mut created = false;
        paths
            .into_iter()
            .map(|path| {
                let (true, Some(name)) = (path.starts_with(staging), path.file_name()) else {
                    return Ok(path);
                };
                if !created {
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    created = true;
                }
                let target = dir.join(name);
                if std::fs::rename(&path, &target).is_err() {
                    // Staging may be on another volume.
                    std::fs::copy(&path, &target).map_err(|e| e.to_string())?;
                    let _ = std::fs::remove_file(&path);
                }
                Ok(target)
            })
            .collect()
    }
    /// Removes folders whose conversation no longer exists, for example after a
    /// crash between deleting the conversation and its folder, or a deletion by
    /// another app sharing this database. Returns how many were removed.
    pub fn sweep_thread_dirs(&self) -> Result<usize, String> {
        if self.shared.is_some() && self.shared_writable().is_err() {
            return Ok(0);
        }
        let Some(root) = self.threads_root() else {
            return Ok(0);
        };
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.to_string()),
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Some(id) = entry.file_name().to_str().and_then(session_id) else {
                continue;
            };
            // Remote sessions are not listed locally; leave them alone.
            if id.starts_with(REMOTE_PREFIX) || self.session_row_exists(&id)? {
                continue;
            }
            if std::fs::remove_dir_all(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
    fn session_row_exists(&self, id: &str) -> Result<bool, String> {
        let (query, key): (&str, &dyn rusqlite::ToSql) = if self.shared.is_some() {
            ("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)", &id)
        } else {
            match id.parse::<i64>() {
                Ok(_) => (
                    "SELECT EXISTS(SELECT 1 FROM native_sessions WHERE id=?1)",
                    &id,
                ),
                // Not a native id; keep the folder rather than guess.
                Err(_) => return Ok(true),
            }
        };
        self.conn
            .query_row(query, [key], |row| row.get(0))
            .map_err(|e| e.to_string())
    }
}

pub(super) fn remove_dir(dir: Option<PathBuf>) {
    if let Some(dir) = dir {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // The startup sweep retries anything left behind.
            Err(e) => eprintln!("Could not remove {}: {e}", dir.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_round_trip_and_stay_inside_the_root() {
        for id in [
            "42",
            "abc-DEF_9",
            "remote:env:7",
            "../etc",
            ".hidden",
            "x-41",
            "a/b",
            "",
        ] {
            let name = folder_name(id);
            assert!(!name.contains('/') && !name.contains('\\') && !name.starts_with('.'));
            assert_eq!(session_id(&name).as_deref(), Some(id), "{id}");
        }
        assert_eq!(folder_name("42"), "42");
    }

    #[test]
    fn deleting_a_session_removes_its_folder_and_sweep_clears_orphans() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open_monocode(&temp.path().join("sessions.db")).unwrap();
        let kept = store.create("claude", "/tmp").unwrap().id;
        let deleted = store.create("claude", "/tmp").unwrap().id;
        let kept_dir = store.thread_attachments_dir(kept).unwrap().unwrap();
        let deleted_dir = store.thread_dir(deleted).unwrap().unwrap();
        std::fs::create_dir_all(&kept_dir).unwrap();
        std::fs::create_dir_all(deleted_dir.join(ATTACHMENTS)).unwrap();
        std::fs::write(deleted_dir.join(ATTACHMENTS).join("pasted.png"), b"png").unwrap();

        store.delete(deleted).unwrap();
        assert!(!deleted_dir.exists());
        assert!(kept_dir.exists());

        let root = temp.path().join(ROOT);
        let orphan = root.join(folder_name("gone"));
        let remote = root.join(folder_name("remote:env:1"));
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::create_dir_all(&remote).unwrap();
        assert_eq!(store.sweep_thread_dirs().unwrap(), 1);
        assert!(!orphan.exists());
        assert!(remote.exists());
        assert!(kept_dir.exists());
    }

    #[test]
    fn images_are_saved_once_inside_the_thread_and_removed_with_it() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open_monocode(&temp.path().join("sessions.db")).unwrap();
        let id = store.create("claude", "/tmp").unwrap().id;
        let png = crate::transcript::images::tests::png(4, 3);
        let first = store.save_thread_image(id, &png, "  Chart ").unwrap();
        let again = store.save_thread_image(id, &png, "").unwrap();
        let dir = store.thread_dir(id).unwrap().unwrap();
        assert_eq!(first.path, again.path);
        assert!(first.path.starts_with(dir.join(IMAGES)));
        assert_eq!(
            (first.title.as_str(), again.title.as_str()),
            ("Chart", "Image")
        );
        assert_eq!((first.width, first.height), (4, 3));
        assert_eq!(std::fs::read(&first.path).unwrap(), png);
        assert!(store.save_thread_image(id, b"text", "x").is_err());

        store.delete(id).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn adopting_moves_only_staged_files() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open_monocode(&temp.path().join("data/sessions.db")).unwrap();
        let id = store.create("claude", "/tmp").unwrap().id;
        let staging = temp.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let staged = staging.join("pasted-1.png");
        let user = temp.path().join("photo.png");
        std::fs::write(&staged, b"a").unwrap();
        std::fs::write(&user, b"b").unwrap();

        let paths = store
            .adopt_attachments(id, &staging, vec![staged.clone(), user.clone()])
            .unwrap();
        let dir = store.thread_attachments_dir(id).unwrap().unwrap();
        assert_eq!(paths, vec![dir.join("pasted-1.png"), user.clone()]);
        assert!(!staged.exists());
        assert!(paths[0].exists() && user.exists());
    }
}
