//! The check of a flow before it runs (design spec section 7, steps 1 to 3): every step is
//! classified with the same rules as the act tools, every address the flow names is checked
//! against the Mono's origin lists, and the parameters are classified with the steps they feed.
//!
//! A flow replays inside Ardia, so the gate never sees the page around a step. The inputs the
//! classifier gets are what the flow says: the step's kind (from its `command`, checked against
//! `action.type`), the target's name (fingerprint, candidates, `--name`), the page the step acts
//! on (the address of the step before), and the value typed (a parameter or a literal). Where
//! that is not enough the step asks with [`Category::Unclear`], which is never granted.
//!
//! What no approval overrides (the run is refused): an address that is blocked or is not a web
//! page, a step that types into a field named like a password or a one-time code, a flow with a
//! `secret` parameter, a value Ardia redacted when it recorded the flow (a typed password), a
//! `fill-otp` step, an `upload` step or any step that names a file path, a parameter named like a
//! password or code, a parameter the flow does not declare, a flow that says it only reads but has
//! a step that writes, and a soul rule that blocks.
use super::flow_model::{self as model, Flow, Kind, ParamSource, Step, StepKind};
use super::fresh::{self, Attrs};
use super::*;
use crate::mono::browser::gate::extension::act::looks_vietnamese;
use crate::mono::browser::gate::extension::ReadRefusal;
use crate::mono::browser::risk::decide::Decision;
use crate::mono::browser::risk::lexicon::{lexicon, ControlKind};
use crate::mono::browser::risk::signals::{
    extract_amounts, registrable_domain_of, ActionKind, ElementSignals,
};
use crate::mono::browser::risk::Category;
use crate::mono::browser::trust::audit::AuditDecision;
use crate::mono::browser::trust::{Action as TrustAction, Config as TrustConfig, Evaluation};
use std::collections::{BTreeMap, BTreeSet};

/// The most distinct addresses one flow may name.
const MAX_URLS: usize = 60;

/// One thing in the flow that needs the user's OK.
pub(super) struct Risk {
    /// What the card says: `step 3: click "Delete"`.
    pub text: String,
    pub categories: BTreeSet<Category>,
}

/// Why a run is refused with no card.
pub(super) struct Refusal {
    pub decision: AuditDecision,
    pub reason: String,
    /// What the Mono is told.
    pub text: String,
    /// A password or a code: the user gets the "needs your password" notice.
    pub secret: bool,
}

/// The result of checking one `flow show` reply with the parameters of the call.
pub(super) struct Classified {
    pub flow: Flow,
    /// The hash an approval and a grant are keyed by.
    pub hash: String,
    /// The registrable domain of the start address.
    pub site: String,
    pub risks: Vec<Risk>,
    /// Every category of every risk.
    pub categories: BTreeSet<Category>,
    /// The classifier's evaluations that found something, to record the answer on.
    pub evals: Vec<Evaluation>,
    pub refusal: Option<Refusal>,
}
impl Classified {
    fn refuse(&mut self, decision: AuditDecision, reason: String, text: String) {
        if self.refusal.is_none() {
            self.refusal = Some(Refusal {
                decision,
                reason,
                text,
                secret: false,
            });
        }
    }
    fn refuse_secret(&mut self, reason: String, text: String) {
        self.refuse(AuditDecision::Invariant, reason, text);
        if let Some(r) = &mut self.refusal {
            r.secret = true;
        }
    }
    fn risk(&mut self, text: String, categories: impl IntoIterator<Item = Category>) {
        self.risks.push(Risk {
            text,
            categories: categories.into_iter().collect(),
        });
    }
}

/// A key that moves the page without typing or pressing anything: it needs no look.
fn quiet_key(key: &str) -> bool {
    matches!(
        key.to_lowercase().as_str(),
        "tab"
            | "escape"
            | "esc"
            | "arrowup"
            | "arrowdown"
            | "arrowleft"
            | "arrowright"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "shift"
            | "control"
            | "alt"
            | "meta"
    )
}

/// A step as the card names it: its intent, else its command, on one line.
fn describe(step: &Step) -> String {
    let base = if step.intent.trim().is_empty() {
        model::masked_command(&step.command)
            .iter()
            .filter(|t| !t.to_lowercase().contains("<redacted>"))
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        model::masked_text(&step.intent, step)
    };
    let text: String = clean(&one_line(&base)).chars().take(100).collect();
    // The target as the command names it, next to what the recording says it is: a recorded
    // intent is only a note, the command is what Ardia replays.
    match model::target_of(&step.command) {
        Some(target) => format!(
            "{text} [{}]",
            clean(&one_line(&target))
                .chars()
                .take(60)
                .collect::<String>()
        ),
        None => text,
    }
}

