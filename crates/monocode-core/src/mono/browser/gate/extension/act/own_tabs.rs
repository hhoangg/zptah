//! `browser_tabs` new, close, adopt and release (spec 9.2, 9.3): the tools that change which tabs
//! belong to the Mono. The extension holds the real table and refuses what it must (`E_NOT_OWNER`);
//! the app checks first so the agent gets a plain reason, keeps the audit rows and asks the user
//! where the spec says so (a tab another Mono owns).
//!
//! * `new` opens an unfocused tab in the Mono's group, under the same navigation rules as
//!   `browser_navigate` (origin lists, sensitive sites, soul rules), and stays under
//!   [`tabs::MAX_TABS`] owned tabs.
//! * `close` and `release` work on the Mono's own tabs only; the user's tabs are never closed.
//! * `adopt` needs `user_reference`: words the user wrote in this task (and, for a tab the user
//!   took back, after the stop) that point at exactly one tab. A tab of another Mono also needs the
//!   user's approval. No reference, no adopt, and no prompt takes the place of one.
use super::super::super::super::ownership::{self, How, ASK_THE_USER};
use super::*;

/// What `browser_tabs` says when the Mono is at the limit.
fn limit_text() -> String {
    format!(
        "You already have {} tabs open. Close one of your own tabs first (browser_tabs close).",
        tabs::MAX_TABS
    )
}

impl ExtensionUpstream {
    pub(in super::super) fn own_tabs_tool(
        &self,
        action: &str,
        args: &Value,
    ) -> Result<Value, String> {
        let level = self.act_config().level;
        if level == BrowserLevel::ReadOnly && action != "new" {
            return Err(format!(
                "browser_tabs {action} is not available: this Mono can only read the browser. Tell the user what to do, or that the browser level must be raised."
            ));
        }
        match action {
            "new" => self.tab_new(args),
            "close" => self.tab_close(args),
            "release" => self.tab_release(args),
            _ => self.tab_adopt(args),
        }
    }

    fn instance_id(&self) -> Option<String> {
        (self.link)().map(|l| l.instance_id().to_owned())
    }

    /// Runs `f` on this link's ownership mirror (a scratch one when no link is up).
    fn mirror<R>(&self, f: impl FnOnce(&mut ownership::Table) -> R) -> R {
        match self.instance_id() {
            Some(id) => ownership::with_table(&id, f),
            None => f(&mut ownership::Table::default()),
        }
    }

    /// The tab a call names: `tab` (an id) or `index` (a line of the last list).
    fn named_tab(&self, args: &Value) -> Result<Option<u64>, String> {
        if let Some(value) = args.get("tab").filter(|v| !v.is_null()) {
            return value
                .as_u64()
                .map(Some)
                .ok_or_else(|| "`tab` must be a tab id (the number after tab=).".to_owned());
        }
        if let Some(index) = args["index"].as_u64() {
            return self
                .state()
                .listed
                .get(index as usize)
                .map(|t| Some(t.tab))
                .ok_or_else(|| {
                    format!(
                        "There is no tab {index} in the last list. Call browser_tabs list first."
                    )
                });
        }
        Ok(None)
    }

    /// A fresh list of this Mono's own tabs, and whether `tab` is in it.
    fn owns(&self, tab: u64) -> Result<bool, String> {
        let own = self.refresh(false)?;
        Ok(own
            .iter()
            .any(|t| t.tab == tab && t.owner.as_deref() == Some(&self.mono_id)))
    }

    fn after_change(&self, current: Option<u64>) -> Result<Value, String> {
        let listed = self.refresh(false)?;
        if let Some(tab) = current {
            self.state().selected = Some(tab);
        }
        Ok(self.list_result(&listed, false))
    }

    // ---- new ----

