//! The fresh read of an act tool (design spec section 6, "Act tools: fresh read, then act").
//!
//! Right before an action the gate reads the page again: the tab URL and title, the snapshot
//! refs, and for the target its `type`, `autocomplete`, `href`, `formaction`, `id` and
//! `placeholder`, the page's `form` and the `action` of every form (`get count form`, then
//! `form >> nth=i`), and, for a click that needs it, the dialog or page text from the full
//! `snapshot`. A key with no ref reads the focused element with one fixed `eval`
//! ([`super::focus`]).
//!
//! The call fails with "the element changed, take a new snapshot" when the ref was never in a
//! snapshot the Mono took, no longer resolves, has another role or name, or the tab is on another
//! URL than at that snapshot. A command that is refused is not the same as an attribute that is
//! absent: the first leaves the gate not knowing (it refuses to type, it asks to click), the
//! second is a fact.
use super::act::{is_field_role, is_stale_code, Op, Plan, NEVER_SEEN};
use super::focus::{self, Focus};
use super::fresh::{self, AttrRead, Attrs, FormActions, FormRead, CHANGED, MAX_FORMS};
use super::judge;
use super::page::{self, PageContext};
use super::*;
use crate::mono::browser::gate::origins::looks_vietnamese;
use crate::mono::browser::risk::lexicon::{lexicon, ControlKind};
use crate::mono::browser::trust::audit::AuditDecision;

/// Everything the gate read from the page right before the action.
#[derive(Default)]
pub(super) struct Read {
    pub tab: String,
    pub url: String,
    pub title: String,
    /// The element the action lands on: the target ref.
    pub ref_: Option<String>,
    pub role: String,
    pub name: String,
    pub attrs: Attrs,
    pub form: FormRead,
    pub actions: FormActions,
    /// What the full snapshot says around the target (asked for only when it matters).
    pub ctx: PageContext,
    /// What the full snapshot says around the submit buttons an Enter (or a line break in typed
    /// text) would press, by cleaned label: asked for when a label is a bare word (Continue, OK,
    /// Confirm, Xác nhận ...) that takes its meaning from its surroundings.
    pub submit_ctx: HashMap<String, PageContext>,
    /// The `snapshot -i` was scoped to an open dialog.
    pub modal: bool,
    /// For a key with no ref: what has the focus (`None`: not asked).
    pub focus: Option<Focus>,
    /// The target is a multiline field (Enter in it is a line break).
    pub multiline: bool,
    /// The page's `form` reply listed fewer fields than the page has.
    pub form_cut: bool,
    /// The action may submit a form (the form actions were read).
    pub submit_like: bool,
    /// A click on a link that opens a new window (`target` set and not `_self`, `_top` or
    /// `_parent`): the resolved address, which the gate loads in the session's own tab instead.
    pub new_window: Option<String>,
    /// The full snapshot could not be matched to the target: its line is not there exactly once
    /// with the same role and name, or the page is modal and the dialog could not be found.
    pub ctx_unreliable: bool,
}

/// What a key does once it lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PressEffect {
    /// Nothing the gate needs to judge (a scroll, Tab, Delete, a key with nothing to act on).
    Nothing,
    /// A character goes into a field.
    Types,
    /// Enter or Space presses a button, a link or a checkbox.
    Presses,
    /// Enter in a field submits its form.
    Submits,
    /// The focus could not be identified, so what Enter or Space does is not known.
    Unknown,
    /// A character key (not Space) whose target is not known to be a text field: nothing is sent.
    Refused,
    /// Delete or Backspace outside a text field, or with a focus that is not known: asks.
    Unclear,
}

