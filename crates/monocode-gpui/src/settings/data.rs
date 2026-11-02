//! Settings discovery runs on a worker; rendering never probes CLIs or Git.
use monocode_core::{
    git,
    provider::{self, Kind},
};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug)]
pub(super) struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub scope: String,
    pub provider: String,
}
#[derive(Clone, Debug)]
pub(super) struct McpServer {
    pub name: String,
    pub provider: String,
    pub scope: String,
    pub path: PathBuf,
    pub transport: String,
    pub status: String,
}
#[derive(Default)]
pub(super) struct Discovery {
    pub binaries: Vec<(Kind, Option<PathBuf>)>,
    pub skills: Vec<Skill>,
    pub mcp: Vec<McpServer>,
    pub mcp_error: Option<String>,
    pub worktrees: ResultTrees,
}
#[derive(Clone, Debug)]
pub(super) struct Tree {
    pub tree: git::Worktree,
    pub locked: bool,
    pub dirty: Option<bool>,
    pub unpushed: usize,
}
#[derive(Default)]
pub(super) struct ResultTrees {
    pub trees: Vec<Tree>,
    pub branches: Vec<git::Branch>,
    pub default_root: PathBuf,
    pub main: String,
    pub error: Option<String>,
    /// Every worktree, main first, with what cleanup needs to judge it.
    pub sweep: Vec<monocode_core::worktree_management::TreeStatus>,
    pub base: Option<String>,
}
pub(super) fn discover(cwd: &str) -> Discovery {
    let mut data = Discovery {
        binaries: Kind::ALL
            .into_iter()
            .map(|k| (k, provider::resolve(k)))
            .collect(),
        ..Default::default()
    };
    data.skills = monocode_core::skills::list_skills(cwd.into(), None)
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            let path = PathBuf::from(s.path);

            Skill {
                name: s.name,
                description: s.description,
                path,
                scope: s.scope,
                provider: s.source,
            }
        })
        .collect();
    match monocode_core::mcp::mcp_discover(cwd.into()) {
        Ok(servers) => {
            data.mcp = servers
                .into_iter()
                .map(|s| McpServer {
                    name: s.name,
                    provider: s.provider,
                    scope: s.scope,
                    path: s.config_path.into(),
                    transport: s.transport,
                    status: if s.enabled { "Configured" } else { "Disabled" }.into(),
                })
                .collect()
        }
        Err(e) => data.mcp_error = Some(e),
    }
    data
}

/// The worktrees of the project at `cwd`. Git runs here, on a worker, apart
/// from `discover` so the Worktrees page never waits on skills or MCP.
pub(super) fn discover_worktrees(cwd: &str) -> ResultTrees {
    let mut out = ResultTrees::default();
    if cwd.is_empty() {
        return out;
    }
    let snapshot = match git::snapshot(Path::new(cwd)) {
        Ok(s) => s,
        Err(e) => {
            out.error = Some(e);
            return out;
        }
    };
    if let Some(main) = snapshot.worktrees.first() {
        out.main = main.path.clone();
        out.default_root = monocode_core::worktree_management::default_root(Path::new(&main.path));
    }
    out.branches = snapshot.branches;
    match monocode_core::worktree_management::sweep(Path::new(cwd), None) {
        // One inspection per worktree feeds both the list and Cleanup.
        Ok((base, sweep)) => {
            out.trees = sweep
                .iter()
                .filter(|s| !s.tree.main)
                .map(|s| Tree {
                    tree: s.tree.clone(),
                    locked: s.locked,
                    dirty: s.dirty,
                    unpushed: s.unpushed.unwrap_or(0),
                })
                .collect();
            out.base = base;
            out.sweep = sweep;
        }
        Err(_) => {
            out.trees = snapshot
                .worktrees
                .into_iter()
                .filter(|t| !t.main)
                .map(|tree| {
                    let status = monocode_core::worktree_management::status(Path::new(cwd), &tree);
                    Tree {
                        tree,
                        locked: status.0,
                        dirty: status.1,
                        unpushed: status.2,
                    }
                })
                .collect();
        }
    }
    out
}
