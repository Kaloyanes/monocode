//! Settings worktree guards; removal keeps the branch and commits.
use std::path::{Path, PathBuf};
pub fn default_root(main: &Path) -> PathBuf {
    main.with_file_name(format!(
        "{}-worktrees",
        main.file_name().unwrap_or_default().to_string_lossy()
    ))
}
pub fn status(cwd: &Path, tree: &crate::git::Worktree) -> (bool, Option<bool>, usize) {
    let locked = crate::git::git(cwd, &["worktree", "list", "--porcelain", "-z"])
        .ok()
        .is_some_and(|s| {
            s.split("\0\0").any(|block| {
                block.starts_with(&format!("worktree {}\0", tree.path))
                    && block
                        .split('\0')
                        .any(|v| v == "locked" || v.starts_with("locked "))
            })
        });
    let dirty = crate::git::git(
        Path::new(&tree.path),
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .ok()
    .map(|s| !s.is_empty());
    let unpushed = crate::git::git(
        Path::new(&tree.path),
        &["rev-list", "--count", "HEAD", "--not", "--remotes"],
    )
    .ok()
    .and_then(|s| s.trim().parse().ok())
    .unwrap_or(0);
    (locked, dirty, unpushed)
}
pub fn check(cwd: &Path, path: &Path) -> Result<crate::git::Worktree, String> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let key = crate::files::path_key(&canonical.to_string_lossy());
    let tree = crate::git::snapshot(cwd)?
        .worktrees
        .into_iter()
        .find(|t| {
            let canonical = Path::new(&t.path)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&t.path));
            crate::files::path_key(&canonical.to_string_lossy()) == key
        })
        .ok_or("This path is not a registered worktree of this repository")?;
    if tree.main {
        return Err("The main working copy cannot be deleted".into());
    }
    if tree.branch.is_none() {
        return Err("Create a branch for this detached worktree before deleting it.".into());
    }
    if status(cwd, &tree).0 {
        return Err("This worktree is locked. Unlock it in Git before deleting it.".into());
    }
    Ok(tree)
}
pub fn remove(cwd: &Path, path: &Path) -> Result<(), String> {
    let tree = check(cwd, path)?;
    crate::provider::discard_worktree(Path::new(&tree.path));
    crate::git::git(cwd, &["worktree", "remove", "--force", "--", &tree.path]).map(|_| ())
}

/// Why a worktree is not offered for cleanup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocker {
    Main,
    Missing,
    Detached,
    Locked,
    Dirty,
    /// Git could not say; unknown is never treated as safe.
    Unknown,
    Unpushed,
}
impl Blocker {
    pub fn message(self) -> &'static str {
        match self {
            Self::Main => "The main working copy cannot be deleted",
            Self::Missing => "The folder is missing",
            Self::Detached => "Create a branch for this detached worktree before deleting it.",
            Self::Locked => "This worktree is locked. Unlock it in Git before deleting it.",
            Self::Dirty => "This worktree has uncommitted changes",
            Self::Unknown => "Git could not report this worktree's state",
            Self::Unpushed => "This worktree has commits that are not merged or pushed",
        }
    }
}
/// Whether cleanup may offer to delete a worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Safety {
    /// Clean, and every commit is already in the base branch.
    Merged,
    /// Clean, and every commit is already on a remote.
    Pushed,
    Blocked(Blocker),
}
/// What Git says about one worktree. `safety` is the only decision made from it.
#[derive(Clone, Debug)]
pub struct TreeStatus {
    pub tree: crate::git::Worktree,
    pub locked: bool,
    /// Uncommitted or untracked changes; `None` when Git could not tell.
    pub dirty: Option<bool>,
    /// Commits on no remote branch; `None` when Git could not tell.
    pub unpushed: Option<usize>,
    /// Commits missing from the base branch; `None` without a base or on error.
    pub unmerged: Option<usize>,
    pub has_upstream: bool,
    /// Unix seconds of the checked-out commit.
    pub last_commit: Option<i64>,
}
impl TreeStatus {
    pub fn merged(&self) -> Option<bool> {
        self.unmerged.map(|n| n == 0)
    }
    pub fn safety(&self) -> Safety {
        if self.tree.main {
            return Safety::Blocked(Blocker::Main);
        }
        if self.tree.missing {
            return Safety::Blocked(Blocker::Missing);
        }
        if self.tree.branch.is_none() {
            return Safety::Blocked(Blocker::Detached);
        }
        if self.locked {
            return Safety::Blocked(Blocker::Locked);
        }
        match self.dirty {
            Some(false) => {}
            Some(true) => return Safety::Blocked(Blocker::Dirty),
            None => return Safety::Blocked(Blocker::Unknown),
        }
        if self.unmerged == Some(0) {
            Safety::Merged
        } else if self.unpushed == Some(0) {
            Safety::Pushed
        } else if self.unpushed.is_none() {
            Safety::Blocked(Blocker::Unknown)
        } else {
            Safety::Blocked(Blocker::Unpushed)
        }
    }
}