fn seq_of(step: &Step, index: usize) -> String {
    step.seq
        .map_or_else(|| (index + 1).to_string(), |n| n.to_string())
}

impl ArdiaUpstream {
    fn evaluate_action(&self, id: i64, action: &TrustAction<'_>) -> Option<Evaluation> {
        self.with_trust(id, |state, cfg| {
            state.evaluate(
                &TrustConfig {
                    level: self.level_now(cfg),
                    rules: &cfg.rules,
                },
                action,
            )
        })
    }

    /// Reads the classifier's answer into the result: a deny refuses the run, anything it found
    /// is a risk.
    fn absorb(&self, c: &mut Classified, ev: Evaluation, text: String) {
        if let Decision::Deny(reason) = &ev.decision {
            let decision = if reason.contains("password") {
                AuditDecision::Invariant
            } else {
                AuditDecision::Blocked
            };
            c.refuse(
                decision,
                format!("{text}: {reason}"),
                format!("Not allowed: {reason} ({text})."),
            );
            return;
        }
        if !ev.categories.is_empty() {
            c.risk(text, ev.categories.iter().copied());
            c.evals.push(ev);
        }
    }

    /// Checks the flow `flow` with `params`: the hash, then every refusal and risk.
    pub(super) fn classify_flow(
        &self,
        id: i64,
        flow: Flow,
        params: &BTreeMap<String, String>,
    ) -> Classified {
        let mut c = Classified {
            hash: model::flow_hash(&flow.raw, &flow.site, params),
            site: registrable_domain_of(&flow.start_url).unwrap_or_default(),
            risks: Vec::new(),
            categories: BTreeSet::new(),
            evals: Vec::new(),
            refusal: None,
            flow: flow.clone(),
        };
        self.classify_flow_into(id, &flow, params, &mut c);
        c.categories = c
            .risks
            .iter()
            .flat_map(|r| r.categories.iter().copied())
            .collect();
        c
    }