/// What the key of `plan` does, given what was read.
///
/// A character key (not Space) is sent only into a text field the gate can see: anywhere else it
/// may be a page shortcut or land in a field the gate could not read. Delete and Backspace outside
/// a text field, or with an unknown focus, ask. Tab, the arrows, Escape, PageUp, PageDown, Home and
/// End are never judged.
pub(super) fn press_effect(plan: &Plan, read: &Read) -> PressEffect {
    let Some(key) = plan.key.as_deref() else {
        return PressEffect::Nothing;
    };
    let prints = fresh::is_printable_key(key);
    let enter = fresh::is_submit_key(key);
    let activates = enter || fresh::is_space_key(key);
    let deletes = fresh::is_delete_key(key);
    if !prints && !enter && !deletes {
        return PressEffect::Nothing;
    }
    let character = prints && !fresh::is_space_key(key);
    // What a key does where it is neither typed into a text field nor presses a control.
    let elsewhere = || {
        if character {
            PressEffect::Refused
        } else if deletes {
            PressEffect::Unclear
        } else {
            PressEffect::Nothing
        }
    };
    if plan.ref_.is_some() {
        return if is_field_role(&read.role) {
            if enter {
                if read.multiline {
                    PressEffect::Nothing
                } else {
                    PressEffect::Submits
                }
            } else if deletes {
                PressEffect::Nothing
            } else {
                PressEffect::Types
            }
        } else if activates {
            PressEffect::Presses
        } else {
            elsewhere()
        };
    }
    match &read.focus {
        None | Some(Focus::Unknown) => {
            if activates {
                PressEffect::Unknown
            } else {
                elsewhere()
            }
        }
        Some(Focus::Nothing) => elsewhere(),
        Some(Focus::Element(el)) => {
            if el.is_text_entry() {
                if enter {
                    if el.is_textarea() {
                        PressEffect::Nothing
                    } else {
                        PressEffect::Submits
                    }
                } else if deletes {
                    PressEffect::Nothing
                } else {
                    PressEffect::Types
                }
            } else if el.is_pressable() || el.is_checkable() {
                if activates {
                    PressEffect::Presses
                } else {
                    elsewhere()
                }
            } else if el.tag.eq_ignore_ascii_case("select") {
                if activates {
                    PressEffect::Nothing
                } else {
                    elsewhere()
                }
            } else if activates {
                // A custom control (a `div` with a role): what it does is not known.
                PressEffect::Unknown
            } else {
                elsewhere()
            }
        }
    }
}

impl ArdiaUpstream {
    /// One attribute: its value, absent (`value: null`) or not readable (the command was refused).
    pub(super) fn attr_read(&self, id: i64, target: &str, name: &str) -> Result<AttrRead, String> {
        match self.browse(id, &["get", "attr"], &[target, name]) {
            Ok(data) => Ok(match fresh::attr_value(&data) {
                Some(value) => AttrRead::Value(value),
                None => AttrRead::Absent,
            }),
            Err(Failure::Refused { code, .. }) if is_stale_code(code.as_deref()) => {
                Err(CHANGED.to_owned())
            }
            Err(Failure::Refused { .. }) => Ok(AttrRead::Unreadable),
            Err(other) => Err(self.fail(id, other)),
        }
    }

    /// Reads one attribute of `at` into `slot`; a refused read is noted in `unreadable`.
    fn probe(
        &self,
        id: i64,
        at: &str,
        name: &str,
        slot: &mut Option<String>,
        unreadable: &mut bool,
    ) -> Result<(), String> {
        match self.attr_read(id, at, name)? {
            AttrRead::Value(v) => *slot = Some(v),
            AttrRead::Absent => {}
            AttrRead::Unreadable => *unreadable = true,
        }
        Ok(())
    }

    fn form_now(&self, id: i64) -> Result<FormRead, String> {
        match self.browse(id, &["form"], &[]) {
            Ok(data) => Ok(FormRead::parse(&data)),
            Err(Failure::Refused { .. }) => Ok(FormRead::default()),
            Err(other) => Err(self.fail(id, other)),
        }
    }