    fn tab_new(&self, args: &Value) -> Result<Value, String> {
        let ctx = self.run_context();
        let url = s(args, "url").ok_or("browser_tabs new needs `url`.")?;
        let parsed = url::Url::parse(url)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .ok_or("Only http and https addresses can be opened.")?;
        self.origins()
            .check(parsed.as_str())
            .map_err(|r| refusal_text(r).to_owned())?;
        let own = self.refresh(false)?;
        if !self.mirror(|t| t.may_open(&self.mono_id)) || own.len() >= tabs::MAX_TABS {
            return Err(limit_text());
        }
        let current = {
            let state = self.state();
            state.selected.and_then(|id| state.known.get(&id).cloned())
        };
        let page_url = current.as_ref().map_or("", |t| t.url.as_str());
        let mut action = TrustAction::new("browser_tabs", args, page_url);
        action.target_url = Some(parsed.as_str());
        let ev = self.with_trust(ctx.session_id, |state, config| {
            state.evaluate(
                &Config {
                    level: config.level,
                    rules: &config.rules,
                },
                &action,
            )
        });
        let summary = format!("new tab {}", audit_url(parsed.as_str()));
        let (decision, approval_id) = match ev.decision.clone() {
            Decision::Deny(reason) => {
                self.write_tab_row(
                    &ctx,
                    &summary,
                    None,
                    parsed.as_str(),
                    &ev,
                    AuditDecision::Blocked,
                    &reason,
                    None,
                );
                return Err(format!("Not allowed: {reason}."));
            }
            Decision::Allow | Decision::AllowByGrant(_) => (AuditDecision::Auto, None),
            Decision::Ask(_) => {
                let mut facts = CardFacts::new(
                    &self.act_config().mono_label,
                    &ev.site,
                    ActionDesc::Navigate,
                    ev.categories.iter().copied().collect(),
                );
                facts.url = parsed.to_string();
                facts.site_kind = site_category(parsed.as_str());
                facts.rule = ev.reason.strip_prefix("soul rule: ").map(str::to_owned);
                facts.purpose = s(args, "purpose").map(str::to_owned);
                match self.ask_card(&ctx, "browser_tabs", None, facts, &ev, None, None) {
                    Ok((answer, id)) => {
                        let granted =
                            self.with_trust(ctx.session_id, |state, _| state.commit(&ev, answer));
                        let decision = if answer == Answer::AllowedTask && !granted.is_empty() {
                            AuditDecision::AskedAllowedTask
                        } else {
                            AuditDecision::AskedAllowedOnce
                        };
                        (decision, Some(id))
                    }
                    Err((text, audit, id)) => {
                        self.write_tab_row(
                            &ctx,
                            &summary,
                            None,
                            parsed.as_str(),
                            &ev,
                            audit,
                            &text,
                            Some(id),
                        );
                        return Err(text);
                    }
                }
            }
        };
        let mut params = json!({"url": parsed.as_str()});
        if let Some(after) = self.state().selected {
            if own.iter().any(|t| t.tab == after) {
                params["after_tab"] = json!(after);
            }
        }
        let result = self
            .ask_raw(Method::TabOpen, params, DEFAULT_DEADLINE_MS)
            .map_err(|e| self.note_tab_error(e, Method::TabOpen))?;
        let info: TabInfo = serde_json::from_value(result)
            .map_err(|_| "The extension sent a tab that could not be read.".to_owned())?;
        self.mirror(|t| {
            let _ = t.claim(info.tab, &self.mono_id, How::Created, crate::time::now_ms());
        });
        self.state().known.insert(info.tab, info.clone());
        self.write_tab_row(
            &ctx,
            &summary,
            Some(&info),
            parsed.as_str(),
            &ev,
            decision,
            "opened",
            approval_id,
        );
        self.after_change(Some(info.tab))
    }

    // ---- close and release ----

    fn own_target(&self, args: &Value, verb: &str) -> Result<u64, String> {
        let tab = self
            .named_tab(args)?
            .or_else(|| self.state().selected)
            .ok_or_else(|| format!("browser_tabs {verb} needs `tab` (a tab id) or `index`."))?;
        if !self.owns(tab)? {
            return Err(format!(
                "Tab {tab} is not yours, so it cannot be {verb}d. Only tabs you opened or adopted can be, and the user's tabs are never closed."
            ));
        }
        Ok(tab)
    }

    fn forget_tab(&self, tab: u64) {
        let mut state = self.state();
        state.known.remove(&tab);
        state.listed.retain(|t| t.tab != tab);
        if state.selected == Some(tab) {
            state.selected = None;
        }
    }