    fn classify_flow_into(
        &self,
        id: i64,
        flow: &Flow,
        params: &BTreeMap<String, String>,
        c: &mut Classified,
    ) {
        // ---- the flow as a whole ----
        if flow.start_url.is_empty() {
            c.refuse(
                AuditDecision::Blocked,
                "the flow has no start address".into(),
                "Not run: the flow has no start address, so zptah can not open the page it starts on.".into(),
            );
            return;
        }
        if let Kind::Other(kind) = &flow.kind {
            c.risk(
                format!(
                    "this is a kind of flow zptah does not know ({})",
                    clean(&one_line(kind)).chars().take(30).collect::<String>()
                ),
                [Category::Unclear],
            );
        }
        if flow.writes.is_none() {
            c.risk(
                "the flow does not say whether it changes pages".into(),
                [Category::Unclear],
            );
        }
        if flow.is_data() {
            for (i, step) in flow.steps.iter().enumerate() {
                let writes = matches!(
                    step.kind,
                    StepKind::Click
                        | StepKind::Check
                        | StepKind::Fill
                        | StepKind::Type
                        | StepKind::Select
                        | StepKind::Press(_)
                        | StepKind::FillOtp
                        | StepKind::Script(_)
                ) || step
                    .methods
                    .iter()
                    .any(|m| !matches!(m.as_str(), "GET" | "HEAD" | "OPTIONS"));
                if writes {
                    let n = seq_of(step, i);
                    c.refuse(
                        AuditDecision::Blocked,
                        format!("the flow says it only reads but step {n} writes"),
                        format!("Not run: the flow says it only reads data, but step {n} ({}) changes the page.", describe(step)),
                    );
                }
            }
        }

        // ---- parameters ----
        for p in &flow.params {
            match &p.source {
                ParamSource::Typed => {}
                // A password field: its value is never recorded and a Mono never supplies one.
                ParamSource::Secret => c.refuse_secret(
                    format!("parameter {} is a password field of the flow", p.name),
                    format!("Not run: this flow has a secret parameter (`{}`, a password field), which a Mono never supplies. Ask the user to run the flow themselves.", clean(&one_line(&p.name)).chars().take(40).collect::<String>()),
                ),
                ParamSource::Other(source) => c.risk(
                    format!(
                        "parameter `{}` comes from a source zptah does not know ({})",
                        clean(&one_line(&p.name)).chars().take(40).collect::<String>(),
                        clean(&one_line(source)).chars().take(20).collect::<String>()
                    ),
                    [Category::Unclear],
                ),
            }
            // A parameter no step names: zptah can not tell where its value would go.
            if !flow.steps.iter().any(|s| model::uses_param(s, &p.name)) {
                c.risk(
                    format!(
                        "parameter `{}` is not named by any step, so zptah can not tell where it goes",
                        clean(&one_line(&p.name)).chars().take(40).collect::<String>()
                    ),
                    [Category::Unclear],
                );
            }
        }
        for key in params.keys() {
            if !flow.declares(key) {
                c.refuse(
                    AuditDecision::Blocked,
                    format!("the flow has no parameter {key}"),
                    format!(
                        "Not run: this flow has no parameter named `{key}`. Its parameters: {}.",
                        if flow.params.is_empty() {
                            "none".to_owned()
                        } else {
                            flow.param_names()
                                .iter()
                                .map(|p| clean(&one_line(p)))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                );
            }
            if fresh::secret_field(key, &Attrs::default()).is_some() {
                c.refuse_secret(
                    format!("parameter {key} is named like a password or a one-time code"),
                    format!("Not run: parameter `{key}` is named like a password or a one-time code, which a Mono never supplies. Ask the user to enter it themselves, then continue."),
                );
            }
        }

        // ---- every address the flow names ----
        let urls = model::urls_of(flow);
        if urls.len() > MAX_URLS {
            c.refuse(
                AuditDecision::Blocked,
                "the flow names too many addresses".into(),
                format!(
                    "Not run: the flow names {} addresses, more than zptah checks ({MAX_URLS}).",
                    urls.len()
                ),
            );
            return;
        }
        let sites = self.classify_urls(id, flow, params, &urls, c);
        self.classify_param_values(id, params, &sites, c);

        // ---- the steps ----
        let unmapped: Vec<&String> = params
            .keys()
            .filter(|k| !flow.steps.iter().any(|s| model::uses_param(s, k)))
            .collect();
        let mut page = flow.start_url.clone();
        for (index, step) in flow.steps.iter().enumerate() {
            self.classify_step(id, flow, step, index, &page, params, &unmapped, c);
            if !step.url.is_empty() {
                page = step.url.clone();
            }
        }
    }

    fn classify_urls(
        &self,
        id: i64,
        flow: &Flow,
        params: &BTreeMap<String, String>,
        urls: &[(String, String)],
        c: &mut Classified,
    ) -> BTreeSet<String> {
        let origins = self.config().origins;
        let mut origins_seen: BTreeSet<String> = BTreeSet::new();
        // Every site the flow can reach: where a parameter value may end up.
        let mut sites: BTreeSet<String> = BTreeSet::new();
        for (url, label) in urls {
            let mut candidates = vec![(url.clone(), false)];
            let substituted = model::substitute(url, params);
            if substituted != *url {
                candidates.push((substituted, true));
            }
            for (candidate, with_params) in candidates {
                if candidate == "about:blank" {
                    continue;
                }
                let shown = defense::redact_url_secrets(&one_line(&candidate))
                    .chars()
                    .take(80)
                    .collect::<String>();
                if web_url(&candidate).is_err() {
                    c.refuse(
                        AuditDecision::Blocked,
                        format!("{label} names an address that is not a web page"),
                        format!("Not run: the flow names an address that is not a web page ({label}: {shown})."),
                    );
                    continue;
                }
                let mut categories: BTreeSet<Category> = BTreeSet::new();
                match origins.check(&candidate) {
                    Err(refusal @ (ReadRefusal::Blocked | ReadRefusal::Restricted)) => {
                        c.refuse(
                            AuditDecision::Blocked,
                            format!(
                                "{label} leads to {}, which this Mono may not use",
                                audit_url(&candidate)
                            ),
                            format!(
                                "Not run: the flow goes to {} ({label}). {}",
                                audit_url(&candidate),
                                tabs_origin_refusal(refusal)
                            ),
                        );
                        continue;
                    }
                    // Leaving the sites this Mono may use is risky, not forbidden.
                    Err(ReadRefusal::NotAllowed) => {
                        categories.insert(Category::UnrelatedDomain);
                    }
                    Ok(()) => {}
                }
                let domain = registrable_domain_of(&candidate).unwrap_or_default();
                if !domain.is_empty() {
                    sites.insert(domain.clone());
                }
                // One look per origin: the first address of it decides. An address a parameter
                // went into is never skipped: the same origin can carry anything in its path.
                let origin = url::Url::parse(&candidate)
                    .map(|u| u.origin().ascii_serialization())
                    .unwrap_or_default();
                if !origins_seen.insert(origin) && !with_params {
                    continue;
                }
                // The navigation parity of the act tools: an address outside the task, a sensitive
                // site, data read elsewhere carried in the address.
                let args = json!({"url": candidate});
                let mut action = TrustAction::new("browser_navigate", &args, &flow.start_url);
                action.target_url = Some(&candidate);
                let text = format!(
                    "{label}: goes to {}",
                    if domain.is_empty() {
                        shown.clone()
                    } else {
                        domain.clone()
                    }
                );
                match self.evaluate_action(id, &action) {
                    Some(ev) if matches!(ev.decision, Decision::Deny(_)) => {
                        self.absorb(c, ev, text)
                    }
                    Some(ev) => {
                        categories.extend(ev.categories.iter().copied());
                        if !categories.is_empty() {
                            c.risk(text, categories);
                            if !ev.categories.is_empty() {
                                c.evals.push(ev);
                            }
                        }
                    }
                    None => {}
                }
            }
        }
        sites
    }

    /// The values the Mono supplies, on every site the flow reaches (whatever field they go
    /// into): a card number, or something read on another site.
    fn classify_param_values(
        &self,
        id: i64,
        params: &BTreeMap<String, String>,
        sites: &BTreeSet<String>,
        c: &mut Classified,
    ) {
        for (key, value) in params {
            for site in sites {
                let found = self
                    .with_trust(id, |state, _| state.value_categories(value, site))
                    .unwrap_or_default();
                if !found.is_empty() {
                    c.risk(
                        format!(
                            "parameter `{}` on {site}",
                            clean(&one_line(key)).chars().take(40).collect::<String>()
                        ),
                        found,
                    );
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn classify_step(
        &self,
        id: i64,
        _flow: &Flow,
        step: &Step,
        index: usize,
        page: &str,
        params: &BTreeMap<String, String>,
        unmapped: &[&String],
        c: &mut Classified,
    ) {
        let n = seq_of(step, index);
        let what = format!("step {n}: {}", describe(step));
        if step.redacted {
            c.refuse_secret(
                format!("step {n} carries a value Ardia redacted when it recorded it"),
                format!("Not run: step {n} carries a secret (Ardia redacted it when the flow was recorded), which a Mono never supplies. Ask the user to run the flow themselves."),
            );
            return;
        }
        if step.kind == StepKind::Upload || step.names_path {
            c.refuse(
                AuditDecision::Blocked,
                format!("step {n} uploads a file or names a file path"),
                format!("Not run: step {n} uploads a file or names a file path, which a flow may not do (browser_upload is the only way a Mono sends a file). Ask the user to run the flow themselves."),
            );
            return;
        }
        if step.mismatch {
            c.risk(
                format!("{what} (the recorded command and action do not agree)"),
                [Category::Unclear],
            );
        }
        match &step.kind {
            StepKind::Open(url) => {
                if web_url(&model::substitute(url, params)).is_err() {
                    c.refuse(
                        AuditDecision::Blocked,
                        format!("step {n} opens an address that is not a web page"),
                        format!("Not run: step {n} opens an address that is not a web page."),
                    );
                }
                if params.keys().any(|k| model::uses_param(step, k)) {
                    c.risk(
                        format!("{what} (a parameter goes into the address)"),
                        [Category::Unclear],
                    );
                }
            }
            // Refused above (an upload is never part of a flow).
            StepKind::Upload => {}
            StepKind::Scroll
            | StepKind::Wait
            | StepKind::History
            | StepKind::Read
            | StepKind::Hover => {}
            StepKind::Press(key) => {
                if !quiet_key(key) {
                    c.risk(what, [Category::Unclear]);
                }
            }
            StepKind::FillOtp => c.refuse_secret(
                format!("step {n} fills a one-time code"),
                format!("Not run: step {n} fills a one-time code, which a Mono never does. Ask the user to run the flow themselves."),
            ),
            StepKind::Script(head) => c.risk(
                format!(
                    "{what} (runs a script: {})",
                    clean(&one_line(head)).chars().take(20).collect::<String>()
                ),
                [Category::Unclear],
            ),
            StepKind::Unknown(head) => c.risk(
                format!(
                    "{what} (a kind of step zptah does not know: {})",
                    clean(&one_line(head)).chars().take(20).collect::<String>()
                ),
                [Category::Unclear],
            ),
            StepKind::Click
            | StepKind::Check
            | StepKind::Select
            | StepKind::Fill
            | StepKind::Type => {
                self.classify_element_step(id, step, &n, &what, page, params, unmapped, c)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn classify_element_step(
        &self,
        id: i64,
        step: &Step,
        n: &str,
        what: &str,
        page: &str,
        params: &BTreeMap<String, String>,
        unmapped: &[&String],
        c: &mut Classified,
    ) {
        let typing = matches!(step.kind, StepKind::Fill | StepKind::Type);
        let names: Vec<String> = step
            .names
            .iter()
            .map(|name| clean(&one_line(name)).chars().take(200).collect::<String>())
            .filter(|name| !name.is_empty())
            .collect();
        if typing {
            let secret = names
                .iter()
                .chain([&step.intent])
                .find_map(|t| fresh::secret_field(t, &Attrs::default()));
            if let Some(why) = secret {
                c.refuse_secret(
                    format!("step {n}: {why}"),
                    format!("Not run: step {n} types into a field that is a password or a one-time code, which a Mono never does ({why}). Ask the user to enter it themselves, then continue."),
                );
                return;
            }
        }
        let vi = looks_vietnamese(&format!("{} {}", names.join(" "), step.intent));
        if names.is_empty() {
            c.risk(
                format!("{what} (no readable name for its target)"),
                [Category::Unclear],
            );
        } else if !typing
            && names
                .iter()
                .any(|name| lexicon().takes_context(name, ControlKind::Button, vi))
        {
            c.risk(
                format!("{what} (a bare word with nothing around it to explain it)"),
                [Category::Unclear],
            );
        }
        // What the step types: the parameters that feed it (those that name it, else every
        // parameter no step names), and a literal of its own.
        let mut typed: Vec<Option<String>> = Vec::new();
        if typing {
            let mapped: Vec<&String> = params
                .keys()
                .filter(|k| model::uses_param(step, k))
                .collect();
            let feeding: Vec<&String> = if mapped.is_empty() {
                unmapped.to_vec()
            } else {
                mapped
            };
            typed.extend(feeding.iter().map(|k| Some(params[*k].clone())));
            typed.extend(step.value.clone().filter(|v| !v.is_empty()).map(Some));
        }
        if typed.is_empty() {
            typed.push(None);
        }
        let probes: Vec<String> = if names.is_empty() {
            vec![String::new()]
        } else {
            names.clone()
        };
        let tool = match step.kind {
            StepKind::Fill | StepKind::Type => "browser_type",
            StepKind::Select => "browser_select_option",
            _ => "browser_click",
        };
        let args = json!({});
        let mut found: BTreeSet<Category> = BTreeSet::new();
        for name in &probes {
            for value in &typed {
                let mut action = TrustAction::new(tool, &args, page);
                action.kind = Some(match step.kind {
                    StepKind::Fill | StepKind::Type => {
                        ActionKind::Type(fresh::field_kind(name, &Attrs::default()))
                    }
                    StepKind::Select => ActionKind::Select,
                    _ => ActionKind::Click,
                });
                action.element = ElementSignals {
                    role: step.role.to_lowercase(),
                    name: name.clone(),
                    label: Some(name.clone()).filter(|l| !l.is_empty()),
                    amounts: extract_amounts(name),
                    input_type: (step.kind == StepKind::Check).then(|| "checkbox".to_owned()),
                    ..Default::default()
                };
                action.page_vi = vi;
                action.typed = value.as_deref();
                let Some(ev) = self.evaluate_action(id, &action) else {
                    continue;
                };
                if matches!(ev.decision, Decision::Deny(_)) {
                    self.absorb(c, ev, what.to_owned());
                    return;
                }
                found.extend(ev.categories.iter().copied());
                if !ev.categories.is_empty() {
                    c.evals.push(ev);
                }
            }
        }
        if !found.is_empty() {
            c.risk(what.to_owned(), found);
        }
    }
}
