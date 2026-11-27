//! The decision of an act tool: what can never be done, then the shared classifier on the fresh
//! read (design spec section 6).
//!
//! * Checks no approval overrides: the page and every destination (a link's `href`, a form's
//!   `action`, a button's `formaction`) against the Mono's origin lists, a password or one-time
//!   code field (by `type`, `autocomplete`, `name`, `id`, `placeholder`, label and name), a
//!   character key whose focus is not known to be harmless, `type` and `fill` into anything that
//!   is not a text field, `check` on anything that is not a checkbox, radio or switch.
//! * An action whose effect the gate cannot read in full (a destination it could not read, a bare
//!   word such as Confirm with no dialog or page text to explain it, a focus it could not
//!   identify) asks with [`Category::Unclear`].
//! * An Enter, or a line break in typed text, submits: every form's `action` in scope is checked
//!   and every submit button in scope is classified as a click (the categories of all of them are
//!   joined). The form of the focused element is its own when the page says which it is.
//! * What the extension backend also did for a normal call stays, and nothing more: the shared
//!   classifier's Tainted category (data read on one site typed or put in an address of
//!   another), Injection (a pattern found in a page of the turn) and the read-only domains. There
//!   is no per-turn intent guard: `browser_declare_intent` never makes a card here.
use super::act::{classifier_name, is_checkable_role, is_field_role, Op, Plan};
use super::focus::Focus;
use super::fresh::{self, Attrs, ElementInput};
use super::page::PageContext;
use super::read::{press_effect, PressEffect, Read};
use super::uploads;
use super::*;
use crate::mono::browser::gate::origins::looks_vietnamese;
use crate::mono::browser::risk::decide::Decision;
use crate::mono::browser::risk::lexicon::{lexicon, ControlKind};
use crate::mono::browser::risk::signals::{extract_amounts, ActionKind, ElementSignals};
use crate::mono::browser::risk::Category;
use crate::mono::browser::trust::audit::AuditDecision;
use crate::mono::browser::trust::{Action as TrustAction, Config as TrustConfig, Evaluation};
use std::collections::BTreeSet;

/// What was decided, and what to compare after the user answered.
pub(super) struct Judged {
    pub ev: Evaluation,
    /// Everything the decision rested on: after a card the same read must give the same one.
    pub sig: Sig,
}

/// The facts a decision rests on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Sig {
    url: String,
    role: String,
    name: String,
    attrs: Attrs,
    per_form: Vec<Option<String>>,
    complete: bool,
    dialog: (bool, String),
    focus: Option<Focus>,
    /// What the page said around the submit buttons an Enter presses, by label.
    submit_ctx: Vec<(String, PageContext)>,
    categories: BTreeSet<Category>,
}

/// Which forms a submit may reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// The form at this index of the page's forms.
    Own(usize),
    /// Any form of the page (the page did not say which one the target belongs to).
    Any,
}

/// The form a submit reaches. A focused element's form index counts only when the page's form
/// count (`document.forms.length`, read by the focus script) is the same number `get count form`
/// gave: when they differ a form came or went between the two reads and the index may point at
/// another form, and when the index is unknown the answer is any form of the page (the union).
fn scope_of(read: &Read) -> Scope {
    match &read.focus {
        Some(Focus::Element(el)) => match (el.form, el.forms_len, read.actions.count) {
            (Some(index), Some(seen), Some(counted)) if seen == counted => Scope::Own(index),
            _ => Scope::Any,
        },
        _ if read.actions.per_form.len() == 1 => Scope::Own(0),
        _ => Scope::Any,
    }
}

/// The labels of the submit buttons an Enter (or a line break in typed text) presses: the focused
/// element's own default button when the page says which form it belongs to, else every submit
/// button of the page's form read.
pub(super) fn submit_labels(read: &Read) -> Vec<String> {
    match (&read.focus, scope_of(read)) {
        (Some(Focus::Element(el)), Scope::Own(_)) => el.default_submit.iter().cloned().collect(),
        _ => read.form.submits.iter().map(|(_, l)| l.clone()).collect(),
    }
}

/// One thing the action does, as the classifier takes it.
struct Cand {
    kind: Option<ActionKind>,
    element: ElementSignals,
}

