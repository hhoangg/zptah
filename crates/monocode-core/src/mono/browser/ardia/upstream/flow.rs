//! The flow tools (design spec sections 5 and 7): `flow_list` and `flow_show` read, `flow_run`
//! and `flow_save` act.
//!
//! `flow_run` in order:
//!
//! 1. `flow show` and the check of [`super::flow_risk`] (every step, every address, the
//!    parameters), keyed by a content hash of `data.flow` and the parameters;
//! 2. a refusal ends it; a risk asks once for the whole run (a conversation grant for "this flow"
//!    is keyed by the hash, so it covers that content with those parameters and nothing else);
//! 3. the session's own tab is opened if there is none and moved to the flow's start address
//!    through the normal navigation path (origin check, the act tools' navigation rules);
//! 4. `flow show` again: only the content that was checked and approved runs, a changed flow is
//!    checked and asked again (three rounds, then the call fails);
//! 5. `flow run [--yes] [--param k=v ...] -- <name>`, `--yes` only for a flow that writes and only
//!    after steps 1 to 4;
//! 6. the tab's URL after, the origin check of it, the windows the flow opened, one audit row.
//!
//! The run is an act call: when its outcome is unknown (Ardia went away after it was sent, a
//! timeout, a Stop) it is never repeated, and the Mono is told it may or may not have run.
//!
//! Not measured against a real Ardia (see `flow_model`): `flow run` with `--param`, `--limit`,
//! `--fields`, `--where`, `--yes` in this position, the output of a data flow, `flow steps` as
//! the name of the command that lists the session's recorded steps (spec section 12b calls it
//! that). The code fails closed on what it does not recognise.
use super::flow_model::{self as model, Flow, Kind, RunOptions};
use super::flow_risk::Classified;
use super::fresh::SeenRefs;
use super::*;
use crate::mono::browser::copy::{ActionDesc, CardFacts};
use crate::mono::browser::risk::decide::{never_granted, Decision};
use crate::mono::browser::risk::signals::{registrable_domain_of, site_category};
use crate::mono::browser::risk::Category;
use crate::mono::browser::trust::audit::AuditDecision;
use crate::mono::browser::trust::{Answer, BrowserLevel, Evaluation, Tier};
use std::collections::BTreeMap;

/// The longest a flow may run.
const FLOW_TIMEOUT: Duration = Duration::from_secs(300);
/// How often a changed flow is checked and asked again before the call fails.
const MAX_ROUNDS: usize = 3;
const MAX_LISTED: usize = 100;
const MAX_DESCRIPTION: usize = 200;

/// What a `flow_run` call asked for.
struct RunRequest {
    name: String,
    params: BTreeMap<String, String>,
    opts: RunOptions,
    purpose: Option<String>,
    declared: Vec<Category>,
}

/// How the run was allowed.
struct Approval {
    decision: AuditDecision,
    card: Option<String>,
}

/// The key a conversation grant for one flow content is stored under (the grant book keys by
/// site; this is no real site, so it never matches one).
pub(super) fn grant_key(name: &str, hash: &str) -> String {
    format!("flow:{name}:{}", &hash[..32.min(hash.len())])
}

fn unreadable(what: &str, why: &str) -> String {
    format!("Ardia's {what} is not in a shape zptah can read ({why}), so it was not used.")
}

fn kind_words(kind: &Kind, writes: Option<bool>) -> &'static str {
    match (kind, writes) {
        (Kind::Data, Some(false)) => "reads data only",
        (Kind::Actions, Some(true)) => "page actions, changes pages",
        (Kind::Actions, _) => "page actions",
        (Kind::Data, _) => "data flow (writes unknown)",
        (Kind::Other(_), _) => "unknown kind",
    }
}

fn short(text: &str, max: usize) -> String {
    clean(&one_line(text)).chars().take(max).collect()
}

impl ArdiaUpstream {
    /// `ardia browse` with a longer timeout, killed by the turn's Stop.
    fn flow_browse(
        &self,
        id: i64,
        cmd: &[&str],
        values: &[&str],
        timeout: Duration,
    ) -> Result<Value, Failure> {
        let cancel = self.conv(id, |c| c.cancel.clone());
        self.controller.cli().browse_cancellable(
            &self.session(id),
            cmd,
            values,
            timeout,
            Some(&cancel),
        )
    }