    fn tab_close(&self, args: &Value) -> Result<Value, String> {
        let ctx = self.run_context();
        let tab = self.own_target(args, "close")?;
        let info = self.state().known.get(&tab).cloned();
        self.ask_raw(Method::TabClose, json!({"tab": tab}), DEFAULT_DEADLINE_MS)
            .map_err(|e| self.note_tab_error(e, Method::TabClose))?;
        self.mirror(|t| t.release(tab));
        self.forget_tab(tab);
        self.write_simple_row(&ctx, &format!("close tab {tab}"), info.as_ref());
        self.after_change(None)
    }

    fn tab_release(&self, args: &Value) -> Result<Value, String> {
        let ctx = self.run_context();
        let tab = self.own_target(args, "release")?;
        let info = self.state().known.get(&tab).cloned();
        self.ask_raw(Method::TabRelease, json!({"tab": tab}), DEFAULT_DEADLINE_MS)
            .map_err(|e| self.note_tab_error(e, Method::TabRelease))?;
        self.mirror(|t| t.release(tab));
        self.write_simple_row(&ctx, &format!("release tab {tab}"), info.as_ref());
        self.after_change(None)
    }

    // ---- adopt ----

    fn tab_adopt(&self, args: &Value) -> Result<Value, String> {
        let ctx = self.run_context();
        let quote = s(args, "user_reference").ok_or_else(|| {
            format!("browser_tabs adopt needs `user_reference`: the words the user wrote that name the tab. {ASK_THE_USER}")
        })?;
        let named = self.named_tab(args)?;
        let messages = self.with_trust(ctx.session_id, |state, _| state.user_texts(None));
        ownership::check_reference(quote, &messages).map_err(|e| e.text())?;

        let all = self.refresh(true)?;
        let target = match find(quote, &all) {
            Ok(tab) => tab.clone(),
            Err(e) => return Err(e.text()),
        };
        if let Some(named) = named {
            if named != target.tab {
                return Err(format!(
                    "That reference points at another tab than tab {named}. {ASK_THE_USER}"
                ));
            }
        }
        // A tab the user took back needs words written after the stop.
        let taken_back = self.mirror(|t| t.taken_back_at(target.tab));
        let message_ms = if taken_back.is_some() {
            let later = self.with_trust(ctx.session_id, |state, _| state.user_texts(taken_back));
            ownership::check_reference(quote, &later).map_err(|_| {
                format!("You stopped on that tab and the user took it back. Only words they write after the stop count. {ASK_THE_USER}")
            })?
        } else {
            ownership::check_reference(quote, &messages).map_err(|e| e.text())?
        };
        self.origins()
            .check(&target.url)
            .map_err(|r| refusal_text(r).to_owned())?;
        match target.owner.as_deref() {
            Some(owner) if owner == self.mono_id => {
                return Err(format!("Tab {} is already yours.", target.tab));
            }
            _ => {}
        }

        // The Mono's own tabs stay under the limit whatever way a tab comes in.
        let own = self.refresh(false)?;
        if own.len() >= tabs::MAX_TABS || !self.mirror(|t| t.may_open(&self.mono_id)) {
            return Err(limit_text());
        }

        let summary = format!("adopt tab {}", target.tab);
        let mut approval_id = None;
        let grant = match target.owner.clone() {
            None => json!({"kind": "user_reference", "quote": quote, "message_ms": message_ms}),
            Some(other) => {
                // Another Mono's tab: the reference is not enough, the user must agree.
                let ev = Evaluation {
                    decision: Decision::Ask(Vec::new()),
                    tier: Tier::Always,
                    categories: BTreeSet::new(),
                    site: registrable_domain_of(&target.url).unwrap_or_default(),
                    reason: "tab transfer".into(),
                };
                let from = self
                    .other_mono_label(&other)
                    .unwrap_or_else(|| "another Mono".to_owned());
                let mut facts = CardFacts::new(
                    &self.act_config().mono_label,
                    &ev.site,
                    ActionDesc::TabTransfer {
                        title: target.title.clone(),
                        from,
                    },
                    Vec::new(),
                );
                facts.url = target.url.clone();
                match self.ask_card(
                    &ctx,
                    "browser_tabs",
                    Some(target.tab),
                    facts,
                    &ev,
                    None,
                    None,
                ) {
                    Ok((_, id)) => {
                        approval_id = Some(id.clone());
                        json!({"kind": "user_approved", "approval_id": id, "quote": quote, "message_ms": message_ms})
                    }
                    Err((text, audit, id)) => {
                        self.write_tab_row(
                            &ctx,
                            &summary,
                            Some(&target),
                            &target.url,
                            &ev,
                            audit,
                            &text,
                            Some(id),
                        );
                        return Err(text);
                    }
                }
            }
        };
        let result = self
            .ask_raw(
                Method::TabAdopt,
                json!({"tab": target.tab, "grant": grant}),
                DEFAULT_DEADLINE_MS,
            )
            .map_err(|e| self.note_tab_error(e, Method::TabAdopt))?;
        let info: TabInfo = serde_json::from_value(result)
            .map_err(|_| "The extension sent a tab that could not be read.".to_owned())?;
        let now = crate::time::now_ms();
        self.mirror(|t| t.transfer(info.tab, &self.mono_id, now));
        self.state().known.insert(info.tab, info.clone());
        // The user pointed at this site: it is part of the task now.
        self.with_trust(ctx.session_id, |state, _| state.note_user_domain(&info.url));
        let ev = Evaluation {
            decision: Decision::Allow,
            tier: Tier::Act,
            categories: BTreeSet::new(),
            site: registrable_domain_of(&info.url).unwrap_or_default(),
            reason: "the user named the tab".into(),
        };
        let decision = if approval_id.is_some() {
            AuditDecision::AskedAllowedOnce
        } else {
            AuditDecision::Auto
        };
        self.write_tab_row(
            &ctx,
            &summary,
            Some(&info),
            &info.url,
            &ev,
            decision,
            "adopted",
            approval_id,
        );
        self.after_change(Some(info.tab))
    }