    /// The focused element, by one fixed `eval` (the snapshot never marks the focus). Anything
    /// but a clear answer is [`Focus::Unknown`].
    fn focus_now(&self, id: i64) -> Result<Focus, String> {
        // `--isolated`: without it the script runs in the page's own world, where a page that
        // replaced `JSON.stringify` could make a focused password field read as `BODY`.
        match self.browse(id, &["eval", "--isolated"], &[focus::FOCUS_SCRIPT]) {
            Ok(data) => Ok(focus::parse(&data)),
            Err(Failure::Refused { .. }) => Ok(Focus::Unknown),
            Err(other) => Err(self.fail(id, other)),
        }
    }

    /// The `action` of every form of the page, resolved against the page address: `get count
    /// form`, then `get attr "form >> nth=i" action` for each (at most [`MAX_FORMS`]). A form with
    /// no `action` posts to the page itself. The read is incomplete when the count could not be
    /// read, there are more forms than are read, an action could not be read or resolved, or the
    /// page's `form` reply says it is cut.
    fn form_actions_now(
        &self,
        id: i64,
        page: &str,
        form: &FormRead,
    ) -> Result<FormActions, String> {
        let mut out = FormActions {
            per_form: Vec::new(),
            complete: true,
            count: None,
        };
        let count = match self.browse(id, &["get", "count"], &["form"]) {
            Ok(data) => data["count"].as_u64().map(|n| n as usize),
            Err(Failure::Refused { code, .. }) if is_stale_code(code.as_deref()) => {
                return Err(CHANGED.to_owned())
            }
            Err(Failure::Refused { .. }) => None,
            Err(other) => return Err(self.fail(id, other)),
        };
        let Some(count) = count else {
            out.complete = false;
            return Ok(out);
        };
        out.count = Some(count);
        if form.more {
            out.complete = false;
        }
        if count == 0 {
            // A page with fields and no form element: where a submit goes is not known.
            if form.known && (!form.fields.is_empty() || !form.submits.is_empty()) {
                out.complete = false;
            }
            return Ok(out);
        }
        if count > MAX_FORMS {
            out.complete = false;
        }
        for i in 0..count.min(MAX_FORMS) {
            let target = format!("form >> nth={i}");
            match self.attr_read(id, &target, "action")? {
                AttrRead::Value(raw) => match fresh::resolve(page, &raw) {
                    Some(url) => out.per_form.push(Some(url)),
                    None => {
                        out.per_form.push(None);
                        out.complete = false;
                    }
                },
                AttrRead::Absent => out.per_form.push(Some(page.to_owned())),
                AttrRead::Unreadable => {
                    out.per_form.push(None);
                    out.complete = false;
                }
            }
        }
        Ok(out)
    }

    pub(super) fn fresh_read(&self, id: i64, tab: &str, plan: &Plan) -> Result<Read, String> {
        self.fresh_read_rows(id, tab, plan, true)
    }

