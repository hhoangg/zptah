//! What the MCP page shows and offers per server and per account. Pure functions, so the rules
//! are tested without a window.
use crate::cli_auth;
use crate::settings::data::McpServer;
use monocode_core::{
    accounts::Book,
    mcp::{AuthKind, plain_login_error},
    provider::Kind,
};

pub(in crate::settings) const STATUS_ONLY_CLI: &str = "Only in CLI login";
pub(in crate::settings) const STATUS_NEEDS_AUTH: &str = "Needs authentication";
pub(in crate::settings) const STATUS_NEEDS_APPROVAL: &str = "Needs approval";
pub(in crate::settings) const STATUS_CONNECTED: &str = "Connected";
pub(in crate::settings) const STATUS_REJECTED: &str = "Rejected";
const STATUS_FAILED: &str = "Failed";

/// The one main action of a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::settings) enum Primary {
    None,
    SignIn,
    Approve,
    AddToAccount,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::settings) struct RowView {
    pub primary: Primary,
    /// Plain text shown when the row has nothing to click for its state.
    pub note: Option<&'static str>,
    /// The row has a menu with "Sign in again" (an OAuth server that is not asking to sign in).
    pub menu: bool,
    /// Whether "Sign in for all accounts" makes sense for this row: a button next to Sign in, or
    /// an item of the menu.
    pub all_accounts: bool,
}

const NONE: RowView = RowView {
    primary: Primary::None,
    note: None,
    menu: false,
    all_accounts: false,
};

/// Which controls a server row gets. Claude reports a live status, so Sign in appears only when
/// the server needs it; the other providers report none, so an OAuth server keeps its button.
pub(in crate::settings) fn row_view(server: &McpServer) -> RowView {
    if server.provider == "claude_desktop" {
        return NONE;
    }
    if server.only_cli {
        return RowView {
            primary: Primary::AddToAccount,
            ..NONE
        };
    }
    let claude = server.provider == "claude";
    if claude
        && matches!(
            server.status.as_str(),
            STATUS_NEEDS_APPROVAL | STATUS_REJECTED
        )
    {
        return RowView {
            primary: Primary::Approve,
            ..NONE
        };
    }
    match server.auth {
        AuthKind::None => NONE,
        AuthKind::Header => RowView {
            note: Some("Uses a key in its config"),
            ..NONE
        },
        AuthKind::Oauth if !claude => RowView {
            primary: Primary::SignIn,
            ..NONE
        },
        AuthKind::Oauth => match server.status.as_str() {
            STATUS_NEEDS_AUTH => RowView {
                primary: Primary::SignIn,
                all_accounts: true,
                ..NONE
            },
            // Connected does not prove a sign-in happened (a server may need none), so no claim.
            STATUS_CONNECTED | STATUS_FAILED => RowView {
                menu: true,
                all_accounts: true,
                ..NONE
            },
            _ => NONE,
        },
    }
}

/// The account whose servers the page shows: the explicit choice if it still exists, else the
/// account new sessions in this project use.
pub(in crate::settings) fn effective_account(
    book: &Book,
    project: &str,
    chosen: Option<&str>,
) -> String {
    chosen
        .filter(|id| book.exists(Kind::Claude, id))
        .map(str::to_owned)
        .unwrap_or_else(|| book.selected(Kind::Claude, project))
}

/// What "Sign in for all accounts" does for one account, from its own `claude mcp list` status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::settings) enum Step {
    SignIn,
    Already,
    NotConfigured,
    NeedsApproval,
    /// Another status (failed, disabled...): signing in would not help.
    Other(String),
}

pub(in crate::settings) fn plan_step(status: Option<&str>) -> Step {
    match status {
        Some(STATUS_NEEDS_AUTH) => Step::SignIn,
        Some(STATUS_CONNECTED) => Step::Already,
        Some(STATUS_NEEDS_APPROVAL) => Step::NeedsApproval,
        None => Step::NotConfigured,
        Some(other) => Step::Other(other.to_owned()),
    }
}

/// How one account ended in "Sign in for all accounts".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::settings) enum Done {
    SignedIn,
    Already,
    NotConfigured,
    NeedsApproval,
    Other(String),
    Failed(String),
    Cancelled,
}

impl From<Step> for Done {
    fn from(step: Step) -> Self {
        match step {
            Step::SignIn => Self::Failed("not attempted".into()),
            Step::Already => Self::Already,
            Step::NotConfigured => Self::NotConfigured,
            Step::NeedsApproval => Self::NeedsApproval,
            Step::Other(status) => Self::Other(status),
        }
    }
}