    /// The name another Mono goes by in this process, for the transfer card.
    fn other_mono_label(&self, id: &str) -> Option<String> {
        super::super::super::super::roster::book().label_of(id)
    }

    // ---- bookkeeping ----

    fn note_tab_error(&self, error: RequestError, method: Method) -> String {
        if matches!(&error, RequestError::Remote(b) if b.code == ErrorCode::Stopped) {
            self.note_stopped();
        }
        request_error_text(error, method)
    }

    #[allow(clippy::too_many_arguments)]
    fn write_tab_row(
        &self,
        ctx: &RunContext,
        summary: &str,
        tab: Option<&TabInfo>,
        url: &str,
        ev: &Evaluation,
        decision: AuditDecision,
        reason: &str,
        approval_id: Option<String>,
    ) {
        self.write_row(
            ctx.session_id,
            DecisionAudit {
                ts_ms: crate::time::now_ms(),
                session_id: ctx.session_id,
                tool: "browser_tabs".into(),
                url: audit_url(url),
                summary: summary.to_owned(),
                tab_id: tab.and_then(|t| i64::try_from(t.tab).ok()),
                tab_owner: tab.and_then(|t| t.owner.clone()),
                tier: ev.tier.letter().into(),
                categories: ev
                    .categories
                    .iter()
                    .map(|c| c.as_str().to_owned())
                    .collect(),
                decision,
                reason: reason.to_owned(),
                approval_id,
                screenshot: None,
                actor: "mono".into(),
                input: None,
                profile: None,
            },
        );
    }

    fn write_simple_row(&self, ctx: &RunContext, summary: &str, tab: Option<&TabInfo>) {
        let ev = Evaluation {
            decision: Decision::Allow,
            tier: Tier::Act,
            categories: BTreeSet::new(),
            site: String::new(),
            reason: String::new(),
        };
        self.write_tab_row(
            ctx,
            summary,
            tab,
            tab.map_or("", |t| t.url.as_str()),
            &ev,
            AuditDecision::Auto,
            "own tab",
            None,
        );
    }

    /// The user took a tab out of this Mono's group: its grants for that site end in every
    /// conversation, its open cards go and the tab is no longer its selection.
    pub(in super::super) fn tab_taken_back(&self, tab: u64) {
        let site = self
            .state()
            .known
            .get(&tab)
            .and_then(|t| registrable_domain_of(&t.url));
        if let (Some(site), Some(store)) = (site, self.act_config().store) {
            let _ = store.revoke_grants(&self.mono_id, None, Some(&site));
        }
        self.approvals().withdraw_all();
        self.forget_tab(tab);
    }
}

fn find<'a>(quote: &str, tabs: &'a [TabInfo]) -> Result<&'a TabInfo, ownership::MatchError> {
    ownership::find_tab(quote, tabs)
}