fn button(label: &str) -> ElementSignals {
    ElementSignals {
        role: "button".into(),
        name: label.to_owned(),
        label: Some(label.to_owned()).filter(|n| !n.is_empty()),
        amounts: extract_amounts(label),
        is_submit: true,
        ..Default::default()
    }
}

impl ArdiaUpstream {
    /// `Err` is the refusal (already audited); `Ok` the evaluation.
    pub(super) fn evaluate(
        &self,
        id: i64,
        plan: &Plan,
        read: &Read,
        files: Option<&[uploads::UploadFile]>,
    ) -> Result<Judged, String> {
        self.evaluate_rows(id, plan, read, files, true)
    }

    /// [`ArdiaUpstream::evaluate`]; with `rows` false a refusal writes no audit row and posts no
    /// notice (the decision made again after a card: the caller writes the one row).
    pub(super) fn evaluate_rows(
        &self,
        id: i64,
        plan: &Plan,
        read: &Read,
        _files: Option<&[uploads::UploadFile]>,
        rows: bool,
    ) -> Result<Judged, String> {
        let page = &read.url;
        let name = clean(&read.name);
        let origins = self.config().origins;
        let deny = |decision: AuditDecision, reason: &str, text: String| {
            if rows {
                self.refuse(id, plan, read, decision, reason, text)
            } else {
                text
            }
        };
        let key = plan.key.as_deref();
        let effect = (plan.op == Op::Press).then(|| press_effect(plan, read));
        let types = matches!(plan.op, Op::Type | Op::Fill) || effect == Some(PressEffect::Types);

        if plan.op == Op::Check && !is_checkable_role(&read.role) {
            return Err(deny(
                AuditDecision::Blocked,
                "check on an element that is not a checkbox, radio or switch",
                format!(
                    "Not done: browser_check works on a checkbox, radio or switch, and that element is a {}. On anything else it would click it. Use browser_click if a click is what you mean.",
                    if read.role.is_empty() { "control" } else { &read.role }
                ),
            ));
        }
        if matches!(plan.op, Op::Type | Op::Fill) && !is_field_role(&read.role) {
            return Err(deny(
                AuditDecision::Blocked,
                "type or fill into an element that is not a text field",
                format!(
                    "Not done: {} types into a text field (a textbox, searchbox, combobox or spinbutton), and that element is a {}. Use browser_click, browser_select or browser_check for it.",
                    plan.tool,
                    if read.role.is_empty() { "control" } else { &read.role }
                ),
            ));
        }
        if effect == Some(PressEffect::Refused) {
            return Err(deny(
                AuditDecision::Blocked,
                "a character key whose target is not known to be a text field",
                "A character key is only sent into a text field zptah can see: here the focus is on nothing, on something that is not a text field, or on something zptah could not read, and a page can act on a key (shortcuts) or the key could land in a password field. Use browser_type or browser_fill with the ref of the field, or press it with the ref of the field.".into(),
            ));
        }
        if types {
            if read.attrs.type_unreadable || read.attrs.autocomplete_unreadable {
                return Err(deny(
                    AuditDecision::Blocked,
                    "the type of the field could not be read",
                    "Not done: zptah could not read what kind of field this is, so it did not type into it (it could be a password).".into(),
                ));
            }
            if let Some(why) = fresh::secret_field(&name, &read.attrs) {
                if rows {
                    self.post_password_notice(page);
                }
                return Err(deny(
                    AuditDecision::Invariant,
                    why,
                    format!(
                        "Not allowed: {why}. Ask the user to enter it themselves, then continue."
                    ),
                ));
            }
        }

        // The element the classifier judges first: the target, or the focused control.
        let mut element = match (&read.focus, plan.ref_.is_some()) {
            (Some(Focus::Element(el)), false) => {
                let mut e = ElementSignals {
                    role: read.role.clone(),
                    name: name.clone(),
                    label: Some(name.clone()).filter(|n| !n.is_empty()),
                    amounts: extract_amounts(&name),
                    input_type: el.input_type.clone(),
                    autocomplete: el.autocomplete.clone(),
                    input_name: el.name.clone(),
                    form_action: el
                        .form_action
                        .as_deref()
                        .filter(|a| !fresh::is_javascript(a))
                        .map(str::to_owned),
                    href: el
                        .href
                        .as_deref()
                        .filter(|h| !fresh::is_javascript(h))
                        .map(str::to_owned),
                    ..Default::default()
                };
                e.is_submit = el.form.is_some()
                    && matches!(
                        el.input_type
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .as_str(),
                        "submit" | "image"
                    );
                e
            }
            _ => fresh::element_signals(&ElementInput {
                role: &read.role,
                name: &name,
                attrs: &read.attrs,
                form: &read.form,
                ref_: read.ref_.as_deref().unwrap_or(""),
                page_url: page,
            }),
        };
        let ctx = &read.ctx;
        element.in_dialog = ctx.in_dialog || read.modal;
        if ctx.in_dialog {
            element.dialog_text = Some(clean(&ctx.dialog_text)).filter(|t| !t.is_empty());
            element.dialog_has_primary = !ctx.dialog_buttons.is_empty();
            element.dialog_primary_names = ctx.dialog_buttons.iter().map(|b| clean(b)).collect();
        }
        element.nearby_text = clean(&ctx.nearby_text);
        let heading = if ctx.heading.is_empty() {
            read.title.clone()
        } else {
            clean(&ctx.heading)
        };

        // What the action does to the page.
        let clicks =
            matches!(plan.op, Op::Click | Op::Check) || effect == Some(PressEffect::Presses);
        let submits_labels = matches!(effect, Some(PressEffect::Submits | PressEffect::Unknown))
            || (matches!(plan.op, Op::Type | Op::Fill) && plan.newline && !read.multiline);
        let scope = scope_of(read);
        let submit_like = clicks && read.submit_like || submits_labels;
        let mut unclear: Option<&str> = None;

        // The destinations: a link's address, a form's action, the button's own `formaction`.
        let mut destinations: Vec<(&str, String)> = Vec::new();
        if clicks {
            let raw = read
                .attrs
                .href
                .as_deref()
                .filter(|h| !fresh::is_javascript(h));
            if let Some(raw) = raw {
                match fresh::resolve(page, raw) {
                    Some(url) => destinations.push(("the link", url)),
                    // A link whose address can not be read is not followed blind.
                    None => destinations.push(("the link", raw.to_owned())),
                }
            }
            if read.attrs.href_unreadable || read.attrs.formaction_unreadable {
                unclear.get_or_insert("the address of the control could not be read");
            }
            if read.attrs.target_unreadable {
                unclear.get_or_insert("whether the link opens in a new window could not be read");
            }
            if let Some(url) = &element.form_action {
                if submit_like {
                    destinations.push(("the button", url.clone()));
                }
            }
        }
        if submit_like {
            let (urls, complete) = match scope {
                Scope::Own(i) => match read.actions.per_form.get(i) {
                    Some(Some(url)) => (vec![url.clone()], true),
                    _ => (Vec::new(), false),
                },
                Scope::Any => (read.actions.urls(), read.actions.complete),
            };
            for url in urls {
                destinations.push(("the form", url));
            }
            if !complete {
                unclear.get_or_insert("where the form sends its data could not be read");
            }
            if read.form_cut {
                unclear.get_or_insert("the page's form lists more fields than were read");
            }
            if !read.form.known && !read.actions.is_empty() {
                unclear.get_or_insert("the page's form could not be read");
            }
        }
        for (what, url) in &destinations {
            if let Err(refusal) = origins.check(url) {
                let shown = audit_url(url);
                return Err(deny(
                    AuditDecision::Blocked,
                    &format!("{what} leads to {shown}, which this Mono may not use"),
                    format!(
                        "Not done: {what} leads to {}. {} The page was left as it is.",
                        if shown.is_empty() {
                            "an address that is not a web page".to_owned()
                        } else {
                            shown
                        },
                        tabs_origin_refusal(refusal)
                    ),
                ));
            }
        }

        // A link that opens a new window is loaded in the session's own tab instead: it has to be
        // an address `open` takes.
        if let Some(url) = &read.new_window {
            if let Err(why) = web_url(url) {
                return Err(deny(
                    AuditDecision::Blocked,
                    "a link that opens a new window is not a web address",
                    format!("Not done: the link opens a new window and {why}"),
                ));
            }
        }

        // The classifier's inputs.
        let mut ext: BTreeSet<Category> = BTreeSet::new();
        let mut declared: BTreeSet<Category> = BTreeSet::new();
        if matches!(effect, Some(PressEffect::Unknown)) {
            unclear.get_or_insert("the focus could not be identified");
        }
        if matches!(effect, Some(PressEffect::Unclear)) {
            unclear.get_or_insert(
                "a Delete or Backspace key where no text field is known to have the focus",
            );
        }
        if read.ctx_unreliable {
            unclear
                .get_or_insert("what the page says around this control could not be matched to it");
        }
        let vi = looks_vietnamese(&format!("{} {} {}", read.title, name, element.nearby_text));
        // A bare word (Confirm, OK, Yes ...) takes its meaning from the dialog or the page.
        let lex = lexicon();
        if clicks && lex.takes_context(&name, ControlKind::Button, vi) && !ctx.readable() {
            unclear.get_or_insert("a bare word with no dialog or page text to explain it");
        }
        let mut cands: Vec<Cand> = Vec::new();
        let field_kind = || fresh::field_kind(&name, &read.attrs);
        match plan.op {
            Op::Click | Op::Check => cands.push(Cand {
                kind: Some(ActionKind::Click),
                element: element.clone(),
            }),
            Op::Select => cands.push(Cand {
                kind: Some(ActionKind::Select),
                element: element.clone(),
            }),
            Op::Upload => cands.push(Cand {
                kind: Some(ActionKind::Upload),
                element: element.clone(),
            }),
            Op::Hover => cands.push(Cand {
                kind: None,
                element: element.clone(),
            }),
            Op::Type | Op::Fill => cands.push(Cand {
                kind: Some(ActionKind::Type(field_kind())),
                element: element.clone(),
            }),
            Op::Press => match effect.unwrap_or(PressEffect::Nothing) {
                PressEffect::Nothing | PressEffect::Unclear | PressEffect::Refused => {
                    cands.push(Cand {
                        kind: None,
                        element: element.clone(),
                    })
                }
                PressEffect::Types => cands.push(Cand {
                    kind: Some(ActionKind::Type(field_kind())),
                    element: element.clone(),
                }),
                PressEffect::Presses => {
                    // Enter or Space presses the control: a click on it.
                    cands.push(Cand {
                        kind: Some(ActionKind::Click),
                        element: element.clone(),
                    })
                }
                PressEffect::Submits | PressEffect::Unknown => {}
            },
        }
        if submits_labels {
            // The submit buttons in scope, each judged as a click on it.
            let labels: Vec<String> = submit_labels(read).iter().map(|l| clean(l)).collect();
            let labels = if labels.is_empty() {
                vec![String::new()]
            } else {
                labels
            };
            for label in labels {
                // What the page says around this button, read like a click on it reads it.
                let around = read.submit_ctx.get(&label).filter(|c| c.found);
                let readable = around.is_some_and(PageContext::readable) || ctx.readable();
                if lex.takes_context(&label, ControlKind::Submit, vi) && !readable {
                    unclear.get_or_insert("a bare word with no dialog or page text to explain it");
                }
                let mut b = button(&label);
                b.in_dialog = element.in_dialog;
                if let Some(around) = around {
                    b.in_dialog |= around.in_dialog;
                    if around.in_dialog {
                        b.dialog_text = Some(clean(&around.dialog_text)).filter(|t| !t.is_empty());
                        b.dialog_has_primary = !around.dialog_buttons.is_empty();
                        b.dialog_primary_names =
                            around.dialog_buttons.iter().map(|n| clean(n)).collect();
                    }
                    b.nearby_text = clean(&around.nearby_text);
                }
                cands.push(Cand {
                    kind: Some(ActionKind::Click),
                    element: b,
                });
            }
        }

        let mut taint_note = String::new();
        if let Some(why) = unclear {
            ext.insert(Category::Unclear);
            taint_note = why.to_owned();
        }
        // Pasting puts text the gate never sees into the field.
        if key.is_some_and(fresh::is_paste_chord) {
            declared.insert(Category::CredentialEntry);
        }
        // A textbox whose kind could not be read, on a sign-in page: asks first.
        if types
            && read.attrs.input_type.is_none()
            && (is_field_role(&read.role) || read.role.is_empty())
            && fresh::login_like(page, &read.title, &read.form)
        {
            declared.insert(Category::CredentialEntry);
        }

        let new_window_load: Option<String> = read
            .new_window
            .clone()
            .filter(|_| plan.op == Op::Click || effect == Some(PressEffect::Presses));
        let evaluate_one =
            |cand: &Cand, extra: &BTreeSet<Category>| -> Result<Evaluation, String> {
                let legacy = classifier_name(&plan.tool);
                let mut action = TrustAction::new(legacy, &plan.args, page);
                action.kind = cand.kind;
                // A link that opens a window of its own is loaded in this tab: judged as the
                // navigation it is too (an address outside the task, a sensitive site, data
                // read elsewhere carried in the address).
                action.target_url = new_window_load.as_deref();
                action.form = read.form.signals();
                action.element = cand.element.clone();
                action.page_title = &read.title;
                action.page_heading = &heading;
                action.page_vi = vi;
                action.typed = plan.typed.as_deref();
                action.declared.extend(declared.iter().copied());
                action.ext = if plan.op == Op::Hover {
                    BTreeSet::new()
                } else {
                    ext.union(extra).copied().collect()
                };
                self.with_trust(id, |state, cfg| {
                    state.evaluate(
                        &TrustConfig {
                            level: self.level_now(cfg),
                            rules: &cfg.rules,
                        },
                        &action,
                    )
                })
                .ok_or_else(|| "This browser has no act settings loaded.".to_owned())
            };
        let none = BTreeSet::new();
        let first_pass: Vec<Evaluation> = cands
            .iter()
            .map(|c| evaluate_one(c, &none))
            .collect::<Result<_, _>>()?;
        let ev = {
            // Every candidate counts: the first that denies decides; otherwise the one with the
            // most categories is judged again with the categories of all the others added.
            let mut ev = first_pass
                .iter()
                .find(|e| matches!(e.decision, Decision::Deny(_)))
                .cloned();
            if ev.is_none() {
                let all: BTreeSet<Category> = first_pass
                    .iter()
                    .flat_map(|e| e.categories.iter().copied())
                    .collect();
                let at = first_pass
                    .iter()
                    .enumerate()
                    .max_by_key(|(i, e)| (e.categories.len(), std::cmp::Reverse(*i)))
                    .map_or(0, |(i, _)| i);
                let extra: BTreeSet<Category> = all
                    .difference(&first_pass[at].categories)
                    .copied()
                    .collect();
                ev = Some(if extra.is_empty() {
                    first_pass[at].clone()
                } else {
                    evaluate_one(&cands[at], &extra)?
                });
            }
            ev.ok_or("There was nothing to judge.")?
        };
        if let Decision::Deny(reason) = &ev.decision {
            let audit = if reason.contains("password") {
                AuditDecision::Invariant
            } else {
                AuditDecision::Blocked
            };
            if rows {
                self.row(
                    id,
                    &plan.tool,
                    page,
                    &plan.summary,
                    self.is_user_tab(id, &read.tab),
                    Some(&ev),
                    audit,
                    reason,
                    None,
                );
            }
            return Err(format!("Not allowed: {reason}."));
        }
        let mut ev = ev;
        if !taint_note.is_empty() && !ev.reason.starts_with("soul rule") {
            ev.reason = format!("{}; {taint_note}", ev.reason);
        }
        let sig = Sig {
            url: read.url.clone(),
            role: read.role.clone(),
            name: read.name.clone(),
            attrs: read.attrs.clone(),
            per_form: read.actions.per_form.clone(),
            complete: read.actions.complete,
            dialog: (element.in_dialog, ctx.dialog_text.clone()),
            focus: read.focus.clone(),
            submit_ctx: {
                let mut all: Vec<(String, PageContext)> = read
                    .submit_ctx
                    .iter()
                    .map(|(l, c)| (l.clone(), c.clone()))
                    .collect();
                all.sort_by(|a, b| a.0.cmp(&b.0));
                all
            },
            categories: ev.categories.clone(),
        };
        Ok(Judged { ev, sig })
    }
}
