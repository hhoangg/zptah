//! Claude Code MCP servers, sign-in and approval per Claude account.
//!
//! Sessions run Claude with the account's own config dir (`CLAUDE_CONFIG_DIR`), so everything the
//! CLI keeps for an MCP server is per account: the servers (`<config dir>/.claude.json`), the
//! project trust that unlocks `.mcp.json` approvals (the same file) and the OAuth tokens (secure
//! storage of that config dir). Every function here works on the account it is given and never
//! reads or copies a token. See docs/adr/0016-mcp-servers-and-sign-in-per-claude-account.md.
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::{expand_home, read_json, resolve_mcp_binary, write_json_file};
use crate::accounts::{self, Profile};
use crate::cli::{self, Env};
use crate::provider::Kind;
use crate::scm::shell::dirs_home;

/// How a server proves who the user is. Only the kind is kept, never a header value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    /// A remote server that signs in through the browser (OAuth).
    Oauth,
    /// A remote server that authenticates with a key in its config (header, helper or env var).
    Header,
    /// A local (stdio) server: nothing to sign in to.
    #[default]
    None,
}

/// The kind of sign-in a server config needs.
pub fn auth_kind(config: &Value) -> AuthKind {
    let transport = super::transport(config);
    if matches!(transport, "stdio" | "local" | "ws") {
        return AuthKind::None;
    }
    if config.get("url").is_none() {
        return AuthKind::None;
    }
    let authorization = config
        .get("headers")
        .and_then(Value::as_object)
        .is_some_and(|headers| {
            headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("authorization"))
        });
    if authorization
        || config.get("headersHelper").is_some()
        || config.get("bearerTokenEnvVar").is_some()
    {
        AuthKind::Header
    } else {
        AuthKind::Oauth
    }
}

/// Same for a Codex `config.toml` table.
pub(super) fn toml_auth_kind(entry: &toml::Value) -> AuthKind {
    if entry.get("url").is_none() {
        return AuthKind::None;
    }
    let authorization = entry
        .get("http_headers")
        .and_then(toml::Value::as_table)
        .is_some_and(|headers| {
            headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("authorization"))
        });
    if authorization || entry.get("bearer_token_env_var").is_some() {
        AuthKind::Header
    } else {
        AuthKind::Oauth
    }
}

/// The config file Claude reads user and local servers, trust and projects from for an account.
pub fn config_file(profile: &Profile) -> Result<PathBuf, String> {
    match profile.directory(Kind::Claude)? {
        Some(dir) => Ok(dir.join(".claude.json")),
        None => dirs_home()
            .map(|home| PathBuf::from(home).join(".claude.json"))
            .ok_or_else(|| "Home directory not found".into()),
    }
}

/// The environment `claude` needs to act as this account. The default account clears the config
/// dir variables, because the app itself may have been started from a Claude session that set them.
pub fn account_env(profile: &Profile) -> Result<Env, String> {
    let (set, remove) = profile.env(Kind::Claude)?;
    let mut env = Env {
        set,
        remove: remove.into_iter().map(str::to_owned).collect(),
    };
    if profile.directory(Kind::Claude)?.is_none() {
        env.remove.extend([
            "CLAUDE_CONFIG_DIR".to_owned(),
            "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_owned(),
        ]);
    }
    Ok(env)
}

fn is_default(profile: &Profile) -> bool {
    profile.id == accounts::DEFAULT
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn run_claude(
    binary: &Path,
    profile: Option<&Profile>,
    args: &[&str],
    cwd: &str,
    timeout: Duration,
    stdin: Option<&[u8]>,
) -> Result<String, String> {
    let env = match profile {
        Some(profile) => account_env(profile)?,
        None => Env::default(),
    };
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    cli::output_with(binary, &args, &expand_home(cwd), timeout, &env, stdin)
}

/// `claude mcp list` as this account.
pub fn claude_health_for(cwd: &str, profile: &Profile) -> Result<String, String> {
    health_at(
        &resolve_mcp_binary("claude", None)?,
        cwd,
        profile,
        Duration::from_secs(30),
    )
}

fn health_at(
    binary: &Path,
    cwd: &str,
    profile: &Profile,
    timeout: Duration,
) -> Result<String, String> {
    run_claude(binary, Some(profile), &["mcp", "list"], cwd, timeout, None)
}

/// A server that exists in the CLI login (default account) but not in the chosen account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliOnly {
    pub name: String,
    /// `user` or `local`.
    pub scope: String,
    pub transport: String,
    pub auth: AuthKind,
}