/// The page message and the "Show details" lines for a finished run over all accounts.
pub(in crate::settings) fn batch_summary(name: &str, done: &[(String, Done)]) -> (String, String) {
    let count = |wanted: fn(&Done) -> bool| done.iter().filter(|(_, d)| wanted(d)).count();
    let signed = count(|d| matches!(d, Done::SignedIn));
    let failed = count(|d| matches!(d, Done::Failed(_)));
    let mut parts = vec![format!("{signed} signed in")];
    for (n, text) in [
        (count(|d| matches!(d, Done::Already)), "already signed in"),
        (
            count(|d| matches!(d, Done::NotConfigured)),
            "do not have it",
        ),
        (
            count(|d| matches!(d, Done::NeedsApproval)),
            "need approval first",
        ),
        (count(|d| matches!(d, Done::Other(_))), "not signed in"),
        (failed, "failed"),
        (count(|d| matches!(d, Done::Cancelled)), "cancelled"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {text}"));
        }
    }
    let message = format!(
        "Sign in for all accounts, \"{name}\": {}.",
        parts.join(", ")
    );
    let lines = done
        .iter()
        .map(|(label, d)| {
            let text = match d {
                Done::SignedIn => "signed in".to_owned(),
                Done::Already => "already signed in".to_owned(),
                Done::NotConfigured => {
                    format!("does not have {name} configured. Add it to this account first.")
                }
                Done::NeedsApproval => {
                    format!("{name} needs approval first. Use Approve on this account.")
                }
                Done::Other(status) => format!("not signed in ({})", status.to_lowercase()),
                Done::Failed(reason) => format!("failed: {reason}"),
                Done::Cancelled => "cancelled".to_owned(),
            };
            format!("{label}: {text}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    (message, lines)
}

/// The line for the result of one sign-in, with a plain reason for the failures that have one.
pub(in crate::settings) fn sign_in_message(
    name: &str,
    provider: &str,
    account: Option<&str>,
    outcome: &cli_auth::Outcome,
) -> String {
    if provider == "claude"
        && let cli_auth::Outcome::Failed { output, .. } = outcome
        && let Some(plain) = plain_login_error(name, account.unwrap_or("This account"), output)
    {
        return plain;
    }
    let message = cli_auth::failure_message(name, provider, outcome);
    match (outcome, account) {
        (cli_auth::Outcome::Success, Some(account)) => {
            format!("Signed in to \"{name}\" for {account}.")
        }
        _ => message,
    }
}

/// Output of a failed command that is safe to show: nothing that looks like it could hold a key.
pub(in crate::settings) fn safe_details(output: &str) -> Option<String> {
    let cleaned = cli_auth::redact_urls(&cli_auth::clean_output(output));
    let lower = cleaned.to_lowercase();
    let risky = [
        "authorization",
        "bearer",
        "token",
        "secret",
        "api key",
        "apikey",
    ];
    (!cleaned.is_empty() && !risky.iter().any(|word| lower.contains(word))).then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(provider: &str, status: &str, auth: AuthKind) -> McpServer {
        McpServer {
            name: "s".into(),
            provider: provider.into(),
            scope: "user".into(),
            path: "/x".into(),
            transport: if auth == AuthKind::None {
                "stdio"
            } else {
                "http"
            }
            .into(),
            status: status.into(),
            auth,
            only_cli: false,
        }
    }

    #[test]
    fn claude_oauth_servers_offer_sign_in_only_when_they_need_it() {
        let view = |status: &str| row_view(&server("claude", status, AuthKind::Oauth));
        let needs = view(STATUS_NEEDS_AUTH);
        assert_eq!(needs.primary, Primary::SignIn);
        assert!(needs.all_accounts && !needs.menu && needs.note.is_none());
        let connected = view(STATUS_CONNECTED);
        assert_eq!(connected.primary, Primary::None);
        assert_eq!(connected.note, None);
        assert!(connected.menu && connected.all_accounts);
        assert_eq!(view("Failed").primary, Primary::None);
        assert!(view("Failed").menu);
        assert_eq!(view("Configured"), NONE);
        assert_eq!(view("Disabled"), NONE);
    }

    #[test]
    fn a_pending_or_rejected_project_server_offers_approve_whatever_its_auth() {
        for status in [STATUS_NEEDS_APPROVAL, STATUS_REJECTED] {
            for auth in [AuthKind::Oauth, AuthKind::Header, AuthKind::None] {
                let view = row_view(&server("claude", status, auth));
                assert_eq!(view.primary, Primary::Approve);
                assert!(!view.menu && !view.all_accounts);
            }
        }
    }

    #[test]
    fn key_and_local_servers_never_offer_sign_in() {
        for status in [STATUS_NEEDS_AUTH, STATUS_CONNECTED, "Failed", "Configured"] {
            let key = row_view(&server("claude", status, AuthKind::Header));
            assert_eq!(key.primary, Primary::None);
            assert!(!key.menu && !key.all_accounts);
            assert_eq!(key.note, Some("Uses a key in its config"));
            assert_eq!(row_view(&server("claude", status, AuthKind::None)), NONE);
        }
        let codex_key = row_view(&server("codex", "Configured", AuthKind::Header));
        assert_eq!(codex_key.primary, Primary::None);
    }

    #[test]
    fn providers_without_live_status_keep_the_button_for_oauth_servers() {
        let view = row_view(&server("cursor", "Configured", AuthKind::Oauth));
        assert_eq!(view.primary, Primary::SignIn);
        assert!(!view.all_accounts);
        assert_eq!(
            row_view(&server("cursor", "Configured", AuthKind::None)),
            NONE
        );
        assert_eq!(
            row_view(&server("claude_desktop", "Configured", AuthKind::Oauth)),
            NONE
        );
    }

    #[test]
    fn a_server_only_in_the_cli_login_offers_add() {
        let mut only = server("claude", STATUS_ONLY_CLI, AuthKind::Header);
        only.only_cli = true;
        assert_eq!(row_view(&only).primary, Primary::AddToAccount);
    }

    #[test]
    fn the_account_is_the_choice_or_what_new_sessions_use() {
        let mut book = Book::default();
        let work = book.add(Kind::Claude, "Work").unwrap();
        assert_eq!(effective_account(&book, "/p", None), "default");
        assert_eq!(
            effective_account(&book, "/p", Some(&work.id)),
            work.id.as_str()
        );
        assert_eq!(effective_account(&book, "/p", Some("gone")), "default");
        book.select(Kind::Claude, "/p", &work.id).unwrap();
        assert_eq!(effective_account(&book, "/p", None), work.id.as_str());
        assert_eq!(effective_account(&book, "/other", None), "default");
        assert_eq!(effective_account(&book, "/p", Some("default")), "default");
    }

    #[test]
    fn plans_follow_each_accounts_own_status() {
        assert_eq!(plan_step(Some("Needs authentication")), Step::SignIn);
        assert_eq!(plan_step(Some("Connected")), Step::Already);
        assert_eq!(plan_step(Some("Needs approval")), Step::NeedsApproval);
        assert_eq!(plan_step(None), Step::NotConfigured);
        assert_eq!(plan_step(Some("Failed")), Step::Other("Failed".into()));
    }

    #[test]
    fn the_summary_says_per_account_what_happened() {
        let done = vec![
            ("CLI login".to_owned(), Done::SignedIn),
            ("Work".to_owned(), Done::Already),
            ("Side".to_owned(), Done::NotConfigured),
            ("Old".to_owned(), Done::Failed("timed out".into())),
        ];
        let (message, lines) = batch_summary("clickup", &done);
        assert_eq!(
            message,
            "Sign in for all accounts, \"clickup\": 1 signed in, 1 already signed in, 1 do not have it, 1 failed."
        );
        let lines: Vec<_> = lines.lines().collect();
        assert_eq!(lines[0], "CLI login: signed in");
        assert_eq!(lines[1], "Work: already signed in");
        assert!(lines[2].starts_with("Side: does not have clickup configured"));
        assert_eq!(lines[3], "Old: failed: timed out");
    }

    #[test]
    fn sign_in_failures_are_explained_in_plain_words() {
        let failed = |output: &str| cli_auth::Outcome::Failed {
            code: Some(1),
            output: output.into(),
        };
        let header = failed(
            "\"coolify\" authenticates with the `Authorization` header in its configuration, so \
             there's no separate login.",
        );
        let message = sign_in_message("coolify", "claude", Some("Work"), &header);
        assert!(message.starts_with("coolify signs in with the Authorization header"));
        let missing = failed("No MCP server named \"coolify\". Configured servers: a");
        assert_eq!(
            sign_in_message("coolify", "claude", Some("Work"), &missing),
            "Work does not have coolify configured for this project. Add it to this account first."
        );
        assert_eq!(
            sign_in_message("x", "claude", None, &failed("boom")),
            "Sign-in for \"x\" failed (exit 1)."
        );
        assert_eq!(
            sign_in_message("x", "claude", Some("Work"), &cli_auth::Outcome::Success),
            "Signed in to \"x\" for Work."
        );
        assert_eq!(
            sign_in_message("x", "claude", None, &cli_auth::Outcome::Success),
            "Signed in to \"x\"."
        );
    }

    #[test]
    fn details_never_show_output_that_may_hold_a_key() {
        assert_eq!(safe_details("plain failure").unwrap(), "plain failure");
        assert!(safe_details("Authorization: Bearer abc").is_none());
        assert!(safe_details("").is_none());
    }
}