    /// [`ArdiaUpstream::fresh_read`]; with `rows` false a refusal writes no audit row (the read
    /// made again after a card: the caller writes the one row).
    pub(super) fn fresh_read_rows(
        &self,
        id: i64,
        tab: &str,
        plan: &Plan,
        rows: bool,
    ) -> Result<Read, String> {
        let refuse = |read: &Read, decision: AuditDecision, reason: &str, text: String| {
            if rows {
                self.refuse(id, plan, read, decision, reason, text)
            } else {
                text
            }
        };
        let mut read = Read {
            tab: tab.to_owned(),
            url: self.url_now(id)?,
            ..Read::default()
        };
        let config = self.config();
        // The page the action would happen on must be one this Mono may use.
        if let Err(refusal) = config.origins.check(&read.url) {
            let text = format!(
                "{} Nothing was done: the tab is on a page this Mono may not use.",
                tabs_origin_refusal(refusal)
            );
            return Err(refuse(
                &read,
                AuditDecision::Blocked,
                "the page the action would happen on is not allowed",
                text,
            ));
        }
        read.title = match self.browse(id, &["get", "title"], &[]) {
            Ok(data) => data["title"].as_str().unwrap_or("").to_owned(),
            Err(Failure::Refused { .. }) => String::new(),
            Err(other) => return Err(self.fail(id, other)),
        };
        let key = plan.key.as_deref();
        let key_acts = key.is_some_and(|k| {
            fresh::is_printable_key(k) || fresh::is_submit_key(k) || fresh::is_delete_key(k)
        });
        if plan.ref_.is_none() && !key_acts {
            return Ok(read);
        }
        if let Some(r) = &plan.ref_ {
            let seen = self
                .conv(id, |c| c.seen.get(tab).cloned())
                .unwrap_or_default();
            if !seen.refs.contains_key(r) {
                return Err(refuse(
                    &read,
                    AuditDecision::Blocked,
                    "the ref was never in a snapshot",
                    NEVER_SEEN.to_owned(),
                ));
            }
            if seen.url != read.url {
                return Err(refuse(
                    &read,
                    AuditDecision::Blocked,
                    "the tab URL changed since the snapshot",
                    CHANGED.to_owned(),
                ));
            }
            let snapshot = self
                .browse(id, &["snapshot", "-i"], &[])
                .map_err(|f| self.act_fail(id, f))?;
            let refs = fresh::refs_of(&snapshot);
            match (refs.get(r), seen.refs.get(r)) {
                (Some(now), Some(then)) if now == then => {
                    read.role = now.0.clone();
                    read.name = now.1.clone();
                    read.ref_ = Some(r.clone());
                }
                _ => {
                    return Err(refuse(
                        &read,
                        AuditDecision::Blocked,
                        "the ref no longer resolves to what the snapshot showed",
                        CHANGED.to_owned(),
                    ))
                }
            }
            read.modal = page::is_modal(&snapshot);
        } else {
            // A key with no ref: the snapshot does not mark the focus, so ask the page.
            read.focus = Some(self.focus_now(id)?);
        }
        self.read_attrs(id, plan, &mut read)?;
        self.read_context(id, plan, &mut read)?;
        Ok(read)
    }