fn servers_of(config: &Value, project: &Path) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    let mut add = |scope: &str, servers: Option<&Value>| {
        if let Some(servers) = servers.and_then(Value::as_object) {
            for (name, server) in servers.iter().filter(|(_, s)| s.is_object()) {
                out.push((scope.to_owned(), name.clone(), server.clone()));
            }
        }
    };
    add("user", config.get("mcpServers"));
    add(
        "local",
        config
            .get("projects")
            .and_then(|projects| projects.get(project_key(project)))
            .and_then(|entry| entry.get("mcpServers")),
    );
    out
}

fn project_key(project: &Path) -> String {
    let text = project.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        text.into_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Servers in `cli_login_config` (user scope, and local scope for `project`) whose name is absent
/// from `account_config`. Nothing is returned for the default account itself.
pub fn only_in_cli_login(
    cli_login_config: &Path,
    account_config: &Path,
    project: &Path,
) -> Vec<CliOnly> {
    if cli_login_config == account_config {
        return Vec::new();
    }
    let Some(source) = read_json(cli_login_config) else {
        return Vec::new();
    };
    let present: Vec<String> = read_json(account_config)
        .map(|c| servers_of(&c, project))
        .unwrap_or_default()
        .into_iter()
        .map(|(_, name, _)| name)
        .collect();
    let mut out: Vec<CliOnly> = servers_of(&source, project)
        .into_iter()
        .filter(|(_, name, _)| !present.contains(name))
        .map(|(scope, name, server)| CliOnly {
            transport: super::transport(&server).to_owned(),
            auth: auth_kind(&server),
            name,
            scope,
        })
        .collect();
    out.sort_by(|a, b| (&a.name, &a.scope).cmp(&(&b.name, &b.scope)));
    out
}

/// Servers only in the CLI login, for the account's own config. Empty for the default account.
pub fn only_in_cli_login_for(cwd: &str, profile: &Profile) -> Vec<CliOnly> {
    if is_default(profile) {
        return Vec::new();
    }
    let (Ok(account), Ok(cli_login)) = (config_file(profile), config_file(&Profile::default()))
    else {
        return Vec::new();
    };
    only_in_cli_login(&cli_login, &account, &expand_home(cwd))
}

/// Adds a server from the CLI login's config to `profile` with `claude mcp add-json`. The server
/// JSON (which may hold a key) goes through stdin, never through the argument list.
pub fn add_from_cli_login(
    cwd: &str,
    profile: &Profile,
    name: &str,
    scope: &str,
) -> Result<(), String> {
    add_from_cli_login_at(
        &resolve_mcp_binary("claude", None)?,
        &config_file(&Profile::default())?,
        cwd,
        profile,
        name,
        scope,
    )
}

fn add_from_cli_login_at(
    binary: &Path,
    cli_login_config: &Path,
    cwd: &str,
    profile: &Profile,
    name: &str,
    scope: &str,
) -> Result<(), String> {
    if is_default(profile) {
        return Err("The CLI login already has this server.".into());
    }
    if !valid_name(name) {
        return Err("Invalid MCP server name".into());
    }
    if !matches!(scope, "user" | "local") {
        return Err("Only user and local servers can be added from the CLI login.".into());
    }
    let project = expand_home(cwd);
    let source = read_json(cli_login_config).ok_or("The CLI login config could not be read.")?;
    let server = servers_of(&source, &project)
        .into_iter()
        .find(|(s, n, _)| s == scope && n == name)
        .map(|(_, _, server)| server)
        .ok_or_else(|| format!("{name} is no longer in the CLI login."))?;
    let json = serde_json::to_vec(&server).map_err(|e| e.to_string())?;
    run_claude(
        binary,
        Some(profile),
        &["mcp", "add-json", name, "-", "--scope", scope],
        cwd,
        Duration::from_secs(30),
        Some(&json),
    )
    .map(|_| ())
}

/// Approves a project `.mcp.json` server for the account that owns `account_config`.
///
/// Claude Code only honours `enabledMcpjsonServers` from the project's settings when the folder
/// is trusted for that config dir, so this does both, on an explicit user action:
/// 1. adds `name` to `enabledMcpjsonServers` (and drops it from `disabledMcpjsonServers`) in
///    `<project>/.claude/settings.local.json`;
/// 2. sets `projects[<project>].hasTrustDialogAccepted = true` in `account_config`.
///
/// Every other key is kept. The home folder is never trusted.
pub fn approve(
    account_config: &Path,
    project: &Path,
    home: Option<&Path>,
    name: &str,
) -> Result<(), String> {
    if !valid_name(name) {
        return Err("Invalid MCP server name".into());
    }
    if !project.is_absolute() {
        return Err("The project folder must be an absolute path.".into());
    }
    let key = project_key(project);
    if home.is_some_and(|home| project_key(home) == key) {
        return Err(
            "The home folder cannot be marked as trusted. Open a project folder instead.".into(),
        );
    }
    if !project.is_dir() {
        return Err("The project folder does not exist.".into());
    }
    write_json_file(
        &project.join(".claude/settings.local.json"),
        |root| {
            let object = root
                .as_object_mut()
                .ok_or("settings.local.json must be a JSON object")?;
            let enabled = object
                .entry("enabledMcpjsonServers")
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .ok_or("enabledMcpjsonServers must be a list")?;
            if !enabled.iter().any(|v| v.as_str() == Some(name)) {
                enabled.push(Value::String(name.to_owned()));
            }
            if let Some(disabled) = object
                .get_mut("disabledMcpjsonServers")
                .and_then(Value::as_array_mut)
            {
                disabled.retain(|v| v.as_str() != Some(name));
            }
            Ok(())
        },
        true,
    )?;
    write_json_file(
        account_config,
        |root| {
            let object = root
                .as_object_mut()
                .ok_or("The Claude config must be a JSON object")?;
            let projects = object
                .entry("projects")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .ok_or("projects must be an object")?;
            let entry = projects
                .entry(key.clone())
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .ok_or("The project entry must be an object")?;
            entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
            Ok(())
        },
        true,
    )
}

/// The folder Claude Code keys local settings and trust by: the canonical git root of `project`,
/// which for a git worktree is the main working tree. Not a git repo: the canonical path.
pub fn project_root(project: &Path) -> PathBuf {
    let real = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    let common = cli::output(
        Path::new("git"),
        &[
            "rev-parse".to_owned(),
            "--path-format=absolute".to_owned(),
            "--git-common-dir".to_owned(),
        ],
        &real,
        Duration::from_secs(10),
    )
    .ok()
    .map(|text| PathBuf::from(text.trim()));
    match common {
        Some(dir) if dir.is_absolute() && dir.file_name().is_some_and(|n| n == ".git") => dir
            .parent()
            .and_then(|root| std::fs::canonicalize(root).ok())
            .unwrap_or(real),
        _ => real,
    }
}

/// [`approve`] for an account and a project path as the app knows them. A session in a git
/// worktree reads the main working tree's settings and trust, so that is where this writes.
pub fn approve_for(cwd: &str, profile: &Profile, name: &str) -> Result<(), String> {
    let home = dirs_home().map(|h| std::fs::canonicalize(&h).unwrap_or_else(|_| PathBuf::from(h)));
    approve(
        &config_file(profile)?,
        &project_root(&expand_home(cwd)),
        home.as_deref(),
        name,
    )
}

/// The command that signs in to an MCP server, with the environment of the account it runs as.
/// The caller runs it on a pseudo-terminal, because the provider CLIs refuse to sign in without one.
#[derive(Clone)]
pub struct LoginCommand {
    pub binary: PathBuf,
    pub args: Vec<String>,
    /// The (home-expanded) working directory.
    pub cwd: PathBuf,
    pub env: Env,
}

/// Shows the variable names of the environment, never its values.
impl std::fmt::Debug for LoginCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginCommand")
            .field("binary", &self.binary)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("env", &self.env)
            .finish()
    }
}