fn ref_exists(cwd: &Path, name: &str) -> bool {
    crate::git::git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{name}^{{commit}}"),
        ],
    )
    .is_ok()
}
/// The branch merges are judged against: the remote's default branch, else
/// `main`/`master`, else the main checkout's branch.
pub fn detect_base(main: &Path) -> Option<String> {
    let local = |b: &str| ref_exists(main, &format!("refs/heads/{b}")).then(|| b.to_owned());
    let remote_default = || {
        let head = crate::git::git(
            main,
            &[
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
        )
        .ok()?;
        let head = head.trim();
        local(head.strip_prefix("origin/")?).or_else(|| Some(head.to_owned()))
    };
    remote_default()
        .or_else(|| local("main"))
        .or_else(|| local("master"))
        .or_else(|| crate::git::worktree_branch(main))
}

fn count(cwd: &Path, args: &[&str]) -> Option<usize> {
    crate::git::git(cwd, args).ok()?.trim().parse().ok()
}
fn inspect(cwd: &Path, tree: crate::git::Worktree, base: Option<&str>) -> TreeStatus {
    let mut out = TreeStatus {
        locked: false,
        dirty: None,
        unpushed: None,
        unmerged: None,
        has_upstream: false,
        last_commit: None,
        tree,
    };
    if out.tree.missing {
        return out;
    }
    let path = PathBuf::from(&out.tree.path);
    (out.locked, out.dirty, _) = status(cwd, &out.tree);
    out.unpushed = count(
        &path,
        &["rev-list", "--count", "HEAD", "--not", "--remotes"],
    );
    out.unmerged = base.and_then(|base| {
        // A base branch also counts what its own upstream already has.
        let upstream = format!("{base}@{{upstream}}");
        let mut args = vec!["rev-list", "--count", "HEAD", "--not", base];
        if ref_exists(&path, &upstream) {
            args.push(&upstream);
        }
        count(&path, &args)
    });
    out.has_upstream = out.tree.branch.as_ref().is_some_and(|b| {
        crate::git::git(
            &path,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{b}@{{upstream}}"),
            ],
        )
        .is_ok()
    });
    out.last_commit = crate::git::git(&path, &["log", "-1", "--format=%ct", "HEAD"])
        .ok()
        .and_then(|s| s.trim().parse().ok());
    out
}
/// Every worktree of the repository, main checkout first, with its status.
/// `base` overrides [`detect_base`].
pub fn sweep(cwd: &Path, base: Option<&str>) -> Result<(Option<String>, Vec<TreeStatus>), String> {
    let trees = crate::git::worktrees(cwd)?;
    let main = trees.first().ok_or("No working copies found")?;
    let base = base
        .map(str::to_owned)
        .or_else(|| detect_base(Path::new(&main.path)));
    let list = trees
        .into_iter()
        .map(|tree| inspect(cwd, tree, base.as_deref()))
        .collect();
    Ok((base, list))
}
/// "3d ago", "Oct 12" or "just now" for a commit time in unix seconds.
pub fn age_label(unix: i64, now_ms: u64) -> String {
    let at = u64::try_from(unix).unwrap_or(0).saturating_mul(1000);
    match crate::time::relative(at, now_ms).as_str() {
        "" => String::new(),
        "now" => "just now".into(),
        r if r.ends_with(|c: char| c == 'm' || c == 'h' || c == 'd') => format!("{r} ago"),
        r => r.into(),
    }
}
/// Removes a worktree only if it is still clean and merged or pushed right now.
/// Unlike [`remove`], this never passes `--force`, so Git itself also refuses
/// when files changed after the check. The branch and commits are kept.
pub fn remove_if_safe(cwd: &Path, path: &Path) -> Result<(), String> {
    let tree = check(cwd, path)?;
    let main = crate::git::worktrees(cwd)?
        .into_iter()
        .next()
        .ok_or("No working copies found")?;
    let base = detect_base(Path::new(&main.path));
    let status = inspect(cwd, tree, base.as_deref());
    if let Safety::Blocked(why) = status.safety() {
        return Err(why.message().into());
    }
    crate::provider::discard_worktree(Path::new(&status.tree.path));
    crate::git::git(cwd, &["worktree", "remove", "--", &status.tree.path]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| crate::git::git(dir.path(), args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        git(&["config", "user.name", "Fixture"]);
        std::fs::write(dir.path().join("file"), "before").unwrap();
        git(&["add", "file"]);
        git(&["commit", "-m", "Initial"]);
        dir
    }
    #[test]
    fn removal_guards_and_branch_retention() {
        let dir = repo();
        assert!(check(dir.path(), dir.path()).unwrap_err().contains("main"));
        let tree = crate::git::create_named_worktree(dir.path(), "settings-parity").unwrap();
        let path = Path::new(&tree.path);
        std::fs::write(path.join("untracked"), "discard in disposable test").unwrap();
        assert_eq!(status(dir.path(), &tree).1, Some(true));
        crate::git::git(dir.path(), &["worktree", "lock", "--", &tree.path]).unwrap();
        assert!(check(dir.path(), path).unwrap_err().contains("locked"));
        crate::git::git(dir.path(), &["worktree", "unlock", "--", &tree.path]).unwrap();
        remove(dir.path(), path).unwrap();
        assert!(!path.exists());
        assert!(crate::git::git(
            dir.path(),
            &["show-ref", "--verify", "refs/heads/settings-parity"]
        )
        .is_ok());
        let _ = std::fs::remove_dir(default_root(dir.path()));
    }
    #[test]
    fn detached_commits_cannot_be_orphaned() {
        let dir = repo();
        let path = dir.path().join("detached");
        crate::git::git(
            dir.path(),
            &[
                "worktree",
                "add",
                "--detach",
                path.to_str().unwrap(),
                "HEAD",
            ],
        )
        .unwrap();
        assert!(remove(dir.path(), &path).unwrap_err().contains("detached"));
        crate::git::git(
            dir.path(),
            &["worktree", "remove", "--force", path.to_str().unwrap()],
        )
        .unwrap();
    }

    struct Fixture {
        _root: tempfile::TempDir,
        main: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let main = root.path().join("project");
            std::fs::create_dir(&main).unwrap();
            let git = |args: &[&str]| crate::git::git(&main, args).unwrap();
            git(&["init", "-b", "main"]);
            git(&["config", "user.email", "fixture@example.invalid"]);
            git(&["config", "user.name", "Fixture"]);
            std::fs::write(main.join("file"), "before").unwrap();
            git(&["add", "file"]);
            git(&["commit", "-m", "Initial"]);
            Self { _root: root, main }
        }
        fn git(&self, args: &[&str]) -> String {
            crate::git::git(&self.main, args).unwrap()
        }
        fn tree(&self, branch: &str) -> PathBuf {
            let path = self._root.path().join("project-worktrees").join(branch);
            self.git(&[
                "worktree",
                "add",
                "-b",
                branch,
                path.to_str().unwrap(),
                "main",
            ]);
            path
        }
        fn commit(path: &Path, name: &str) {
            std::fs::write(path.join(name), name).unwrap();
            crate::git::git(path, &["add", name]).unwrap();
            crate::git::git(path, &["commit", "-m", name]).unwrap();
        }
        fn status(&self, path: &Path) -> TreeStatus {
            let key = path.canonicalize().unwrap();
            sweep(&self.main, None)
                .unwrap()
                .1
                .into_iter()
                .find(|t| Path::new(&t.tree.path).canonicalize().unwrap() == key)
                .unwrap()
        }
        /// A bare remote that `main` and everything pushed later lands on.
        fn remote(&self) {
            let bare = self._root.path().join("remote.git");
            crate::git::git(
                self._root.path(),
                &["init", "--bare", "-b", "main", "remote.git"],
            )
            .unwrap();
            self.git(&["remote", "add", "origin", bare.to_str().unwrap()]);
            self.git(&["push", "-u", "origin", "main"]);
        }
    }
    fn fake(tree: crate::git::Worktree) -> TreeStatus {
        TreeStatus {
            tree,
            locked: false,
            dirty: Some(false),
            unpushed: Some(0),
            unmerged: Some(0),
            has_upstream: false,
            last_commit: None,
        }
    }
    fn linked() -> crate::git::Worktree {
        crate::git::Worktree {
            path: "/x".into(),
            branch: Some("b".into()),
            head: "abc".into(),
            main: false,
            missing: false,
        }
    }

    #[test]
    fn verdict_never_treats_unknown_or_risky_state_as_safe() {
        assert_eq!(fake(linked()).safety(), Safety::Merged);
        let blocked = |edit: &dyn Fn(&mut TreeStatus)| {
            let mut s = fake(linked());
            edit(&mut s);
            s.safety()
        };
        use Blocker::*;
        assert_eq!(blocked(&|s| s.tree.main = true), Safety::Blocked(Main));
        assert_eq!(
            blocked(&|s| s.tree.missing = true),
            Safety::Blocked(Missing)
        );
        assert_eq!(
            blocked(&|s| s.tree.branch = None),
            Safety::Blocked(Detached)
        );
        assert_eq!(blocked(&|s| s.locked = true), Safety::Blocked(Locked));
        assert_eq!(blocked(&|s| s.dirty = Some(true)), Safety::Blocked(Dirty));
        assert_eq!(blocked(&|s| s.dirty = None), Safety::Blocked(Unknown));
        // Merged wins even if the push count is unknown; otherwise unknown blocks.
        assert_eq!(blocked(&|s| s.unpushed = None), Safety::Merged);
        assert_eq!(
            blocked(&|s| (s.unmerged, s.unpushed) = (None, None)),
            Safety::Blocked(Unknown)
        );
        assert_eq!(
            blocked(&|s| (s.unmerged, s.unpushed) = (Some(2), Some(2))),
            Safety::Blocked(Unpushed)
        );
        assert_eq!(
            blocked(&|s| (s.unmerged, s.unpushed) = (Some(2), Some(0))),
            Safety::Pushed
        );
        assert_eq!(
            blocked(&|s| (s.unmerged, s.unpushed, s.dirty) = (Some(0), Some(0), Some(true))),
            Safety::Blocked(Dirty)
        );
    }
    #[test]
    fn age_label_reads_naturally() {
        let now = 1_700_000_000_000u64;
        let ago = |secs: i64| age_label(1_700_000_000 - secs, now);
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(3 * 3600), "3h ago");
        assert_eq!(ago(2 * 86_400), "2d ago");
        assert_eq!(ago(30 * 86_400), "Oct 15");
        assert_eq!(age_label(0, now), "");
    }
    #[test]
    fn base_prefers_the_remote_default_then_main() {
        let f = Fixture::new();
        assert_eq!(detect_base(&f.main).as_deref(), Some("main"));
        f.git(&["branch", "trunk"]);
        f.git(&["update-ref", "refs/remotes/origin/trunk", "trunk"]);
        f.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ]);
        assert_eq!(detect_base(&f.main).as_deref(), Some("trunk"));
    }
    #[test]
    fn main_checkout_is_listed_but_never_removable() {
        let f = Fixture::new();
        let list = sweep(&f.main, None).unwrap().1;
        assert_eq!(list.len(), 1);
        assert!(list[0].tree.main);
        assert_eq!(list[0].safety(), Safety::Blocked(Blocker::Main));
        assert!(remove_if_safe(&f.main, &f.main)
            .unwrap_err()
            .contains("main"));
        assert!(f.main.join("file").exists());
    }
    #[test]
    fn clean_merged_worktree_is_removed_and_keeps_its_branch() {
        let f = Fixture::new();
        let path = f.tree("done");
        Fixture::commit(&path, "work");
        let before = f.status(&path);
        assert_eq!(before.merged(), Some(false));
        assert_eq!(before.safety(), Safety::Blocked(Blocker::Unpushed));
        f.git(&["merge", "--no-ff", "-m", "merge", "done"]);
        let merged = f.status(&path);
        assert_eq!(merged.merged(), Some(true));
        assert_eq!(merged.safety(), Safety::Merged);
        assert!(!merged.has_upstream);
        assert!(merged.last_commit.is_some_and(|t| t > 1_600_000_000));
        remove_if_safe(&f.main, &path).unwrap();
        assert!(!path.exists());
        f.git(&["show-ref", "--verify", "refs/heads/done"]);
    }
    #[test]
    fn unique_unpushed_commits_and_uncommitted_changes_block_removal() {
        let f = Fixture::new();
        let ahead = f.tree("ahead");
        Fixture::commit(&ahead, "unique");
        let err = remove_if_safe(&f.main, &ahead).unwrap_err();
        assert!(err.contains("not merged or pushed"), "{err}");
        assert!(ahead.exists());

        let dirty = f.tree("dirty");
        std::fs::write(dirty.join("untracked"), "x").unwrap();
        assert_eq!(f.status(&dirty).safety(), Safety::Blocked(Blocker::Dirty));
        assert!(remove_if_safe(&f.main, &dirty)
            .unwrap_err()
            .contains("uncommitted"));
        std::fs::remove_file(dirty.join("untracked")).unwrap();
        std::fs::write(dirty.join("file"), "edited").unwrap();
        assert_eq!(f.status(&dirty).safety(), Safety::Blocked(Blocker::Dirty));
        assert!(dirty.join("file").exists());
    }
    #[test]
    fn a_change_after_the_listing_still_stops_removal() {
        let f = Fixture::new();
        let path = f.tree("late");
        assert_eq!(f.status(&path).safety(), Safety::Merged);
        std::fs::write(path.join("late-edit"), "x").unwrap();
        assert!(remove_if_safe(&f.main, &path).is_err());
        assert!(path.join("late-edit").exists());
    }
    #[test]
    fn fully_pushed_branch_without_merge_is_safe_and_reports_its_upstream() {
        let f = Fixture::new();
        f.remote();
        let path = f.tree("pushed");
        Fixture::commit(&path, "feature");
        let local = f.status(&path);
        assert!(!local.has_upstream);
        assert_eq!(local.unpushed, Some(1));
        assert_eq!(local.safety(), Safety::Blocked(Blocker::Unpushed));
        crate::git::git(&path, &["push", "-u", "origin", "pushed"]).unwrap();
        let pushed = f.status(&path);
        assert!(pushed.has_upstream);
        assert_eq!(pushed.merged(), Some(false));
        assert_eq!(pushed.safety(), Safety::Pushed);
        crate::git::git(&path, &["commit", "--allow-empty", "-m", "later"]).unwrap();
        assert_eq!(f.status(&path).safety(), Safety::Blocked(Blocker::Unpushed));
    }
    #[test]
    fn locked_and_detached_worktrees_are_blocked() {
        let f = Fixture::new();
        let locked = f.tree("locked");
        f.git(&["worktree", "lock", "--", locked.to_str().unwrap()]);
        assert_eq!(f.status(&locked).safety(), Safety::Blocked(Blocker::Locked));
        assert!(remove_if_safe(&f.main, &locked).is_err());
        let detached = f._root.path().join("detached");
        f.git(&[
            "worktree",
            "add",
            "--detach",
            detached.to_str().unwrap(),
            "main",
        ]);
        assert_eq!(
            f.status(&detached).safety(),
            Safety::Blocked(Blocker::Detached)
        );
        assert!(remove_if_safe(&f.main, &detached).is_err());
        assert!(detached.exists());
    }
}