    /// The attributes, the form and the form actions the action's risk depends on.
    fn read_attrs(&self, id: i64, plan: &Plan, read: &mut Read) -> Result<(), String> {
        let at = read.ref_.as_ref().map(|r| format!("@{r}"));
        let key = plan.key.as_deref();
        let prints = key.is_some_and(fresh::is_printable_key);
        let enters = key.is_some_and(fresh::is_submit_key);
        let field = is_field_role(&read.role);
        // A key in a ref'd button or link is read like a click on it.
        let as_click = plan.op == Op::Click
            || (plan.op == Op::Press && plan.ref_.is_some() && !field && (enters || prints));
        let as_field = matches!(plan.op, Op::Type | Op::Fill)
            || (plan.op == Op::Press && plan.ref_.is_some() && field);
        if let Some(at) = &at {
            let a = &mut read.attrs;
            if as_click {
                self.probe(id, at, "type", &mut a.input_type, &mut a.type_unreadable)?;
                self.probe(
                    id,
                    at,
                    "formaction",
                    &mut a.formaction,
                    &mut a.formaction_unreadable,
                )?;
                self.probe(id, at, "href", &mut a.href, &mut a.href_unreadable)?;
                // A link with a `target` opens a window of its own: read where it would go.
                if a.href.as_deref().is_some_and(|h| !fresh::is_javascript(h)) {
                    self.probe(id, at, "target", &mut a.target, &mut a.target_unreadable)?;
                }
            } else if as_field {
                self.probe(id, at, "type", &mut a.input_type, &mut a.type_unreadable)?;
                self.probe(
                    id,
                    at,
                    "autocomplete",
                    &mut a.autocomplete,
                    &mut a.autocomplete_unreadable,
                )?;
                // Readable when the page allows it; a refusal here is not a block.
                let mut ignored = false;
                self.probe(id, at, "id", &mut a.field_id, &mut ignored)?;
                self.probe(id, at, "placeholder", &mut a.placeholder, &mut ignored)?;
            } else if matches!(plan.op, Op::Check | Op::Upload) {
                self.probe(id, at, "type", &mut a.input_type, &mut a.type_unreadable)?;
            }
        }
        if let Some(Focus::Element(el)) = &read.focus {
            read.attrs.input_type = el.input_type.clone();
            read.attrs.autocomplete = el.autocomplete.clone();
            read.attrs.field_name = el.name.clone();
            read.attrs.field_label = el.label.clone();
            read.attrs.href = el.href.clone();
            read.attrs.formaction = el.form_action.clone();
            read.multiline = el.is_textarea();
            // The focused control stands in for a target: the classifier reads its role and name.
            read.role = if el.is_text_entry() {
                "textbox"
            } else if el.tag.eq_ignore_ascii_case("a") {
                "link"
            } else if el.is_pressable() {
                "button"
            } else if el.is_checkable() {
                "checkbox"
            } else {
                ""
            }
            .to_owned();
            read.name = el.shown_name();
        }
        if let (Some(href), Some(target)) = (&read.attrs.href, &read.attrs.target) {
            if !fresh::is_javascript(href) && fresh::opens_new_window(target) {
                read.new_window =
                    Some(fresh::resolve(&read.url, href).unwrap_or_else(|| href.clone()));
            }
        }
        let effect = (plan.op == Op::Press).then(|| press_effect(plan, read));
        let needs_form = match plan.op {
            Op::Click => read.role != "link",
            Op::Type | Op::Fill => true,
            Op::Press => !matches!(effect, Some(PressEffect::Nothing)) || at.is_some(),
            _ => false,
        };
        if needs_form {
            read.form = self.form_now(id)?;
            read.form_cut = read.form.more;
            if let Some(r) = read.ref_.clone() {
                if let Some(f) = read.form.field(&r) {
                    read.attrs.field_name = read.attrs.field_name.take().or(Some(f.name.clone()));
                    read.attrs.field_label =
                        read.attrs.field_label.take().or(Some(f.label.clone()));
                    read.multiline |= f.input_type.eq_ignore_ascii_case("textarea");
                    if read.attrs.input_type.is_none() && !f.input_type.is_empty() {
                        // `form` lists the type of every field: it fills a `get attr` that failed.
                        read.attrs.input_type = Some(f.input_type.clone());
                    }
                }
            }
            read.multiline |= read.attrs.input_type.as_deref() == Some("textarea");
            read.attrs.field_name = read.attrs.field_name.take().filter(|n| !n.is_empty());
            read.attrs.field_label = read.attrs.field_label.take().filter(|n| !n.is_empty());
        }
        // Press effects depend on the multiline flag the form just set.
        let effect = (plan.op == Op::Press).then(|| press_effect(plan, read));
        let link = read.role == "link";
        let submit_like = match plan.op {
            Op::Click => {
                !link
                    && (read.ref_.as_deref().is_some_and(|r| read.form.is_submit(r))
                        || read.attrs.input_type.as_deref() == Some("submit")
                        || !read.form.known
                        || read.attrs.input_type.is_none()
                        || read.attrs.type_unreadable)
            }
            Op::Press => match effect {
                Some(PressEffect::Submits | PressEffect::Unknown) => true,
                Some(PressEffect::Presses) => match &read.focus {
                    Some(Focus::Element(el)) => {
                        el.form.is_some()
                            && matches!(
                                el.input_type
                                    .as_deref()
                                    .unwrap_or("")
                                    .to_lowercase()
                                    .as_str(),
                                "submit" | "image"
                            )
                    }
                    _ => {
                        !link
                            && (read.ref_.as_deref().is_some_and(|r| read.form.is_submit(r))
                                || read.attrs.input_type.as_deref() == Some("submit")
                                || !read.form.known
                                || read.attrs.input_type.is_none()
                                || read.attrs.type_unreadable)
                    }
                },
                _ => false,
            },
            Op::Type | Op::Fill => plan.newline && !read.multiline,
            _ => false,
        };
        read.submit_like = submit_like;
        if submit_like {
            read.actions = self.form_actions_now(id, &read.url, &read.form)?;
        } else {
            read.actions.complete = true;
        }
        Ok(())
    }

