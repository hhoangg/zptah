use super::mcp_row::{self, Done, Primary, Step};
use crate::cli_auth;
use crate::popover::{Anchor, AnchorSlot, Menu, MenuAction, MenuEvent, MenuItem, menu_popover};
use crate::{
    settings::{
        SettingsView,
        data::McpServer,
        management::Target,
        widgets::{note, secondary_button},
    },
    theme::*,
};
use gpui::{
    AnyElement, App, Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use monocode_core::{accounts::Profile, provider::Kind};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// A sign-in that is running. Dropping it (closing Settings, or replacing it) kills the command.
pub(in crate::settings) struct SignIn {
    project: String,
    provider: String,
    name: String,
    scope: String,
    /// The account's label, shown when there is more than one account.
    account_label: Option<String>,
    url: Option<String>,
    cancelling: bool,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
}

impl Drop for SignIn {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// How the last sign-in ended, shown under the filter bar.
pub(in crate::settings) struct SignInResult {
    kind: ResultKind,
    message: String,
    details: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResultKind {
    Ok,
    Cancelled,
    Failed,
}

impl SignIn {
    fn is_for(&self, project: &str, server: &super::super::data::McpServer) -> bool {
        self.project == project
            && self.provider == server.provider
            && self.name == server.name
            && self.scope == server.scope
    }

    /// The status line, the Open browser button (once a URL is known) and Cancel.
    fn controls(&self, key: &str, cx: &mut Context<SettingsView>) -> gpui::Div {
        let status = match (&self.url, self.cancelling) {
            (_, true) => "Cancelling…".to_owned(),
            (Some(url), _) => format!(
                "Waiting for sign-in in your browser… ({})",
                cli_auth::url_host(url)
            ),
            (None, _) => "Starting sign-in…".to_owned(),
        };
        let status = match &self.account_label {
            Some(account) => format!("{account}: {status}"),
            None => status,
        };
        let cancelling = self.cancelling;
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .child(
                div()
                    .id(SharedString::from(format!("mcp-progress-{key}")))
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(content(0.6))
                    .child(status),
            )
            .when_some(self.url.clone(), |d, url| {
                d.child(
                    secondary_button(
                        SharedString::from(format!("mcp-open-browser-{key}")),
                        "Open browser",
                        false,
                        false,
                    )
                    .child("Open browser")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .child(
                secondary_button(
                    SharedString::from(format!("mcp-cancel-{key}")),
                    "Cancel",
                    false,
                    cancelling,
                )
                .child("Cancel")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.cancel_sign_in();
                    cx.notify();
                })),
            )
    }
}

/// The open "more sign-in actions" menu of a row.
pub(in crate::settings) struct McpMenu {
    menu: Entity<Menu>,
    anchor: AnchorSlot,
    _events: Subscription,
}

enum Msg {
    Event(cli_auth::Event),
    Done(cli_auth::Outcome),
}

/// "Sign in for all accounts" for one server: first each account's own status is read, then the
/// accounts that need it sign in one after the other.
pub(in crate::settings) struct Batch {
    token: u64,
    project: String,
    server: McpServer,
    checking: bool,
    queue: VecDeque<(String, String)>,
    current: Option<String>,
    done: Vec<(String, Done)>,
    /// Reading the accounts' status; dropping the batch stops it.
    _task: Task<()>,
}

impl SettingsView {
    /// The Claude account the page shows, as a profile: the explicit choice, else the account new
    /// sessions in this project use. `None` only when there is no account store (tests).
    pub(in crate::settings) fn mcp_profile(&self, cx: &App) -> Option<Profile> {
        let center = crate::accounts::get(cx)?;
        let center = center.read(cx);
        let id = mcp_row::effective_account(
            &center.book,
            &self.selected_project,
            self.mcp_account.as_deref(),
        );
        Some(center.profile(&id))
    }

    fn mcp_menu_anchor(&self, ix: usize) -> AnchorSlot {
        self.mcp_menu_anchors
            .borrow_mut()
            .entry(ix)
            .or_default()
            .clone()
    }

    /// The row menu: sign in again, and sign in for all accounts.
    fn open_mcp_menu(
        &mut self,
        server: McpServer,
        multiple: bool,
        anchor: AnchorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut items: Vec<MenuItem> = vec![MenuAction::new("again", "Sign in again").into()];
        if multiple {
            items.push(MenuAction::new("all", "Sign in for all accounts").into());
        }
        let menu = cx.new(|cx| Menu::new("Sign-in actions", items, window, cx).width(220.));
        let events = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            this.mcp_menu = None;
            if let MenuEvent::Picked(id, _) = event {
                match id.as_ref() {
                    "again" => this.mcp_action(server.clone(), false, cx),
                    "all" => this.start_sign_in_all(server.clone(), cx),
                    _ => {}
                }
            }
            cx.notify();
        });
        self.mcp_menu = Some(McpMenu {
            menu,
            anchor,
            _events: events,
        });
        cx.notify();
    }

    pub(in crate::settings) fn mcp_overlay(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.mcp_menu
            .iter()
            .map(|open| {
                menu_popover(&open.menu, Anchor::Slot(open.anchor.clone()), cx)
                    .gap(4.)
                    .into_any_element()
            })
            .collect()
    }

    /// Every Claude account with its label.
    fn mcp_accounts(&self, cx: &App) -> Vec<(String, String, Profile)> {
        let Some(center) = crate::accounts::get(cx) else {
            return Vec::new();
        };
        let center = center.read(cx);
        center
            .book
            .list(Kind::Claude)
            .into_iter()
            .map(|a| {
                let profile = center.profile(&a.id);
                (a.id, a.label, profile)
            })
            .collect()
    }

    fn mcp_account_label(&self, id: &str, cx: &App) -> Option<String> {
        self.mcp_accounts(cx)
            .into_iter()
            .find(|(account, _, _)| account == id)
            .map(|(_, label, _)| label)
    }

    /// The account picker, only when there is more than one Claude account.
    fn mcp_account_picker(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let accounts = self.mcp_accounts(cx);
        if accounts.len() < 2 {
            return None;
        }
        let center = crate::accounts::get(cx)?;
        let (used, value) = {
            let center = center.read(cx);
            (
                center.book.selected(Kind::Claude, &self.selected_project),
                mcp_row::effective_account(
                    &center.book,
                    &self.selected_project,
                    self.mcp_account.as_deref(),
                ),
            )
        };
        let options = accounts
            .into_iter()
            .map(|(id, label, _)| {
                let text = if id == used {
                    format!("{label} (used by new sessions)")
                } else {
                    label
                };
                (id, SharedString::from(text), None)
            })
            .collect();
        Some(self.select(
            "settings-mcp-account",
            "Claude account",
            &value,
            options,
            None,
            cx,
            |this, value, cx| {
                this.cancel_sign_in();
                this.mcp_result = None;
                this.mcp_account = Some(value);
                this.refresh(cx);
            },
        ))
    }
}

impl SettingsView {
    pub(super) fn mcp_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let project = self.project_picker(cx);
        let account_picker = self.mcp_account_picker(cx);
        let account_count = self.mcp_accounts(cx).len();
        let account_label = self
            .mcp_profile(cx)
            .and_then(|p| self.mcp_account_label(&p.id, cx))
            .unwrap_or_else(|| "this account".into());
        let busy = self.mcp_busy || self.management.as_ref().is_some_and(|e| e.busy);
        let mut page = vec![
            div()
                .relative()
                .child(self.reveal_probe("mcp-servers"))
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("MCP connections"),
                                )
                                .child(project)
                                .children(account_picker),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_size(px(12.))
                                .text_color(content(0.55))
                                .child(
                                    "Servers for this project. Claude Code servers, sign-in and \
                                     approval are kept per Claude account.",
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(self.refresh_button("refresh-mcp", "Refresh", cx))
                        .child(
                            secondary_button(
                                "mcp-show-all",
                                if self.mcp_show_all {
                                    "Show available providers"
                                } else {
                                    "Show all providers"
                                },
                                false,
                                false,
                            )
                            .child(icon("filter", 14.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.mcp_show_all = !this.mcp_show_all;
                                cx.notify();
                            })),
                        )
                        .child(
                            secondary_button("add-mcp", "Add MCP server", false, busy)
                                .debug_selector(|| "settings-add-mcp".into())
                                .child(icon("plus", 14.))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if !busy {
                                        this.open_management(Target::Mcp, window, cx);
                                    }
                                })),
                        ),
                )
                .into_any_element(),
        ];
        let filter = self.mcp_filter.clone();
        let counts = std::iter::once(("all", "All"))
            .chain(
                [
                    ("claude", "Claude Code"),
                    ("claude_desktop", "Claude Desktop"),
                    ("codex", "Codex"),
                    ("cursor", "Cursor"),
                    ("opencode", "OpenCode"),
                ]
                .into_iter()
                .filter(|(p, _)| {
                    self.mcp_show_all || self.discovery.mcp.iter().any(|s| s.provider == *p)
                }),
            )
            .map(|(id, label)| {
                let count = self
                    .discovery
                    .mcp
                    .iter()
                    .filter(|s| id == "all" || s.provider == id)
                    .count();
                (id, format!("{label} {count}"))
            })
            .collect::<Vec<_>>();
        let options = counts
            .iter()
            .map(|(id, label)| (*id, label.as_str()))
            .collect::<Vec<_>>();
        page.push(
            div()
                .mt_6()
                .child(self.segmented(
                    "mcp-filter",
                    "Filter MCP servers by provider",
                    &filter,
                    &options,
                    cx,
                    |this, v, cx| {
                        this.mcp_filter = v.into();
                        cx.notify();
                    },
                ))
                .into_any_element(),
        );
        if let Some(result) = &self.mcp_result {
            let failed = result.kind == ResultKind::Failed;
            let mut block = div()
                .id(if failed {
                    "settings-mcp-signin-error"
                } else {
                    "settings-mcp-note"
                })
                .mt_3()
                .text_size(px(12.))
                .text_color(if failed {
                    gpui::rgb(0xf87171).into()
                } else {
                    content(0.6)
                })
                .when(failed, |b| b.role(gpui::Role::Alert));
            block = block.child(result.message.clone());
            if let Some(details) = &result.details {
                let open = self.mcp_details_open;
                block = block
                    .child(
                        div().mt_2().child(
                            secondary_button(
                                "mcp-signin-details",
                                if open { "Hide details" } else { "Show details" },
                                false,
                                false,
                            )
                            .child(if open { "Hide details" } else { "Show details" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.mcp_details_open = !this.mcp_details_open;
                                cx.notify();
                            })),
                        ),
                    )
                    .when(open, |b| {
                        b.child(
                            div()
                                .id("mcp-signin-details-text")
                                .mt_2()
                                .p_3()
                                .rounded_lg()
                                .bg(content(0.05))
                                .text_color(content(0.6))
                                .text_size(px(11.))
                                .children(details.lines().map(|l| div().child(l.to_owned()))),
                        )
                    });
            }
            page.push(block.into_any_element());
        }
        if let Some(error) = &self.discovery.mcp_error {
            page.push(
                div()
                    .id("settings-page-error")
                    .role(gpui::Role::Alert)
                    .mt_3()
                    .text_size(px(12.))
                    .text_color(gpui::rgb(0xf87171))
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        // A running sign-in always keeps its Cancel reachable, even when its row is hidden by the
        // filter or the list is reloading.
        if let Some(state) = &self.mcp_signin {
            let shown = !self.discovery_loading
                && self.discovery.mcp.iter().any(|s| {
                    (filter == "all" || filter == s.provider)
                        && state.is_for(&self.selected_project, s)
                });
            if !shown {
                page.push(
                    div()
                        .mt_3()
                        .child(state.controls("page", cx))
                        .into_any_element(),
                );
            }
        }
        if self.mcp_batch.as_ref().is_some_and(|b| b.checking) {
            page.push(
                div()
                    .mt_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.6))
                            .child("Checking each account…"),
                    )
                    .child(
                        secondary_button("mcp-cancel-check", "Cancel", false, false)
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_sign_in();
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if self.discovery_loading {
            page.push(note("Checking servers…"));
            return page;
        }
        let servers = self
            .discovery
            .mcp
            .iter()
            .filter(|s| filter == "all" || filter == s.provider)
            .cloned()
            .collect::<Vec<_>>();
        if servers.is_empty() {
            page.push(note("No MCP servers configured for this provider."));
        } else {
            page.push(
                div()
                    .mt_6()
                    .overflow_hidden()
                    .rounded_xl()
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.03))
                    .children(servers.into_iter().enumerate().map(|(ix, server)| {
                        let mut row = div()
                            .id(SharedString::from(format!("mcp-server-{ix}")))
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_3()
                            .px_4()
                            .py(px(14.))
                            .border_b_1()
                            .border_color(content(0.05))
                            .child(provider_icon(
                                if server.provider == "claude_desktop" {
                                    "claude"
                                } else {
                                    &server.provider
                                },
                                16.,
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child(server.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_size(px(12.))
                                            .text_color(content(0.45))
                                            .child(format!(
                                                "{} · {} · {} · {}",
                                                if server.provider == "claude_desktop" {
                                                    "Claude Desktop"
                                                } else {
                                                    monocode_core::provider::Kind::from_id(
                                                        &server.provider,
                                                    )
                                                    .label()
                                                },
                                                server.scope,
                                                server.transport,
                                                server.status
                                            )),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(11.))
                                            .text_color(content(0.35))
                                            .child(server.path.display().to_string()),
                                    ),
                            );
                        let view = mcp_row::row_view(&server);
                        if let Some(text) = view.note {
                            row = row.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(content(0.55))
                                    .child(text),
                            );
                        }
                        let signing = self
                            .mcp_signin
                            .as_ref()
                            .filter(|s| s.is_for(&self.selected_project, &server));
                        let sign_in_button = |id: String, idle: &'static str| {
                            let label = if signing.is_some() {
                                "Signing in…"
                            } else {
                                idle
                            };
                            let login = server.clone();
                            secondary_button(SharedString::from(id), label, false, busy)
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.mcp_action(login.clone(), false, cx)
                                }))
                        };
                        match view.primary {
                            Primary::SignIn => {
                                row =
                                    row.child(sign_in_button(format!("mcp-login-{ix}"), "Sign in"));
                            }
                            Primary::Approve => {
                                let target = server.clone();
                                row = row.child(
                                    secondary_button(
                                        SharedString::from(format!("mcp-approve-{ix}")),
                                        "Approve",
                                        false,
                                        busy,
                                    )
                                    .child("Approve")
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.mcp_approve(target.clone(), window, cx)
                                        },
                                    )),
                                );
                            }
                            Primary::AddToAccount => {
                                let target = server.clone();
                                let label = format!("Add to {account_label}");
                                row = row.child(
                                    secondary_button(
                                        SharedString::from(format!("mcp-add-account-{ix}")),
                                        label.clone(),
                                        false,
                                        busy,
                                    )
                                    .child(label)
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.mcp_add_to_account(target.clone(), window, cx)
                                        },
                                    )),
                                );
                            }
                            Primary::None => {}
                        }
                        let multiple = account_count > 1;
                        if view.all_accounts && multiple && view.primary == Primary::SignIn {
                            let target = server.clone();
                            row = row.child(
                                secondary_button(
                                    SharedString::from(format!("mcp-login-all-{ix}")),
                                    "Sign in for all accounts",
                                    false,
                                    busy,
                                )
                                .child("Sign in for all accounts")
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.start_sign_in_all(target.clone(), cx)
                                    },
                                )),
                            );
                        }
                        if view.menu {
                            let target = server.clone();
                            let anchor = self.mcp_menu_anchor(ix);
                            let trigger = anchor.clone();
                            row = row.child(
                                secondary_button(
                                    SharedString::from(format!("mcp-menu-{ix}")),
                                    "More sign-in actions",
                                    false,
                                    busy,
                                )
                                .relative()
                                .child(anchor.probe())
                                .child(icon("more-horizontal", 14.))
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if !busy {
                                            this.open_mcp_menu(
                                                target.clone(),
                                                multiple,
                                                trigger.clone(),
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            );
                        }
                        if let Some(state) = signing {
                            row = row.child(state.controls(&ix.to_string(), cx));
                        }
                        if server.only_cli {
                            // Not in this account: there is nothing to remove or show here.
                        } else if server.provider == "claude" {
                            let mut remove = server.clone();
                            if server.path.as_os_str().is_empty() {
                                let name = server.name.clone();
                                let scope = self
                                    .mcp_remove_scopes
                                    .get(&name)
                                    .cloned()
                                    .unwrap_or_else(|| "local".into());
                                remove.scope = scope.clone();
                                row = row.child(self.segmented(
                                    "mcp-remove-scope",
                                    "Scope to remove this server from",
                                    &scope,
                                    &[("local", "Local"), ("project", "Project"), ("user", "User")],
                                    cx,
                                    move |this, value, cx| {
                                        this.mcp_remove_scopes.insert(name.clone(), value.into());
                                        cx.notify();
                                    },
                                ));
                            }
                            row = row.child(
                                secondary_button(
                                    SharedString::from(format!("mcp-remove-{ix}")),
                                    "Remove",
                                    false,
                                    busy,
                                )
                                .child("Remove")
                                .on_click(cx.listener(
                                    move |_, _, window, cx| {
                                        let prompt = window.prompt(
                                            gpui::PromptLevel::Warning,
                                            "Remove MCP server",
                                            Some(&format!(
                                                "Remove {} from {} scope?",
                                                remove.name, remove.scope
                                            )),
                                            &["Remove", "Cancel"],
                                            cx,
                                        );
                                        let server = remove.clone();
                                        cx.spawn(async move |this, cx| {
                                            if prompt.await == Ok(0) {
                                                let _ = this.update(cx, |this, cx| {
                                                    this.mcp_action(server, true, cx)
                                                });
                                            }
                                        })
                                        .detach();
                                    },
                                )),
                            );
                        } else {
                            let path = server.path;
                            row = row.child(
                                secondary_button(
                                    SharedString::from(format!("mcp-show-{ix}")),
                                    "Show config",
                                    false,
                                    false,
                                )
                                .child("Show config")
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                            );
                        }
                        row
                    }))
                    .into_any_element(),
            );
        }
        page.push(note("Live connection status is available for Claude Code."));
        page
    }
    pub(crate) fn refresh_mcp_health(&mut self, cx: &mut Context<Self>) {
        let cwd = self.selected_project.clone();
        // A result belongs to one (project, account); a late one for another is ignored.
        let account = self.mcp_profile(cx);
        let expected = (cwd.clone(), account.as_ref().map(|p| p.id.clone()));
        let work = cx.background_executor().spawn(async move {
            match &account {
                Some(account) => monocode_core::mcp::claude_health_for(&cwd, account),
                None => monocode_core::mcp::claude_health(&cwd),
            }
        });
        self.mcp_health = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ =
                this.update(cx, |this, cx| {
                    let current = (
                        this.selected_project.clone(),
                        this.mcp_profile(cx).map(|p| p.id),
                    );
                    if current != expected {
                        return;
                    }
                    match result {
                        Ok(output) => {
                            for (name, status) in monocode_core::mcp::parse_claude_health(&output) {
                                let name = name.as_str();
                                let status = status.as_str();
                                let http = output.lines().filter_map(|l| l.split_once(": ")).any(
                                    |(n, d)| n.trim() == name && d.to_lowercase().contains("http"),
                                );
                                let mut found = false;
                                for server in &mut this.discovery.mcp {
                                    if server.provider == "claude"
                                        && server.name == name
                                        && !server.only_cli
                                    {
                                        if server.status != "Disabled" {
                                            server.status = status.into();
                                        }
                                        found = true;
                                    }
                                }
                                if !found {
                                    this.discovery.mcp.push(McpServer {
                                        name: name.into(),
                                        provider: "claude".into(),
                                        scope: "local".into(),
                                        path: Default::default(),
                                        transport: if http { "http" } else { "" }.into(),
                                        status: status.into(),
                                        auth: if http {
                                            monocode_core::mcp::AuthKind::Oauth
                                        } else {
                                            monocode_core::mcp::AuthKind::None
                                        },
                                        only_cli: false,
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            this.discovery.mcp_error =
                                Some(format!("Claude connection status unavailable: {e}"))
                        }
                    }
                    cx.notify();
                });
        }));
    }
    /// Asks the running sign-in (if any) to stop; the runner kills the process tree and reports.
    pub(in crate::settings) fn cancel_sign_in(&mut self) {
        if let Some(state) = &mut self.mcp_signin {
            state.cancelling = true;
            state.cancel.store(true, Ordering::SeqCst);
        } else if self.mcp_batch.as_ref().is_some_and(|b| b.checking) {
            // Only reading the accounts' status so far: drop the run.
            self.mcp_batch = None;
            self.mcp_busy = false;
        }
    }

    fn mcp_action(&mut self, server: McpServer, remove: bool, cx: &mut Context<Self>) {
        if self.mcp_busy {
            return;
        }
        self.mcp_busy = true;
        self.mcp_result = None;
        self.mcp_details_open = false;
        let cwd = self.selected_project.clone();
        let expected = cwd.clone();
        let account = self.mcp_profile(cx);
        if !remove {
            let label = self.mcp_label_if_many(account.as_ref(), cx);
            self.start_sign_in(server, cwd, account, label, cx);
            cx.notify();
            return;
        }
        let work = cx.background_executor().spawn(async move {
            monocode_core::mcp::remove(&cwd, &server.name, &server.scope, account.as_ref())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.mcp_busy = false;
                if this.selected_project != expected {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(()) => this.refresh(cx),
                    Err(e) => this.discovery.mcp_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The account's label for messages, when there is more than one account to tell apart.
    fn mcp_label_if_many(&self, account: Option<&Profile>, cx: &App) -> Option<String> {
        let accounts = self.mcp_accounts(cx);
        if accounts.len() < 2 {
            return None;
        }
        let id = &account?.id;
        accounts
            .into_iter()
            .find(|(account, _, _)| account == id)
            .map(|(_, label, _)| label)
    }

    /// Asks before approving, then approves for the shown account. Only a click gets here.
    fn mcp_approve(&mut self, server: McpServer, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.mcp_profile(cx) else {
            return;
        };
        let label = self
            .mcp_account_label(&account.id, cx)
            .unwrap_or_else(|| "this account".into());
        let folder = self.selected_project.clone();
        let undo = if server.status == mcp_row::STATUS_REJECTED {
            " It was rejected before; this un-rejects it."
        } else {
            ""
        };
        let prompt = window.prompt(
            gpui::PromptLevel::Warning,
            &format!("Approve {} for {label}", server.name),
            Some(&format!(
                "Approve {} for {label}? This also marks {folder} (its main working tree, if \
                 this is a git worktree) as trusted for {label}, like accepting Claude's trust \
                 prompt.{undo} Running Claude sessions for {label} must be restarted to pick up \
                 the approval.",
                server.name
            )),
            &["Approve", "Cancel"],
            cx,
        );
        let expected = (folder, account.id.clone());
        cx.spawn(async move |this, cx| {
            if prompt.await == Ok(0) {
                let _ = this.update(cx, |this, cx| {
                    let same = this.selected_project == expected.0
                        && this.mcp_profile(cx).is_some_and(|p| p.id == expected.1);
                    if same {
                        this.run_approve(server, account, label, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn run_approve(
        &mut self,
        server: McpServer,
        account: Profile,
        label: String,
        cx: &mut Context<Self>,
    ) {
        if self.mcp_busy {
            return;
        }
        self.mcp_busy = true;
        self.mcp_result = None;
        self.mcp_details_open = false;
        let cwd = self.selected_project.clone();
        let expected = cwd.clone();
        let name = server.name.clone();
        let work = cx
            .background_executor()
            .spawn(async move { monocode_core::mcp::approve_for(&cwd, &account, &server.name) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.mcp_busy = false;
                if this.selected_project != expected {
                    cx.notify();
                    return;
                }
                this.mcp_result = Some(match result {
                    Ok(()) => SignInResult {
                        kind: ResultKind::Ok,
                        message: format!(
                            "Approved \"{name}\" for {label}. This project is now trusted for {label}."
                        ),
                        details: None,
                    },
                    Err(e) => SignInResult {
                        kind: ResultKind::Failed,
                        message: format!("Could not approve \"{name}\" for {label}."),
                        details: Some(e),
                    },
                });
                this.mcp_details_open = false;
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Copies a server from the CLI login into the shown account. A server with a key in its
    /// config asks first, because the key is copied with it.
    fn mcp_add_to_account(
        &mut self,
        server: McpServer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(account) = self.mcp_profile(cx) else {
            return;
        };
        let label = self
            .mcp_account_label(&account.id, cx)
            .unwrap_or_else(|| "this account".into());
        if server.auth != monocode_core::mcp::AuthKind::Header {
            self.run_add_to_account(server, account, label, cx);
            return;
        }
        let prompt = window.prompt(
            gpui::PromptLevel::Warning,
            &format!("Add {} to {label}", server.name),
            Some(&format!(
                "{} uses a key stored in its config. Adding it copies that config, including the \
                 key, from the CLI login to {label}.",
                server.name
            )),
            &["Add", "Cancel"],
            cx,
        );
        let expected = (self.selected_project.clone(), account.id.clone());
        cx.spawn(async move |this, cx| {
            if prompt.await == Ok(0) {
                let _ = this.update(cx, |this, cx| {
                    let same = this.selected_project == expected.0
                        && this.mcp_profile(cx).is_some_and(|p| p.id == expected.1);
                    if same {
                        this.run_add_to_account(server, account, label, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn run_add_to_account(
        &mut self,
        server: McpServer,
        account: Profile,
        label: String,
        cx: &mut Context<Self>,
    ) {
        if self.mcp_busy {
            return;
        }
        self.mcp_busy = true;
        self.mcp_result = None;
        self.mcp_details_open = false;
        let cwd = self.selected_project.clone();
        let expected = cwd.clone();
        let (name, scope) = (server.name.clone(), server.scope.clone());
        let oauth = server.auth == monocode_core::mcp::AuthKind::Oauth;
        let work = cx.background_executor().spawn(async move {
            monocode_core::mcp::add_from_cli_login(&cwd, &account, &server.name, &server.scope)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.mcp_busy = false;
                if this.selected_project != expected {
                    cx.notify();
                    return;
                }
                this.mcp_result = Some(match result {
                    Ok(()) => SignInResult {
                        kind: ResultKind::Ok,
                        message: format!(
                            "Added \"{name}\" ({scope} scope) to {label}.{}",
                            if oauth {
                                " Sign in for this account next."
                            } else {
                                ""
                            }
                        ),
                        details: None,
                    },
                    Err(e) => SignInResult {
                        kind: ResultKind::Failed,
                        message: format!("Could not add \"{name}\" to {label}."),
                        details: mcp_row::safe_details(&e),
                    },
                });
                this.mcp_details_open = false;
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Signs in to a server for every Claude account that needs it, one after the other.
    fn start_sign_in_all(&mut self, server: McpServer, cx: &mut Context<Self>) {
        if self.mcp_busy {
            return;
        }
        self.mcp_busy = true;
        self.mcp_result = None;
        self.mcp_details_open = false;
        self.mcp_batch_token += 1;
        let token = self.mcp_batch_token;
        let cwd = self.selected_project.clone();
        let accounts = self.mcp_accounts(cx);
        let name = server.name.clone();
        // One check per account, all running at once.
        let checks: Vec<_> = accounts
            .into_iter()
            .map(|(id, label, profile)| {
                let (cwd, name) = (cwd.clone(), name.clone());
                cx.background_executor().spawn(async move {
                    let status =
                        monocode_core::mcp::claude_health_for(&cwd, &profile).map(|text| {
                            monocode_core::mcp::parse_claude_health(&text)
                                .into_iter()
                                .find(|(n, _)| *n == name)
                                .map(|(_, status)| status)
                        });
                    (id, label, status)
                })
            })
            .collect();
        let task = cx.spawn(async move |this, cx| {
            let mut checked = Vec::new();
            for check in checks {
                checked.push(check.await);
            }
            let _ = this.update(cx, |this, cx| {
                let Some(batch) = this.mcp_batch.as_mut().filter(|b| b.token == token) else {
                    return;
                };
                batch.checking = false;
                for (id, label, status) in checked {
                    match status {
                        Ok(status) => match mcp_row::plan_step(status.as_deref()) {
                            Step::SignIn => batch.queue.push_back((id, label)),
                            other => batch.done.push((label, other.into())),
                        },
                        Err(e) => batch.done.push((
                            label,
                            Done::Failed(
                                mcp_row::safe_details(&e)
                                    .and_then(|d| d.lines().next().map(str::to_owned))
                                    .unwrap_or_else(|| "its status could not be read".into()),
                            ),
                        )),
                    }
                }
                this.next_in_batch(cx);
                cx.notify();
            });
        });
        self.mcp_batch = Some(Batch {
            token,
            project: self.selected_project.clone(),
            server,
            checking: true,
            queue: VecDeque::new(),
            current: None,
            done: Vec::new(),
            _task: task,
        });
        cx.notify();
    }

    /// Starts the next account's sign-in, or ends the run when none is left.
    fn next_in_batch(&mut self, cx: &mut Context<Self>) {
        let Some(batch) = &mut self.mcp_batch else {
            return;
        };
        let Some((id, label)) = batch.queue.pop_front() else {
            self.finish_batch(cx);
            return;
        };
        batch.current = Some(label.clone());
        let server = batch.server.clone();
        let project = batch.project.clone();
        let profile = self
            .mcp_accounts(cx)
            .into_iter()
            .find(|(account, _, _)| *account == id)
            .map(|(_, _, profile)| profile);
        let Some(profile) = profile else {
            // Removed while the run was going: never sign in with the inherited environment.
            if let Some(batch) = &mut self.mcp_batch {
                batch.current = None;
                batch
                    .done
                    .push((label, Done::Failed("the account was removed".into())));
            }
            self.next_in_batch(cx);
            return;
        };
        self.start_sign_in(server, project, Some(profile), Some(label), cx);
    }

    fn finish_batch(&mut self, cx: &mut Context<Self>) {
        let Some(batch) = self.mcp_batch.take() else {
            return;
        };
        self.mcp_busy = false;
        let (message, details) = mcp_row::batch_summary(&batch.server.name, &batch.done);
        let failed = batch.done.iter().any(|(_, d)| matches!(d, Done::Failed(_)));
        let cancelled = batch.done.iter().any(|(_, d)| matches!(d, Done::Cancelled));
        self.mcp_result = Some(SignInResult {
            kind: if failed {
                ResultKind::Failed
            } else if cancelled {
                ResultKind::Cancelled
            } else {
                ResultKind::Ok
            },
            message,
            details: Some(details),
        });
        self.mcp_details_open = true;
        // Which accounts are signed in may have changed.
        self.refresh(cx);
    }

    /// Runs the provider's sign-in on a pseudo-terminal and follows it until it ends. `account` is
    /// the Claude account it signs in for; its config dir is what the CLI stores the sign-in in.
    fn start_sign_in(
        &mut self,
        server: McpServer,
        cwd: String,
        account: Option<Profile>,
        account_label: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let expected = cwd.clone();
        let expected_project = cwd.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = async_channel::unbounded::<Msg>();
        let flag = cancel.clone();
        let (provider, name) = (server.provider.clone(), server.name.clone());
        // A dedicated thread, not an executor worker: workers block signals and the CLI would
        // inherit that mask, and a sign-in can take minutes.
        let done = tx.clone();
        let started = std::thread::Builder::new()
            .name("mcp-sign-in".into())
            .spawn(move || {
                let outcome = match monocode_core::mcp::login_command(
                    &cwd,
                    &provider,
                    &name,
                    account.as_ref(),
                ) {
                    Ok(login) => {
                        let spec = cli_auth::Spec {
                            binary: login.binary,
                            args: login.args,
                            cwd: login.cwd,
                            provider,
                            timeout: cli_auth::SIGN_IN_TIMEOUT,
                            fallback_delay: cli_auth::FALLBACK_DELAY,
                            env: login
                                .env
                                .set
                                .into_iter()
                                .map(|(k, v)| (k, v.to_string_lossy().into_owned()))
                                .collect(),
                            remove: login.env.remove,
                        };
                        cli_auth::run(&spec, &flag, |event| {
                            let _ = tx.send_blocking(Msg::Event(event));
                        })
                    }
                    Err(e) => cli_auth::Outcome::Start(e),
                };
                let _ = tx.send_blocking(Msg::Done(outcome));
            });
        if let Err(e) = started {
            let _ = done.send_blocking(Msg::Done(cli_auth::Outcome::Start(e.to_string())));
        }
        let task = cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let done = matches!(msg, Msg::Done(_));
                let alive = this
                    .update(cx, |this, cx| this.sign_in_message(msg, &expected, cx))
                    .is_ok();
                if done || !alive {
                    break;
                }
            }
        });
        self.mcp_signin = Some(SignIn {
            project: expected_project,
            provider: server.provider,
            name: server.name,
            scope: server.scope,
            account_label,
            url: None,
            cancelling: false,
            cancel,
            task: Some(task),
        });
    }

    fn sign_in_message(&mut self, msg: Msg, expected: &str, cx: &mut Context<Self>) {
        match msg {
            Msg::Event(cli_auth::Event::Url(url)) => {
                if let Some(state) = &mut self.mcp_signin {
                    state.url = Some(url);
                }
            }
            Msg::Event(cli_auth::Event::Open(url)) => {
                // Nothing opens after Cancel, or for a sign-in that is no longer shown.
                if self.mcp_signin.as_ref().is_some_and(|s| !s.cancelling) {
                    cx.open_url(&url);
                }
            }
            Msg::Done(outcome) => {
                let Some(mut state) = self.mcp_signin.take() else {
                    self.mcp_busy = false;
                    self.mcp_batch = None;
                    return;
                };
                // The task is the one running this; let it finish instead of dropping it.
                if let Some(task) = state.task.take() {
                    task.detach();
                }
                if self.selected_project != expected {
                    self.mcp_busy = false;
                    self.mcp_batch = None;
                    cx.notify();
                    return;
                }
                let message = mcp_row::sign_in_message(
                    &state.name,
                    &state.provider,
                    state.account_label.as_deref(),
                    &outcome,
                );
                if let Some(batch) = &mut self.mcp_batch {
                    let label = batch.current.take().unwrap_or_default();
                    let cancelled = matches!(outcome, cli_auth::Outcome::Cancelled);
                    batch.done.push((
                        label,
                        match outcome {
                            cli_auth::Outcome::Success => Done::SignedIn,
                            cli_auth::Outcome::Cancelled => Done::Cancelled,
                            _ => Done::Failed(message),
                        },
                    ));
                    if cancelled {
                        // Stopping one sign-in stops the run; the rest are reported as cancelled.
                        let rest: Vec<_> = batch.queue.drain(..).collect();
                        batch
                            .done
                            .extend(rest.into_iter().map(|(_, label)| (label, Done::Cancelled)));
                    }
                    self.next_in_batch(cx);
                    cx.notify();
                    return;
                }
                self.mcp_busy = false;
                self.mcp_result = Some(SignInResult {
                    kind: match outcome {
                        cli_auth::Outcome::Success => ResultKind::Ok,
                        cli_auth::Outcome::Cancelled => ResultKind::Cancelled,
                        _ => ResultKind::Failed,
                    },
                    message,
                    details: cli_auth::details(&outcome),
                });
                self.mcp_details_open = false;
                // The status may have changed whatever happened (a half-finished sign-in too).
                self.refresh(cx);
            }
        }
        cx.notify();
    }
}
