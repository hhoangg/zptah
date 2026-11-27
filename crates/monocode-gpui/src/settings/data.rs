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
    /// How the server signs in (the kind only, never a credential).
    pub auth: monocode_core::mcp::AuthKind,
    /// The server exists in the CLI login but not in the account the page shows.
    pub only_cli: bool,
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
}
/// `account` is the Claude account whose servers are listed (the one sessions use, or the one the
/// page selector chose); `None` reads the CLI login's config.
pub(super) fn discover(cwd: &str, account: Option<&monocode_core::accounts::Profile>) -> Discovery {
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
    match monocode_core::mcp::mcp_discover_for(cwd.into(), account) {
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
                    auth: s.auth,
                    only_cli: false,
                })
                .collect()
        }
        Err(e) => data.mcp_error = Some(e),
    }
    if let Some(account) = account {
        let cli_config =
            monocode_core::mcp::config_file(&monocode_core::accounts::Profile::default())
                .unwrap_or_default();
        data.mcp.extend(
            monocode_core::mcp::only_in_cli_login_for(cwd, account)
                .into_iter()
                .map(|s| McpServer {
                    name: s.name,
                    provider: "claude".into(),
                    scope: s.scope,
                    path: cli_config.clone(),
                    transport: s.transport,
                    status: super::pages::mcp_row::STATUS_ONLY_CLI.into(),
                    auth: s.auth,
                    only_cli: true,
                }),
        );
    }
    if !cwd.is_empty() {
        match git::snapshot(Path::new(cwd)) {
            Ok(s) => {
                if let Some(main) = s.worktrees.first() {
                    data.worktrees.main = main.path.clone();
                    data.worktrees.default_root =
                        monocode_core::worktree_management::default_root(Path::new(&main.path));
                }
                data.worktrees.branches = s.branches;
                data.worktrees.trees = s
                    .worktrees
                    .into_iter()
                    .filter(|t| !t.main)
                    .map(|tree| {
                        let status =
                            monocode_core::worktree_management::status(Path::new(cwd), &tree);
                        Tree {
                            tree,
                            locked: status.0,
                            dirty: status.1,
                            unpushed: status.2,
                        }
                    })
                    .collect();
            }
            Err(e) => data.worktrees.error = Some(e),
        }
    }
    data
}