    fn show_flow(&self, id: i64, name: &str) -> Result<Flow, String> {
        let data = self
            .flow_browse(id, &["flow", "show"], &[name], BROWSE_TIMEOUT)
            .map_err(|f| self.fail(id, f))?;
        model::parse_flow(&data, name).map_err(|why| unreadable("description of the flow", &why))
    }

    // ---- flow_list and flow_show (read) ----

    pub(super) fn flow_list(&self, id: i64) -> Result<String, String> {
        let data = self
            .flow_browse(id, &["flow", "list"], &[], BROWSE_TIMEOUT)
            .map_err(|f| self.fail(id, f))?;
        let flows = data["flows"]
            .as_array()
            .ok_or_else(|| unreadable("list of flows", "no `flows` list"))?;
        let mut out = String::from("### Flows\n");
        if flows.is_empty() {
            out.push_str("No flows are saved.\n");
        }
        for flow in flows.iter().take(MAX_LISTED) {
            let name = flow["name"].as_str().unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let kind = match &flow["kind"] {
                Value::Null => Kind::Data,
                Value::String(s) if s == "actions" => Kind::Actions,
                Value::String(s) if s == "data" => Kind::Data,
                other => Kind::Other(other.as_str().unwrap_or("?").to_owned()),
            };
            let params = model::param_names(&flow["params"]).unwrap_or_default();
            out.push_str(&format!(
                "- {} (site {}) [{}]{}{}\n",
                short(name, 64),
                short(flow["site"].as_str().unwrap_or("?"), 80),
                kind_words(&kind, flow["writes"].as_bool()),
                match flow["description"]
                    .as_str()
                    .filter(|d| !d.trim().is_empty())
                {
                    Some(d) => format!(": {}", short(d, 160)),
                    None => String::new(),
                },
                if params.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; parameters: {}",
                        params
                            .iter()
                            .map(|p| short(p, 40))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                },
            ));
        }
        if flows.len() > MAX_LISTED {
            out.push_str(&format!("... and {} more.\n", flows.len() - MAX_LISTED));
        }
        self.audit(id, "flow_list", "", "list flows");
        Ok(out)
    }

    pub(super) fn flow_show(&self, id: i64, args: &Value) -> Result<String, String> {
        let name = model::flow_name(args["name"].as_str().unwrap_or("").trim())?;
        let flow = self.show_flow(id, &name)?;
        let mut out = String::from("### Flow\n");
        out.push_str(&format!("- Name: {}\n", short(&flow.name, 64)));
        out.push_str(&format!("- Site: {}\n", short(&flow.site, 80)));
        out.push_str(&format!(
            "- Start page: {}\n",
            defense::redact_url_secrets(&short(&flow.start_url, 200))
        ));
        out.push_str(&format!(
            "- Kind: {}\n",
            kind_words(&flow.kind, flow.writes)
        ));
        out.push_str(&format!(
            "- Verified: {}\n",
            match flow.verified {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            }
        ));
        if !flow.description.trim().is_empty() {
            out.push_str(&format!(
                "- Description: {}\n",
                short(&flow.description, 200)
            ));
        }
        out.push_str(&format!(
            "- Parameters: {}\n",
            if flow.params.is_empty() {
                "none".to_owned()
            } else {
                flow.params
                    .iter()
                    .map(|p| short(p, 40))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
        out.push_str("### Steps\n");
        for (i, step) in flow.steps.iter().enumerate() {
            let n = step
                .seq
                .map_or_else(|| (i + 1).to_string(), |n| n.to_string());
            let words = defense::redact_url_secrets(&short(&step.command.join(" "), 200));
            if step.intent.trim().is_empty() {
                out.push_str(&format!("- {n}: {words}\n"));
            } else {
                out.push_str(&format!("- {n}: {} ({words})\n", short(&step.intent, 160)));
            }
        }
        if flow.steps.is_empty() {
            out.push_str("No steps.\n");
        }
        self.audit(id, "flow_show", "", &format!("show flow {}", flow.name));
        Ok(clean(&out))
    }

    // ---- audit ----

    #[allow(clippy::too_many_arguments)]
    fn flow_row(
        &self,
        id: i64,
        tool: &str,
        cls: Option<&Classified>,
        url: &str,
        summary: &str,
        decision: AuditDecision,
        reason: &str,
        approval: Option<String>,
    ) {
        let ev = cls.map(|c| Evaluation {
            decision: Decision::Allow,
            tier: Tier::Act,
            categories: c.categories.clone(),
            site: c.site.clone(),
            reason: reason.to_owned(),
        });
        let user_tab = self.conv(id, |c| c.bound.is_some() && c.user_tab == c.bound);
        self.row(
            id,
            tool,
            url,
            summary,
            user_tab,
            ev.as_ref(),
            decision,
            reason,
            approval,
        );
    }

    fn run_summary(req: &RunRequest, cls: &Classified) -> String {
        format!("flow {} #{}", req.name, &cls.hash[..8.min(cls.hash.len())])
    }

    // ---- flow_run ----

    fn run_request(args: &Value) -> Result<RunRequest, String> {
        let name = model::flow_name(args["name"].as_str().unwrap_or("").trim())?;
        let declared = args["declared_risk"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().and_then(Category::parse))
            .collect();
        Ok(RunRequest {
            name,
            params: model::parse_params(args)?,
            opts: RunOptions::parse(args)?,
            purpose: args["purpose"]
                .as_str()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned),
            declared,
        })
    }

    /// `flow show` and the check of it. An unreadable reply fails the call (nothing runs).
    fn check_flow(&self, id: i64, req: &RunRequest) -> Result<Classified, String> {
        let flow = self.show_flow(id, &req.name).inspect_err(|text| {
            self.flow_row(
                id,
                "flow_run",
                None,
                "",
                &format!("flow {}: its description could not be read", req.name),
                AuditDecision::Blocked,
                text,
                None,
            );
        })?;
        let mut cls = self.classify_flow(id, flow, &req.params);
        // What the Mono declared can only add questions.
        if !req.declared.is_empty() {
            cls.categories.extend(req.declared.iter().copied());
            cls.risks.push(super::flow_risk::Risk {
                text: "risk the Mono declared".into(),
                categories: req.declared.iter().copied().collect(),
            });
        }
        Ok(cls)
    }

    pub(super) fn flow_run(&self, id: i64, args: &Value) -> Result<String, String> {
        let act = self.act_allowed(id, "flow_run")?;
        let req = Self::run_request(args)?;
        self.not_stopped(id)?;
        self.ready(id)?;
        let mut cls = self.check_flow(id, &req)?;
        if !cls.flow.is_data() && !req.opts.is_empty() {
            return Err("`limit`, `fields` and `where` shape the data of a flow that only reads. This flow changes pages.".into());
        }
        let mut navigated: Option<String> = None;
        for _ in 0..MAX_ROUNDS {
            if let Some(refusal) = cls.refusal.take() {
                self.flow_row(
                    id,
                    "flow_run",
                    Some(&cls),
                    &cls.flow.start_url,
                    &Self::run_summary(&req, &cls),
                    refusal.decision,
                    &refusal.reason,
                    None,
                );
                if refusal.secret {
                    self.post_password_notice(&cls.flow.start_url);
                }
                return Err(refusal.text);
            }
            let approval = self.approve_flow(id, &act, &req, &cls)?;
            // The session's own tab, on the flow's start page (the normal, origin-checked path).
            if navigated.as_deref() != Some(cls.flow.start_url.as_str()) {
                self.navigate(id, &cls.flow.start_url)?;
                navigated = Some(cls.flow.start_url.clone());
            }
            self.not_stopped(id)?;
            // Right before the run: only the content that was checked (and approved) runs.
            let again = self.check_flow(id, &req)?;
            if again.hash == cls.hash {
                return self.execute_flow(id, &req, &again, approval);
            }
            self.flow_row(
                id,
                "flow_run",
                Some(&again),
                &again.flow.start_url,
                &Self::run_summary(&req, &again),
                AuditDecision::Blocked,
                "the flow changed after it was checked; it is checked again",
                approval.card,
            );
            cls = again;
        }
        self.flow_row(
            id,
            "flow_run",
            Some(&cls),
            &cls.flow.start_url,
            &Self::run_summary(&req, &cls),
            AuditDecision::Blocked,
            "the flow kept changing; it was not run",
            None,
        );
        Err("Not run: the flow changed again and again while zptah was checking it. Read it with flow_show and try again.".into())
    }

    /// Allow, allow by a grant on this content, or ask once for the whole run.
    fn approve_flow(
        &self,
        id: i64,
        act: &ActConfig,
        req: &RunRequest,
        cls: &Classified,
    ) -> Result<Approval, String> {
        if cls.risks.is_empty() {
            return Ok(Approval {
                decision: AuditDecision::Auto,
                card: None,
            });
        }
        let key = grant_key(&req.name, &cls.hash);
        let covered = self
            .with_trust(id, |state, _| state.grants().covered(&key))
            .unwrap_or_default();
        if cls
            .categories
            .iter()
            .all(|c| covered.contains(c) && !never_granted(*c))
        {
            self.note_flow_sites(id, cls);
            return Ok(Approval {
                decision: AuditDecision::Auto,
                card: None,
            });
        }
        let categories: Vec<Category> = cls.categories.iter().copied().collect();
        let ev = Evaluation {
            decision: Decision::Ask(categories.clone()),
            tier: Tier::Act,
            categories: cls.categories.clone(),
            site: cls.site.clone(),
            reason: "the flow has steps that need the user's OK".into(),
        };
        let mut facts = CardFacts::new(
            &act.mono_label,
            &cls.site,
            ActionDesc::FlowRun {
                name: req.name.clone(),
                risky: cls.risks.iter().map(|r| r.text.clone()).collect(),
                params: req
                    .params
                    .iter()
                    .map(|(k, v)| (k.clone(), v.chars().count()))
                    .collect(),
            },
            categories,
        );
        facts.url = cls.flow.start_url.clone();
        facts.site_kind = site_category(&cls.flow.start_url);
        facts.purpose = req.purpose.clone();
        let summary = Self::run_summary(req, cls);
        match self.ask_card(act, id, "flow_run", facts, &ev) {
            Err((text, audit, card)) => {
                self.flow_row(
                    id,
                    "flow_run",
                    Some(cls),
                    &cls.flow.start_url,
                    &summary,
                    audit,
                    &text,
                    Some(card).filter(|c| !c.is_empty()),
                );
                Err(text)
            }
            Ok((answer, card)) => {
                // The card may have waited minutes: a Stop or a lowered level ends the run.
                let blocked = |why: &str| {
                    self.flow_row(
                        id,
                        "flow_run",
                        Some(cls),
                        &cls.flow.start_url,
                        &summary,
                        AuditDecision::Blocked,
                        why,
                        Some(card.clone()),
                    );
                };
                if let Err(stopped) = self.not_stopped(id) {
                    blocked("stopped while the user was asked");
                    return Err(stopped);
                }
                if self
                    .config()
                    .act
                    .is_none_or(|a| self.level_now(&a) != BrowserLevel::Act)
                {
                    blocked("the browser level was changed to read only while the user was asked");
                    return Err("flow_run is not available any more: the browser level was changed to read only while the user was asked, so nothing was done.".into());
                }
                self.note_flow_sites(id, cls);
                let granted = if answer == Answer::AllowedTask {
                    let grantable = cls
                        .categories
                        .iter()
                        .copied()
                        .filter(|c| !never_granted(*c))
                        .collect();
                    self.with_trust(id, |state, _| state.grants_mut().grant(&key, &grantable))
                        .unwrap_or_default()
                } else {
                    Default::default()
                };
                Ok(Approval {
                    decision: if !granted.is_empty() {
                        AuditDecision::AskedAllowedTask
                    } else {
                        AuditDecision::AskedAllowedOnce
                    },
                    card: Some(card),
                })
            }
        }
    }

    /// The flow's own start site, once the user approved the run (or a grant on this content
    /// covers it), is a task site, so moving to the start page does not ask about it a second
    /// time. Only that site: every other address of the flow asks again for another content, and
    /// the answer is recorded as "once", so a site-wide grant is never made from a flow.
    fn note_flow_sites(&self, id: i64, cls: &Classified) {
        self.with_trust(id, |state, _| {
            for ev in cls.evals.iter().filter(|ev| ev.site == cls.site) {
                state.commit(ev, Answer::AllowedOnce);
            }
        });
    }

    fn execute_flow(
        &self,
        id: i64,
        req: &RunRequest,
        cls: &Classified,
        approval: Approval,
    ) -> Result<String, String> {
        let tab = self.bound_tab(id)?;
        if self.conv(id, |c| c.halted.contains(&tab)) {
            return Err("This tab was stopped for the rest of the turn because it reached a page this Mono may not use. Do not go back to it.".into());
        }
        self.hold(id)?;
        let before = self.conv(id, |c| c.page.clone());
        // `--yes` is the gate's: only for a flow that says it writes, only after the check, the
        // approval and the second read above. A data flow never gets it.
        let yes = cls.flow.writes == Some(true);
        let (cmd, values) = model::run_argv(&req.name, yes, &req.params, &req.opts);
        let cmd: Vec<&str> = cmd.iter().map(String::as_str).collect();
        let values: Vec<&str> = values.iter().map(String::as_str).collect();
        let summary = Self::run_summary(req, cls);
        let watch = self.watch_before(id, &tab);
        let sent = self.flow_browse(id, &cmd, &values, FLOW_TIMEOUT);
        let row = |url: &str, text: &str, reason: &str| {
            self.flow_row(
                id,
                "flow_run",
                Some(cls),
                url,
                text,
                approval.decision,
                reason,
                approval.card.clone(),
            );
        };
        let data = match sent {
            Ok(data) => data,
            Err(Failure::Refused { code, message }) => {
                // Refused: usually nothing ran, but a flow can stop midway, so the page is
                // looked at all the same.
                let effects = self.watch_after(id, &tab, watch);
                let note = format!(
                    "{}{}",
                    clean(&message),
                    if effects.lines().is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", effects.lines())
                    }
                );
                row(
                    &before,
                    &format!("{summary}; refused"),
                    &format!(
                        "the command was refused: {}",
                        code.as_deref().unwrap_or("failed")
                    ),
                );
                let after = self.after_flow(id, &tab, &before)?;
                self.after_state(id, &tab, &before, &after);
                return Err(note);
            }
            Err(other) => {
                // Unreachable after the command was sent, a timeout, an unreadable reply or a
                // Stop: nobody knows whether it ran. Never repeated (see `call_tool`).
                let stopped = matches!(other, Failure::Cancelled);
                let cleanup = self.watch_after(id, &tab, watch).lines();
                let unreachable = matches!(other, Failure::Unreachable(_));
                let _ = self.fail(id, other);
                if unreachable {
                    // `fail` marked the call for a repeat; a flow run is never repeated.
                    self.conv(id, |c| c.lost = false);
                }
                row(
                    &before,
                    &format!("{summary}; outcome unknown"),
                    "the outcome is unknown",
                );
                return Err(if stopped {
                    format!("{STOPPED} The flow may or may not have run: look at the page before trying anything again.\n{cleanup}")
                } else {
                    format!("The result of this flow is unknown: Ardia stopped answering after the command was sent, so it may or may not have run, and some steps may have been done. It was NOT repeated. Look at the page (browser_snapshot) before trying again.\n{cleanup}")
                });
            }
        };
        let effects = self.watch_after(id, &tab, watch);
        let after = match self.after_flow(id, &tab, &before) {
            Ok(after) => after,
            Err(stop) => {
                row(
                    &before,
                    &format!("{summary}; ran, the tab ended on a page this Mono may not use"),
                    "the page after the flow was blocked or could not be read",
                );
                return Err(stop);
            }
        };
        self.after_state(id, &tab, &before, &after);
        let steps: Vec<&Value> = data["steps"].as_array().into_iter().flatten().collect();
        let failed = data["ok"] == false || steps.iter().any(|s| s["ok"] == false);
        let mut summary = format!(
            "{summary}; {} step(s){}; url after {}",
            steps.len(),
            if failed { ", a step failed" } else { "" },
            audit_url(&after)
        );
        if !effects.closed.is_empty() {
            summary = format!(
                "{summary}; closed {} new window(s) the page opened",
                effects.closed.len()
            );
        }
        if effects.needs_attention() {
            summary = format!("{summary}; a window the page opened may have been left open");
        }
        self.flow_row(
            id,
            "flow_run",
            Some(cls),
            &before,
            &summary,
            approval.decision,
            if cls.risks.is_empty() {
                "no risk found"
            } else {
                "the user approved the flow"
            },
            approval.card.clone(),
        );
        let mut body = String::from("### Result\n");
        body.push_str(&format!(
            "- {} flow \"{}\": {} step(s){}.\n",
            if failed { "Stopped" } else { "Ran" },
            req.name,
            steps.len(),
            if failed {
                ", at least one failed"
            } else {
                ", all ok"
            }
        ));
        body.push_str(
            "- The page may have changed: take a browser_snapshot before acting on it.\n",
        );
        body.push_str(&effects.lines());
        for step in steps.iter().take(100) {
            body.push_str(&format!(
                "- step {}: {}{}\n",
                step["seq"]
                    .as_i64()
                    .map_or("?".to_owned(), |n| n.to_string()),
                short(step["intent"].as_str().unwrap_or(""), 120),
                if step["ok"] == true { "" } else { " (failed)" }
            ));
        }
        // What a data flow returned: not measured, so only these keys are shown, as JSON.
        let mut output = serde_json::Map::new();
        for key in ["list", "total", "matched", "items"] {
            if !data[key].is_null() {
                output.insert(key.to_owned(), data[key].clone());
            }
        }
        if !output.is_empty() {
            body.push_str(&format!(
                "### Data\n{}\n",
                cap(&Value::Object(output).to_string())
            ));
        }
        let body = clean(&body);
        // What the Mono reads is remembered (in memory) to spot it being sent to another site.
        self.note_read(id, &after, &body);
        let reply = format!("{}{}", header(&after, ""), body);
        if failed {
            Err(reply)
        } else {
            Ok(reply)
        }
    }

    /// The tab's address after a flow: read, and checked against the origin lists. A page that
    /// can not be read counts as a hit.
    fn after_flow(&self, id: i64, tab: &str, before: &str) -> Result<String, String> {
        let after = match self.url_now(id) {
            Ok(url) if !url.is_empty() => url,
            _ => return Err(self.halt_unknown(id, tab, before, "flow_run")),
        };
        self.after(id, tab, &after, "flow_run")?;
        Ok(after)
    }

    /// What the page being somewhere else means for the next call.
    fn after_state(&self, id: i64, tab: &str, before: &str, after: &str) {
        if registrable_domain_of(after) != registrable_domain_of(before) {
            self.with_trust(id, |state, _| state.note_link_followed(after));
        }
        // The refs the Mono holds belong to the page before the flow.
        self.conv(id, |c| {
            c.seen.insert(
                tab.to_owned(),
                SeenRefs {
                    url: after.to_owned(),
                    epoch: None,
                    refs: HashMap::new(),
                },
            );
        });
    }

    // ---- flow_save ----

    pub(super) fn flow_save(&self, id: i64, args: &Value) -> Result<String, String> {
        let act = self.act_allowed(id, "flow_save")?;
        let name = model::flow_name(args["name"].as_str().unwrap_or("").trim())?;
        let actions = args["actions"].as_bool().unwrap_or(false);
        let description = match args["description"]
            .as_str()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            Some(d) => {
                if d.chars().count() > MAX_DESCRIPTION {
                    return Err(format!(
                        "`description` is one line of at most {MAX_DESCRIPTION} characters."
                    ));
                }
                Some(plain_value("description", d)?.to_owned())
            }
            None => None,
        };
        self.not_stopped(id)?;
        self.ready(id)?;
        let page = self.conv(id, |c| c.page.clone());
        let fail_row = |why: &str| {
            self.flow_row(
                id,
                "flow_save",
                None,
                &page,
                &format!("save flow {name}"),
                AuditDecision::Blocked,
                why,
                None,
            );
        };
        // The steps this conversation recorded (what `steps` shows).
        let recorded = self
            .flow_browse(id, &["flow", "steps"], &[], BROWSE_TIMEOUT)
            .map_err(|f| self.fail(id, f))?;
        let count = recorded["steps"]
            .as_array()
            .ok_or_else(|| unreadable("list of recorded steps", "no `steps` list"))?
            .len();
        if count == 0 {
            fail_row("there are no recorded steps");
            return Err("There are no recorded steps in this conversation to save. Do the task in the browser first.".into());
        }
        let listed = self
            .flow_browse(id, &["flow", "list"], &[], BROWSE_TIMEOUT)
            .map_err(|f| self.fail(id, f))?;
        let replaces = listed["flows"]
            .as_array()
            .ok_or_else(|| unreadable("list of flows", "no `flows` list"))?
            .iter()
            .any(|f| f["name"].as_str() == Some(name.as_str()));
        let ev = Evaluation {
            decision: Decision::Ask(Vec::new()),
            tier: Tier::Always,
            categories: Default::default(),
            site: registrable_domain_of(&page).unwrap_or_default(),
            reason: "a save writes into the flow store the user shares".into(),
        };
        let mut facts = CardFacts::new(
            &act.mono_label,
            &ev.site,
            ActionDesc::FlowSave {
                name: name.clone(),
                steps: count,
                replaces,
            },
            Vec::new(),
        );
        facts.url = page.clone();
        facts.agent_details = description.clone();
        facts.purpose = args["purpose"]
            .as_str()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned);
        let summary = format!("save flow {name} ({count} step(s))");
        let card = match self.ask_card(&act, id, "flow_save", facts, &ev) {
            Ok((_, card)) => card,
            Err((text, audit, card)) => {
                self.flow_row(
                    id,
                    "flow_save",
                    None,
                    &page,
                    &summary,
                    audit,
                    &text,
                    Some(card).filter(|c| !c.is_empty()),
                );
                return Err(text);
            }
        };
        if let Err(stopped) = self.not_stopped(id) {
            fail_row("stopped while the user was asked");
            return Err(stopped);
        }
        if self
            .config()
            .act
            .is_none_or(|a| self.level_now(&a) != BrowserLevel::Act)
        {
            fail_row("the browser level was changed to read only while the user was asked");
            return Err("flow_save is not available any more: the browser level was changed to read only while the user was asked, so nothing was saved.".into());
        }
        let mut cmd: Vec<&str> = vec!["flow", "save"];
        if actions {
            cmd.push("--actions");
        }
        if let Some(d) = &description {
            cmd.extend(["--description", d.as_str()]);
        }
        let row = |text: &str, decision: AuditDecision, reason: &str| {
            self.flow_row(
                id,
                "flow_save",
                None,
                &page,
                text,
                decision,
                reason,
                Some(card.clone()),
            );
        };
        match self.flow_browse(id, &cmd, &[name.as_str()], BROWSE_TIMEOUT) {
            Ok(data) => {
                row(
                    &summary,
                    AuditDecision::AskedAllowedOnce,
                    "the user approved the save",
                );
                let saved = &data["flow"];
                Ok(format!(
                    "### Result\nSaved the flow \"{}\" for {} ({}, {} parameter(s)). It is in Ardia's flow store, shared with the user.\n",
                    short(saved["name"].as_str().unwrap_or(&name), 64),
                    short(saved["site"].as_str().unwrap_or("?"), 80),
                    kind_words(
                        &match saved["kind"].as_str() {
                            Some("actions") => Kind::Actions,
                            _ => Kind::Data,
                        },
                        saved["writes"].as_bool()
                    ),
                    data["params"].as_array().map_or(0, Vec::len),
                ))
            }
            Err(Failure::Refused { code, message }) => {
                row(
                    &format!("{summary}; refused"),
                    AuditDecision::AskedAllowedOnce,
                    &format!(
                        "Ardia refused the save: {}",
                        code.as_deref().unwrap_or("failed")
                    ),
                );
                Err(format!("Ardia did not save the flow: {}", clean(&message)))
            }
            Err(other) => {
                let stopped = matches!(other, Failure::Cancelled);
                let unreachable = matches!(other, Failure::Unreachable(_));
                let _ = self.fail(id, other);
                if unreachable {
                    self.conv(id, |c| c.lost = false);
                }
                row(
                    &format!("{summary}; outcome unknown"),
                    AuditDecision::AskedAllowedOnce,
                    "the outcome is unknown",
                );
                Err(if stopped {
                    format!("{STOPPED} The flow may or may not have been saved.")
                } else {
                    "The result of this save is unknown: Ardia stopped answering after the command was sent, so the flow may or may not be saved. It was NOT repeated. Use flow_list to see.".into()
                })
            }
        }
    }
}