/// A plain sentence for a known CLI failure, or `None` to keep the generic message. `account` is
/// the label of the account the command ran as.
pub fn plain_login_error(name: &str, account: &str, output: &str) -> Option<String> {
    let lower = output.to_lowercase();
    if lower.contains("no separate login")
        || (lower.contains("authenticates with the") && lower.contains("header"))
    {
        return Some(format!(
            "{name} signs in with the Authorization header in its config, so there is nothing to \
             sign in to. Edit that header to change its key."
        ));
    }
    if lower.contains("no mcp server named") {
        return Some(format!(
            "{account} does not have {name} configured for this project. Add it to this account \
             first."
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(root: &Path) -> PathBuf {
        let project = root.join("work/app");
        std::fs::create_dir_all(&project).unwrap();
        project
    }

    #[test]
    fn auth_kind_tells_oauth_header_and_local_servers_apart() {
        let kind = |value: Value| auth_kind(&value);
        assert_eq!(
            kind(json!({"type": "http", "url": "https://x.example/mcp"})),
            AuthKind::Oauth
        );
        assert_eq!(
            kind(json!({"type": "sse", "url": "https://x.example/sse", "headers": {}})),
            AuthKind::Oauth
        );
        assert_eq!(
            kind(json!({"type": "http", "url": "https://x", "headers": {"authorization": "k"}})),
            AuthKind::Header
        );
        assert_eq!(
            kind(json!({"type": "http", "url": "https://x", "headers": {"Authorization": "k"}})),
            AuthKind::Header
        );
        assert_eq!(
            kind(json!({"url": "https://x", "headersHelper": "echo"})),
            AuthKind::Header
        );
        assert_eq!(
            kind(json!({"type": "http", "url": "https://x", "bearerTokenEnvVar": "TOKEN"})),
            AuthKind::Header
        );
        // A header that is not credentials does not stop an OAuth sign-in.
        assert_eq!(
            kind(json!({"type": "http", "url": "https://x", "headers": {"X-Team": "a"}})),
            AuthKind::Oauth
        );
        assert_eq!(
            kind(json!({"command": "npx", "args": ["x"]})),
            AuthKind::None
        );
        assert_eq!(
            kind(json!({"type": "stdio", "command": "npx"})),
            AuthKind::None
        );
    }

    #[test]
    fn toml_auth_kind_reads_codex_bearer_and_headers() {
        let parse = |text: &str| toml_auth_kind(&toml::from_str::<toml::Value>(text).unwrap());
        assert_eq!(parse("url = 'https://x'"), AuthKind::Oauth);
        assert_eq!(
            parse("url = 'https://x'\nbearer_token_env_var = 'T'"),
            AuthKind::Header
        );
        assert_eq!(
            parse("url = 'https://x'\n[http_headers]\nAuthorization = 'k'"),
            AuthKind::Header
        );
        assert_eq!(parse("command = 'node'"), AuthKind::None);
    }

    #[test]
    fn config_file_follows_the_account_directory() {
        let root = tempfile::tempdir().unwrap();
        let profile = Profile::new("work", root.path());
        assert_eq!(
            config_file(&profile).unwrap(),
            root.path()
                .join("provider-accounts/claude/work/.claude.json")
        );
    }

    #[test]
    fn account_env_sets_both_config_variables_and_default_clears_them() {
        let root = tempfile::tempdir().unwrap();
        let env = account_env(&Profile::new("work", root.path())).unwrap();
        let dir = root.path().join("provider-accounts/claude/work");
        for key in ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"] {
            assert!(env.set.iter().any(|(k, v)| k == key && Path::new(v) == dir));
        }
        assert!(env.remove.contains(&"ANTHROPIC_API_KEY".to_owned()));
        assert!(env.remove.contains(&"CLAUDE_CODE_OAUTH_TOKEN".to_owned()));
        let default = account_env(&Profile::default()).unwrap();
        assert!(default.set.is_empty());
        for key in ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"] {
            assert!(default.remove.contains(&key.to_owned()));
        }
    }

    #[test]
    fn lists_servers_only_in_the_cli_login() {
        let root = tempfile::tempdir().unwrap();
        let project = project(root.path());
        let key = project.to_string_lossy().into_owned();
        let cli_login = root.path().join("cli.json");
        let account = root.path().join("account.json");
        std::fs::write(
            &cli_login,
            json!({
                "mcpServers": {
                    "clickup": {"type": "http", "url": "https://c"},
                    "keyed": {"type": "http", "url": "https://k", "headers": {"Authorization": "secret"}},
                    "shared": {"command": "npx"}
                },
                "projects": {key: {"mcpServers": {"coolify": {"type": "http", "url": "https://x", "headers": {"Authorization": "secret"}}}}}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            &account,
            json!({"mcpServers": {"shared": {"command": "npx"}}}).to_string(),
        )
        .unwrap();
        let found = only_in_cli_login(&cli_login, &account, &project);
        let summary: Vec<_> = found
            .iter()
            .map(|s| (s.name.as_str(), s.scope.as_str(), s.auth))
            .collect();
        assert_eq!(
            summary,
            [
                ("clickup", "user", AuthKind::Oauth),
                ("coolify", "local", AuthKind::Header),
                ("keyed", "user", AuthKind::Header),
            ]
        );
        assert!(!format!("{found:?}").contains("secret"));
        assert!(only_in_cli_login(&cli_login, &cli_login, &project).is_empty());
        // A different project does not see the local server.
        let other = root.path().join("other");
        let found = only_in_cli_login(&cli_login, &account, &other);
        assert!(found.iter().all(|s| s.name != "coolify"));
    }

    #[test]
    fn approve_enables_the_server_and_trusts_the_project_keeping_other_keys() {
        let root = tempfile::tempdir().unwrap();
        let project = project(root.path());
        let key = project.to_string_lossy().into_owned();
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::write(
            project.join(".claude/settings.local.json"),
            r#"{"permissions":{"allow":["Bash(ls)"]},"enabledMcpjsonServers":["other"],"disabledMcpjsonServers":["supabase","x"],"zeta":1}"#,
        )
        .unwrap();
        let config = root.path().join("acct/.claude.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            json!({"zzz": 1, "aaa": {"b": 2}, "projects": {key.clone(): {"allowedTools": ["a"], "mcpServers": {"p": {}}}, "/else": {"hasTrustDialogAccepted": false}}}).to_string(),
        )
        .unwrap();

        approve(&config, &project, None, "supabase").unwrap();
        approve(&config, &project, None, "supabase").unwrap();

        let settings: Value = read_json(&project.join(".claude/settings.local.json")).unwrap();
        assert_eq!(
            settings["enabledMcpjsonServers"],
            json!(["other", "supabase"])
        );
        assert_eq!(settings["disabledMcpjsonServers"], json!(["x"]));
        assert_eq!(settings["permissions"]["allow"], json!(["Bash(ls)"]));
        assert_eq!(settings["zeta"], 1);
        let saved: Value = read_json(&config).unwrap();
        assert_eq!(saved["projects"][&key]["hasTrustDialogAccepted"], true);
        assert_eq!(saved["projects"][&key]["allowedTools"], json!(["a"]));
        assert_eq!(saved["projects"][&key]["mcpServers"], json!({"p": {}}));
        assert_eq!(saved["projects"]["/else"]["hasTrustDialogAccepted"], false);
        assert_eq!(saved["aaa"], json!({"b": 2}));
        // Only the content is promised: serde_json sorts the keys of the file it rewrites.
    }

    #[test]
    fn approve_creates_missing_files_and_entries() {
        let root = tempfile::tempdir().unwrap();
        let project = project(root.path());
        let config = root.path().join("acct/.claude.json");
        approve(&config, &project, None, "supabase").unwrap();
        let settings = read_json(&project.join(".claude/settings.local.json")).unwrap();
        assert_eq!(settings["enabledMcpjsonServers"], json!(["supabase"]));
        let saved = read_json(&config).unwrap();
        let key = project.to_string_lossy().into_owned();
        assert_eq!(saved["projects"][&key]["hasTrustDialogAccepted"], true);
    }

    #[test]
    fn approve_refuses_home_bad_names_and_invalid_files() {
        let root = tempfile::tempdir().unwrap();
        let project = project(root.path());
        let config = root.path().join("acct/.claude.json");
        let home = root.path().join("work/app/");
        assert!(approve(&config, &project, Some(&home), "supabase")
            .unwrap_err()
            .contains("home folder"));
        assert!(!config.exists());
        assert!(!project.join(".claude").exists());
        assert!(approve(&config, &project, None, "bad name").is_err());
        assert!(approve(&config, Path::new("relative"), None, "x").is_err());
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "[1]").unwrap();
        assert!(approve(&config, &project, None, "supabase").is_err());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "[1]");
    }

    #[cfg(unix)]
    #[test]
    fn approve_keeps_the_mode_of_an_existing_file_and_creates_new_ones_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let project = project(root.path());
        let config = root.path().join("acct/.claude.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "{}").unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o640)).unwrap();
        approve(&config, &project, None, "supabase").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&config), 0o640);
        assert_eq!(mode(&project.join(".claude/settings.local.json")), 0o600);
    }

    #[test]
    fn approve_leaves_nothing_but_the_edited_files_next_to_them() {
        let root = tempfile::tempdir().unwrap();
        let project = project(root.path());
        let config = root.path().join("acct/.claude.json");
        approve(&config, &project, None, "supabase").unwrap();
        let names = |dir: &Path| {
            let mut names: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        assert_eq!(names(config.parent().unwrap()), [".claude.json"]);
        assert_eq!(names(&project.join(".claude")), ["settings.local.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_stays_a_link_and_the_real_file_is_edited() {
        let root = tempfile::tempdir().unwrap();
        let project = project(root.path());
        let real = root.path().join("dotfiles/claude.json");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, r#"{"keep":1}"#).unwrap();
        let link = root.path().join("acct/.claude.json");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        approve(&link, &project, None, "supabase").unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        let saved = read_json(&real).unwrap();
        assert_eq!(saved["keep"], 1);
        assert_eq!(saved["projects"].as_object().unwrap().len(), 1);
        assert_eq!(
            std::fs::read_dir(real.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn an_edit_is_applied_again_when_another_process_wrote_the_file_meanwhile() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".claude.json");
        std::fs::write(&path, r#"{"a":1}"#).unwrap();
        let mut calls = 0;
        write_json_file(
            &path,
            |value| {
                calls += 1;
                if calls == 1 {
                    // Claude rewrites its config between our read and our rename.
                    std::fs::write(&path, r#"{"a":1,"claude":"wrote this"}"#).unwrap();
                }
                value["mine"] = json!(true);
                Ok(())
            },
            true,
        )
        .unwrap();
        assert_eq!(calls, 2);
        let saved = read_json(&path).unwrap();
        assert_eq!(saved["claude"], "wrote this");
        assert_eq!(saved["mine"], true);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_file_that_keeps_changing_fails_in_plain_words_and_leaves_no_temp_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".claude.json");
        std::fs::write(&path, "{}").unwrap();
        let mut size = 0;
        let error = write_json_file(
            &path,
            |_| {
                size += 1;
                std::fs::write(&path, format!("{{{}}}", " ".repeat(size))).unwrap();
                Ok(())
            },
            true,
        )
        .unwrap_err();
        assert!(error.contains("keeps changing"), "{error}");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn approve_targets_the_main_working_tree_of_a_worktree() {
        let root = tempfile::tempdir().unwrap();
        let main = std::fs::canonicalize(root.path()).unwrap().join("repo");
        std::fs::create_dir_all(&main).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(["-c", "user.email=a@example.com", "-c", "user.name=a"])
                .args(args)
                .current_dir(dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .unwrap();
            assert!(status.status.success(), "{args:?}");
        };
        git(&main, &["init", "-q"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "x"]);
        let tree = main.parent().unwrap().join("tree");
        git(&main, &["worktree", "add", "-q", tree.to_str().unwrap()]);
        let sub = tree.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(project_root(&tree), main);
        assert_eq!(project_root(&sub), main);
        assert_eq!(project_root(&main), main);
        // Not a repository: the canonical path itself.
        let plain = std::fs::canonicalize(root.path()).unwrap();
        let outside = tempfile::tempdir_in(&plain).unwrap();
        let outside = std::fs::canonicalize(outside.path()).unwrap();
        if !plain.join(".git").exists() {
            assert_eq!(project_root(&outside), outside);
        }

        let config = root.path().join("acct/.claude.json");
        approve(&config, &project_root(&tree), None, "supabase").unwrap();
        let saved = read_json(&config).unwrap();
        let key = main.to_string_lossy().into_owned();
        assert_eq!(saved["projects"][&key]["hasTrustDialogAccepted"], true);
        let settings = read_json(&main.join(".claude/settings.local.json")).unwrap();
        assert_eq!(settings["enabledMcpjsonServers"], json!(["supabase"]));
        assert!(!tree.join(".claude").exists());
    }

    #[test]
    fn debug_output_never_shows_environment_values() {
        let env = Env {
            set: vec![("CLAUDE_CODE_OAUTH_TOKEN".into(), "TOPSECRET".into())],
            remove: vec!["X".into()],
        };
        let login = LoginCommand {
            binary: "claude".into(),
            args: vec!["mcp".into()],
            cwd: "/p".into(),
            env: env.clone(),
        };
        for text in [format!("{env:?}"), format!("{login:?}")] {
            assert!(text.contains("CLAUDE_CODE_OAUTH_TOKEN"));
            assert!(!text.contains("TOPSECRET"));
        }
    }

    #[cfg(unix)]
    fn fake_claude(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("claude");
        let out = dir.join("seen");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n{{\nprintf 'dir=%s\\n' \"${{CLAUDE_CONFIG_DIR-unset}}\"\n\
                 printf 'secure=%s\\n' \"${{CLAUDE_SECURESTORAGE_CONFIG_DIR-unset}}\"\n\
                 printf 'key=%s\\n' \"${{ANTHROPIC_API_KEY-unset}}\"\n\
                 printf 'args=%s\\n' \"$*\"\n\
                 printf 'cwd=%s\\n' \"$PWD\"\n}} > '{out}'\ncat > '{out}.stdin'\n\
                 echo 'x: https://x (HTTP) - Connected'\n",
                out = out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        script
    }

    #[cfg(unix)]
    #[test]
    fn health_runs_with_the_account_environment() {
        let root = tempfile::tempdir().unwrap();
        let project = project(root.path());
        let binary = fake_claude(root.path());
        let profile = Profile::new("work", root.path());
        let text = health_at(
            &binary,
            project.to_str().unwrap(),
            &profile,
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(text.contains("Connected"));
        let seen = std::fs::read_to_string(root.path().join("seen")).unwrap();
        let dir = root.path().join("provider-accounts/claude/work");
        assert!(seen.contains(&format!("dir={}\n", dir.display())));
        assert!(seen.contains(&format!("secure={}\n", dir.display())));
        assert!(seen.contains("args=mcp list\n"));
        assert!(seen.contains("key=unset\n"));
    }

    #[cfg(unix)]
    #[test]
    fn default_account_does_not_inherit_a_stray_config_dir() {
        // The variable is set on the command through `Env`, which is what the stray value of the
        // app process would look like to the child.
        let root = tempfile::tempdir().unwrap();
        let binary = fake_claude(root.path());
        let env = account_env(&Profile::default()).unwrap();
        let mut command = std::process::Command::new(&binary);
        command.env("CLAUDE_CONFIG_DIR", "/stray");
        env.apply(&mut command);
        command.stdin(std::process::Stdio::null()).output().unwrap();
        let seen = std::fs::read_to_string(root.path().join("seen")).unwrap();
        assert!(seen.contains("dir=unset\n"));
        assert!(seen.contains("secure=unset\n"));
    }

    #[cfg(unix)]
    #[test]
    fn add_from_cli_login_sends_the_server_on_stdin_not_in_arguments() {
        let root = tempfile::tempdir().unwrap();
        let project = project(root.path());
        let binary = fake_claude(root.path());
        let cli_login = root.path().join("cli.json");
        let key = project.to_string_lossy().into_owned();
        std::fs::write(
            &cli_login,
            json!({
                "mcpServers": {"keyed": {"type": "http", "url": "https://k", "headers": {"Authorization": "TOPSECRET"}}},
                "projects": {key: {"mcpServers": {"coolify": {"type": "http", "url": "https://c"}}}}
            })
            .to_string(),
        )
        .unwrap();
        let profile = Profile::new("work", root.path());
        let cwd = project.to_str().unwrap();
        add_from_cli_login_at(&binary, &cli_login, cwd, &profile, "keyed", "user").unwrap();
        let seen = std::fs::read_to_string(root.path().join("seen")).unwrap();
        assert!(seen.contains("args=mcp add-json keyed - --scope user\n"));
        assert!(!seen.contains("TOPSECRET"));
        let stdin = std::fs::read_to_string(root.path().join("seen.stdin")).unwrap();
        let sent: Value = serde_json::from_str(&stdin).unwrap();
        assert_eq!(sent["headers"]["Authorization"], "TOPSECRET");
        assert!(seen.contains("provider-accounts/claude/work"));

        add_from_cli_login_at(&binary, &cli_login, cwd, &profile, "coolify", "local").unwrap();
        let seen = std::fs::read_to_string(root.path().join("seen")).unwrap();
        assert!(seen.contains("args=mcp add-json coolify - --scope local\n"));

        for (name, scope) in [
            ("missing", "user"),
            ("keyed", "local"),
            ("keyed", "project"),
        ] {
            assert!(
                add_from_cli_login_at(&binary, &cli_login, cwd, &profile, name, scope).is_err()
            );
        }
        assert!(add_from_cli_login_at(
            &binary,
            &cli_login,
            cwd,
            &Profile::default(),
            "keyed",
            "user"
        )
        .is_err());
        assert!(
            add_from_cli_login_at(&binary, &cli_login, cwd, &profile, "bad name", "user").is_err()
        );
    }

    #[test]
    fn known_cli_failures_become_plain_sentences() {
        let header = "\"coolify\" authenticates with the `Authorization` header in its \
                      configuration, so there's no separate login. Update that header to change \
                      its credentials.";
        let message = plain_login_error("coolify", "Work", header).unwrap();
        assert!(message.contains("coolify signs in with the Authorization header"));
        assert!(message.contains("nothing to sign in to"));
        let missing = plain_login_error(
            "coolify",
            "Work",
            "No MCP server named \"coolify\". Configured servers: playwright",
        )
        .unwrap();
        assert_eq!(
            missing,
            "Work does not have coolify configured for this project. Add it to this account first."
        );
        assert_eq!(plain_login_error("x", "Work", "boom"), None);
    }
}