    /// The text around a clicked control: the dialog it is in and what the dialog says, the text
    /// next to it and the heading above it, from the full `snapshot`. Asked for when the
    /// `snapshot -i` was scoped to a dialog, or when the control's name is a bare word that takes
    /// its meaning from its surroundings (Confirm, OK, Yes, Continue, Xác nhận ...).
    fn read_context(&self, id: i64, plan: &Plan, read: &mut Read) -> Result<(), String> {
        let effect = (plan.op == Op::Press).then(|| press_effect(plan, read));
        let submits_by_key = matches!(effect, Some(PressEffect::Submits | PressEffect::Unknown))
            || (matches!(plan.op, Op::Type | Op::Fill) && plan.newline && !read.multiline);
        if submits_by_key {
            return self.read_submit_context(id, read);
        }
        let clicks = plan.op == Op::Click
            || (plan.op == Op::Press
                && plan.ref_.is_some()
                && !is_field_role(&read.role)
                && plan.key.as_deref().is_some_and(fresh::is_activation_key));
        let Some(r) = read.ref_.clone().filter(|_| clicks) else {
            return Ok(());
        };
        let vi = looks_vietnamese(&format!("{} {}", read.title, read.name));
        if !read.modal && !lexicon().takes_context(&read.name, ControlKind::Button, vi) {
            return Ok(());
        }
        match self.browse(id, &["snapshot"], &[]) {
            Ok(data) => {
                read.ctx = page::context_of(
                    data["snapshot"].as_str().unwrap_or(""),
                    &r,
                    &read.role,
                    &read.name,
                );
            }
            Err(Failure::Refused { code, .. }) if is_stale_code(code.as_deref()) => {
                return Err(CHANGED.to_owned())
            }
            // Unreadable: the context stays empty, and a bare word then asks.
            Err(Failure::Refused { .. }) => {}
            Err(other) => return Err(self.fail(id, other)),
        }
        // The page says it is modal but no dialog around the control was found (or the line was
        // not the control's): the text read does not describe it.
        read.ctx_unreliable = read.ctx.mismatch || (read.modal && !read.ctx.in_dialog);
        Ok(())
    }

    /// The text around the submit buttons an Enter (or a line break in typed text) presses, for
    /// the labels that take their meaning from it, read like a click on the button reads it. A
    /// label that names no button of the page's forms exactly once is left unread (it then asks).
    fn read_submit_context(&self, id: i64, read: &mut Read) -> Result<(), String> {
        let vi = looks_vietnamese(&read.title);
        let lex = lexicon();
        let labels: Vec<String> = judge::submit_labels(read)
            .into_iter()
            .map(|l| clean(&l))
            .filter(|l| lex.takes_context(l, ControlKind::Submit, vi))
            .collect();
        if labels.is_empty() {
            return Ok(());
        }
        let text = match self.browse(id, &["snapshot"], &[]) {
            Ok(data) => data["snapshot"].as_str().unwrap_or("").to_owned(),
            Err(Failure::Refused { code, .. }) if is_stale_code(code.as_deref()) => {
                return Err(CHANGED.to_owned())
            }
            // Unreadable: the context stays empty, and a bare word then asks.
            Err(Failure::Refused { .. }) => return Ok(()),
            Err(other) => return Err(self.fail(id, other)),
        };
        for label in labels {
            let same = |l: &str| one_line(&clean(l)) == one_line(&label);
            let refs: Vec<&str> = read
                .form
                .submits
                .iter()
                .filter(|(_, l)| same(l))
                .map(|(r, _)| r.as_str())
                .collect();
            let [target] = refs.as_slice() else {
                continue;
            };
            let ctx = page::context_of(&text, target, "button", &label);
            read.ctx_unreliable |= ctx.mismatch;
            read.submit_ctx.insert(label, ctx);
        }
        Ok(())
    }
}
