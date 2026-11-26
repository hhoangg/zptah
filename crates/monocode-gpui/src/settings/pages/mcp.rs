use crate::cli_auth;
use crate::{
    settings::{
        SettingsView,
        management::Target,
        widgets::{note, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, SharedString, Task, div, prelude::*, px};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A sign-in that is running. Dropping it (closing Settings, or replacing it) kills the command.
pub(in crate::settings) struct SignIn {
    project: String,
    provider: String,
    name: String,
    scope: String,
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

enum Msg {
    Event(cli_auth::Event),
    Done(cli_auth::Outcome),
}

impl SettingsView {
    pub(super) fn mcp_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let project = self.project_picker(cx);
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
                                .child(project),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_size(px(12.))
                                .text_color(content(0.55))
                                .child("Servers for this project and your accounts."),
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
                        if server.provider != "claude_desktop"
                            && !server.transport.is_empty()
                            && !matches!(server.transport.as_str(), "stdio" | "local" | "ws")
                        {
                            let login = server.clone();
                            let signing = self
                                .mcp_signin
                                .as_ref()
                                .filter(|s| s.is_for(&self.selected_project, &server));
                            let label = if signing.is_some() {
                                "Signing in…"
                            } else {
                                "Sign in"
                            };
                            row = row.child(
                                secondary_button(
                                    SharedString::from(format!("mcp-login-{ix}")),
                                    label,
                                    false,
                                    busy,
                                )
                                .child(label)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.mcp_action(login.clone(), false, cx),
                                )),
                            );
                            if let Some(state) = signing {
                                row = row.child(state.controls(&ix.to_string(), cx));
                            }
                        }
                        if server.provider == "claude" {
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
        let expected = cwd.clone();
        let work = cx
            .background_executor()
            .spawn(async move { monocode_core::mcp::claude_health(&cwd) });
        self.mcp_health = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ =
                this.update(cx, |this, cx| {
                    if this.selected_project != expected {
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
                                    if server.provider == "claude" && server.name == name {
                                        if server.status != "Disabled" {
                                            server.status = status.into();
                                        }
                                        found = true;
                                    }
                                }
                                if !found {
                                    this.discovery.mcp.push(super::super::data::McpServer {
                                        name: name.into(),
                                        provider: "claude".into(),
                                        scope: "local".into(),
                                        path: Default::default(),
                                        transport: if http { "http" } else { "" }.into(),
                                        status: status.into(),
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
        }
    }

    fn mcp_action(
        &mut self,
        server: super::super::data::McpServer,
        remove: bool,
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
        if !remove {
            self.start_sign_in(server, cwd, cx);
            cx.notify();
            return;
        }
        let work = cx
            .background_executor()
            .spawn(async move { monocode_core::mcp::remove(&cwd, &server.name, &server.scope) });
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

    /// Runs the provider's sign-in on a pseudo-terminal and follows it until it ends.
    fn start_sign_in(
        &mut self,
        server: super::super::data::McpServer,
        cwd: String,
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
                let outcome = match monocode_core::mcp::login_command(&cwd, &provider, &name) {
                    Ok((binary, args, dir)) => {
                        let spec = cli_auth::Spec {
                            binary,
                            args,
                            cwd: dir,
                            provider,
                            timeout: cli_auth::SIGN_IN_TIMEOUT,
                            fallback_delay: cli_auth::FALLBACK_DELAY,
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
                    return;
                };
                // The task is the one running this; let it finish instead of dropping it.
                if let Some(task) = state.task.take() {
                    task.detach();
                }
                self.mcp_busy = false;
                if self.selected_project != expected {
                    cx.notify();
                    return;
                }
                self.mcp_result = Some(SignInResult {
                    kind: match outcome {
                        cli_auth::Outcome::Success => ResultKind::Ok,
                        cli_auth::Outcome::Cancelled => ResultKind::Cancelled,
                        _ => ResultKind::Failed,
                    },
                    message: cli_auth::failure_message(&state.name, &state.provider, &outcome),
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
