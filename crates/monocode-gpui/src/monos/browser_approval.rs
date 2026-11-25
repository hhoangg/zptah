//! The approval side of the zptah Browser extension in the Mono manager (spec 8.2, 8.3, 7.5, 7.6).
//!
//! The act tools of a Mono park on its approval queue until the user answers. This file keeps one
//! subscription per Mono (a queue nobody listens to denies at once), turns the queue, the stop flag
//! and the rule status into the `Surface` the Mono's conversation shows, and answers what that
//! surface emits (Resume, the rule prompt). The Settings > Browser cards live here too: the preview
//! switch, the conversation grants, the soul rule confirmation and the app language.
use super::browser_ui::{action, note};
use super::panels::{card, setting};
use super::*;
use crate::conversation::browser_cards::{BrowserAction, ReviewPrompt, Surface};
use monocode_core::mono::browser::copy::{self, Lang};
use monocode_core::mono::browser::risk::Category;
use monocode_core::mono::browser::trust::approval::{self, Subscription};
use monocode_core::mono::browser::trust::rules::{self, RuleStatus};
use monocode_core::mono::browser::{MonoBrowser, gate};
use std::time::Duration;

/// The saved language choice.
pub(super) const LANGUAGE_KEY: &str = "mono-app-language";
const TICK: Duration = Duration::from_secs(1);
/// The soul file is read again this often (in ticks) to notice a changed rule list.
const RULE_TICKS: u64 = 5;

/// Which language the app UI (and the approval cards) use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum LanguagePref {
    #[default]
    System,
    Vi,
    En,
}

impl LanguagePref {
    pub(crate) const ALL: [LanguagePref; 3] = [Self::System, Self::Vi, Self::En];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Vi => "vi",
            Self::En => "en",
        }
    }

    pub(crate) fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|p| p.id() == id.trim())
            .unwrap_or_default()
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Vi => "Tiếng Việt",
            Self::En => "English",
        }
    }

    /// The language the copy is written in.
    pub(crate) fn resolve(self) -> Lang {
        match self {
            Self::Vi => Lang::Vi,
            Self::En => Lang::En,
            Self::System => system_language(),
        }
    }
}

/// The system language. macOS apps started from the Dock have no `LANG`, so the user's language
/// list is asked first.
fn system_language() -> Lang {
    static SYSTEM: std::sync::OnceLock<Lang> = std::sync::OnceLock::new();
    *SYSTEM.get_or_init(read_system_language)
}

fn read_system_language() -> Lang {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        && let Some(first) = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| {
                l.trim()
                    .trim_matches(|c| matches!(c, '(' | ')' | ',' | '"' | ' '))
            })
            .find(|l| !l.is_empty())
    {
        return Lang::from_locale(first);
    }
    Lang::system()
}

/// What the Settings > Browser cards keep between frames.
pub(super) struct ApprovalUi {
    subs: HashMap<String, Subscription>,
    wake: async_channel::Sender<String>,
    wake_rx: Option<async_channel::Receiver<String>>,
    _pump: Option<gpui::Task<()>>,
    _tick: Option<gpui::Task<()>>,
    ticks: u64,
    /// "Keep the old rules" answers, by Mono: the changed lines the prompt was hidden for.
    kept: HashMap<String, Vec<String>>,
    /// The last rule status of each Mono that uses the extension.
    rules: HashMap<String, RuleStatus>,
    /// What each conversation was last given, to skip identical pushes.
    keys: HashMap<String, String>,
    /// The active Mono's conversation grants, as `(site, category)`.
    pub(super) grants: Vec<(String, Category)>,
    pub(super) language: LanguagePref,
    pub(super) error: Option<String>,
}

impl ApprovalUi {
    pub(super) fn new() -> Self {
        let (wake, wake_rx) = async_channel::unbounded();
        Self {
            subs: HashMap::new(),
            wake,
            wake_rx: Some(wake_rx),
            _pump: None,
            _tick: None,
            ticks: 0,
            kept: HashMap::new(),
            rules: HashMap::new(),
            keys: HashMap::new(),
            grants: Vec::new(),
            language: LanguagePref::default(),
            error: None,
        }
    }
}

fn category_label(category: Category) -> String {
    category.as_str().replace('_', " ")
}

impl Monos {
    /// Starts watching the Mono approval queues. Call once, after the roster is loaded.
    pub(super) fn browser_approval_start(&mut self, cx: &mut Context<Self>) {
        let saved = locked(&self.store)
            .ok()
            .and_then(|s| s.setting(LANGUAGE_KEY).ok().flatten())
            .map(|id| LanguagePref::from_id(&id))
            .unwrap_or_default();
        self.browser_ui.approval.language = saved;
        // Tests keep the process wide default unless a language was chosen on purpose.
        if !cx.is_test() || saved != LanguagePref::System {
            self.browser_apply_language(saved, cx);
        }
        self.browser_approval_sync(cx);
        if let Some(rx) = self.browser_ui.approval.wake_rx.take() {
            self.browser_ui.approval._pump = Some(cx.spawn(async move |this, cx| {
                while let Ok(mono_id) = rx.recv().await {
                    if this
                        .update(cx, |this, cx| {
                            this.browser_approval_refresh(&mono_id, false, cx)
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        if !cx.is_test() {
            self.browser_ui.approval._tick = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    if this
                        .update(cx, |this, cx| this.browser_approval_tick(cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
    }

    /// One subscription per Mono in the roster; those of removed Monos go.
    pub(super) fn browser_approval_sync(&mut self, _cx: &mut Context<Self>) {
        let ids: HashSet<String> = self.roster.monos.iter().map(|m| m.id.clone()).collect();
        let ui = &mut self.browser_ui.approval;
        ui.subs.retain(|id, _| ids.contains(id));
        ui.rules.retain(|id, _| ids.contains(id));
        for id in ids {
            if ui.subs.contains_key(&id) {
                continue;
            }
            let wake = ui.wake.clone();
            let mono = id.clone();
            let sub = approval::queue_for(&id).subscribe(move || {
                let _ = wake.try_send(mono.clone());
            });
            ui.subs.insert(id, sub);
        }
    }

    /// The Mono's browser is on (the extension is the only backend).
    fn browser_uses_extension(&self, mono_id: &str) -> bool {
        self.roster
            .get(mono_id)
            .and_then(|m| m.browser.as_ref())
            .is_some_and(|b| b.enabled)
    }

    fn browser_approval_tick(&mut self, cx: &mut Context<Self>) {
        self.browser_ui.approval.ticks += 1;
        let with_rules = self.browser_ui.approval.ticks.is_multiple_of(RULE_TICKS);
        let ids: Vec<String> = self
            .roster
            .monos
            .iter()
            .filter(|m| m.browser.as_ref().is_some_and(|b| b.enabled))
            .map(|m| m.id.clone())
            .collect();
        for id in ids {
            self.browser_approval_refresh(&id, with_rules, cx);
        }
    }

    /// Reads the soul and the confirmed rules on a worker, then refreshes the Mono's surface.
    /// `rules::status` also records a first or pure-addition set as confirmed.
    fn browser_rules_reload(&mut self, mono_id: &str, cx: &mut Context<Self>) {
        if !self.browser_uses_extension(mono_id) {
            self.browser_ui.approval.rules.remove(mono_id);
            return;
        }
        let Ok(store) = self.browser_store() else {
            return;
        };
        let (root, id) = (self.root.clone(), mono_id.to_owned());
        cx.spawn(async move |this, cx| {
            let work = {
                let id = id.clone();
                cx.background_executor().spawn(async move {
                    let soul = mono::files::read(&root, &id, None, "SOUL.md").ok()?;
                    Some(rules::status(&store, &id, &soul.text.unwrap_or_default()))
                })
            };
            let status = work.await;
            let _ = this.update(cx, |this, cx| {
                match status {
                    Some(status) => this.browser_ui.approval.rules.insert(id.clone(), status),
                    None => this.browser_ui.approval.rules.remove(&id),
                };
                this.browser_approval_refresh(&id, false, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Builds the surface of one Mono and gives it to its conversation when it changed (a card
    /// with a countdown makes every call count as a change).
    pub(super) fn browser_approval_refresh(
        &mut self,
        mono_id: &str,
        with_rules: bool,
        cx: &mut Context<Self>,
    ) {
        let uses = self.browser_uses_extension(mono_id);
        if with_rules {
            self.browser_rules_reload(mono_id, cx);
        }
        let queue = approval::queue_for(mono_id);
        let label = self
            .roster
            .get(mono_id)
            .map(|m| m.label())
            .unwrap_or_default();
        let review = self
            .browser_ui
            .approval
            .rules
            .get(mono_id)
            .filter(|s| s.needs_confirmation)
            .map(|s| s.changed_lines.clone())
            .filter(|lines| self.browser_ui.approval.kept.get(mono_id) != Some(lines))
            .map(|changed_lines| ReviewPrompt {
                mono_label: label,
                changed_lines,
            });
        let surface = Surface {
            cards: queue.pending(),
            notices: queue.notices(),
            stopped: uses && gate::upstream_for(mono_id).is_some_and(|u| u.stopped()),
            review,
        };
        let key = format!(
            "{:?}|{:?}|{}|{:?}",
            surface.cards.iter().map(|c| &c.id).collect::<Vec<_>>(),
            surface.notices.iter().map(|n| &n.id).collect::<Vec<_>>(),
            surface.stopped,
            surface.review
        );
        let changed = self.browser_ui.approval.keys.get(mono_id) != Some(&key);
        let counting = !surface.cards.is_empty();
        self.browser_ui
            .approval
            .keys
            .insert(mono_id.to_owned(), key);
        if let Some(chat) = self.chats.get(mono_id).cloned()
            && (changed || counting)
        {
            chat.update(cx, |chat, cx| chat.set_browser_surface(surface, cx));
            // The countdown only repaints; the notification layer hears about real changes.
            if changed {
                cx.emit(NotificationChanged);
            }
        }
        if changed && self.settings == Some(SettingsSection::Browser) {
            self.browser_approval_page_refresh(cx);
        }
    }

    /// A conversation was created: it learns its Mono and what is already waiting.
    pub(super) fn browser_approval_attach(
        &mut self,
        mono_id: &str,
        chat: &Entity<Conversation>,
        cx: &mut Context<Self>,
    ) {
        chat.update(cx, |chat, _| chat.set_browser_mono(mono_id));
        let source = mono_id.to_owned();
        self.observations.push(
            cx.subscribe(chat, move |this, _, event: &BrowserAction, cx| {
                this.browser_action(&source, *event, cx)
            }),
        );
        self.browser_ui.approval.keys.remove(mono_id);
    }

    /// Resume, Confirm or Keep from a conversation's surface.
    pub(super) fn browser_action(
        &mut self,
        mono_id: &str,
        event: BrowserAction,
        cx: &mut Context<Self>,
    ) {
        match event {
            BrowserAction::Resume => {
                if let Some(upstream) = gate::upstream_for(mono_id) {
                    let id = mono_id.to_owned();
                    cx.spawn(async move |this, cx| {
                        // The request goes over the link and can take a moment.
                        let result = cx
                            .background_executor()
                            .spawn(async move { upstream.resume() })
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            this.browser_ui.approval.error = result.err();
                            this.browser_approval_refresh(&id, false, cx);
                        });
                    })
                    .detach();
                }
            }
            BrowserAction::ConfirmRules => self.browser_rules_confirm(mono_id, cx),
            BrowserAction::KeepRules => {
                if let Some(status) = self.browser_ui.approval.rules.get(mono_id) {
                    let lines = status.changed_lines.clone();
                    self.browser_ui
                        .approval
                        .kept
                        .insert(mono_id.to_owned(), lines);
                }
                self.browser_approval_refresh(mono_id, false, cx);
            }
        }
    }

    /// "Confirm the new rules": the soul's rule lines become the confirmed set (read and written
    /// on a worker).
    pub(super) fn browser_rules_confirm(&mut self, mono_id: &str, cx: &mut Context<Self>) {
        let store = match self.browser_store() {
            Ok(store) => store,
            Err(error) => {
                self.browser_ui.approval.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.browser_ui.approval.kept.remove(mono_id);
        let (root, id) = (self.root.clone(), mono_id.to_owned());
        cx.spawn(async move |this, cx| {
            let work = {
                let id = id.clone();
                cx.background_executor().spawn(async move {
                    let soul = mono::files::read(&root, &id, None, "SOUL.md")?;
                    rules::confirm(&store, &id, &soul.text.unwrap_or_default())
                })
            };
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.browser_ui.approval.error = result.err();
                this.browser_rules_reload(&id, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads the grants and the rule status the Settings cards show.
    pub(super) fn browser_approval_page_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let grants = self
            .browser_store()
            .map(|s| s.list_grants(&id, None))
            .unwrap_or_default();
        self.browser_ui.approval.grants = grants;
        self.browser_rules_reload(&id, cx);
        cx.notify();
    }

    /// Ends the grants of the active Mono: one site's, or all.
    pub(super) fn browser_revoke(&mut self, site: Option<String>, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let result = self
            .browser_store()
            .and_then(|s| s.revoke_grants(&id, None, site.as_deref()));
        self.browser_ui.approval.error = result.err();
        self.browser_approval_page_refresh(cx);
    }

    /// Sets the copy language. "System" asks the OS, which can run a process, so it happens on a
    /// worker; the cards keep the old language until the answer comes.
    fn browser_apply_language(&mut self, pref: LanguagePref, cx: &mut Context<Self>) {
        match pref {
            LanguagePref::System => {
                cx.spawn(async move |this, cx| {
                    let lang = cx
                        .background_executor()
                        .spawn(async { system_language() })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        // A newer choice wins over a slow system lookup.
                        if this.browser_ui.approval.language == LanguagePref::System {
                            copy::set_app_language(lang);
                            this.browser_repaint_chats(cx);
                        }
                    });
                })
                .detach();
            }
            explicit => copy::set_app_language(explicit.resolve()),
        }
    }

    fn browser_repaint_chats(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        for chat in self.chats.values() {
            chat.update(cx, |_, cx| cx.notify());
        }
    }

    pub(super) fn browser_set_language(&mut self, pref: LanguagePref, cx: &mut Context<Self>) {
        self.browser_ui.approval.language = pref;
        self.browser_apply_language(pref, cx);
        let saved = locked(&self.store).and_then(|s| s.save_setting(LANGUAGE_KEY, pref.id()));
        self.browser_ui.approval.error = saved.err();
        // The cards on screen take the new language on the next frame.
        self.browser_repaint_chats(cx);
    }

    /// The cards of the Browser page that belong to the extension: language, rule review, grants.
    pub(super) fn browser_extension_cards(
        &mut self,
        mono: &Mono,
        browser: &MonoBrowser,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut cards = Vec::new();
        let ui = &self.browser_ui.approval;
        let language = ui.language;
        let error = ui.error.clone();
        let chips: Vec<AnyElement> = LanguagePref::ALL
            .into_iter()
            .map(|pref| {
                let id: SharedString = format!("mono-browser-language-{}", pref.id()).into();
                let selected = pref == language;
                div()
                    .id(id.clone())
                    .debug_selector({
                        let id = id.clone();
                        move || id.to_string()
                    })
                    .role(Role::Button)
                    .aria_label(pref.label())
                    .px_2p5()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(content(if selected { 0.3 } else { 0.1 }))
                    .text_size(px(12.))
                    .text_color(content(if selected { 0.95 } else { 0.7 }))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.browser_set_language(pref, cx)),
                    )
                    .child(pref.label())
                    .into_any_element()
            })
            .collect();
        cards.push(
            card()
                .child(setting(
                    "Language",
                    Some("The language of the approval cards and the browser notices."),
                    div().flex().gap_2().children(chips).into_any_element(),
                ))
                .into_any_element(),
        );
        if !browser.enabled {
            return cards;
        }
        let status = self.browser_ui.approval.rules.get(&mono.id).cloned();
        if let Some(status) = status.filter(|s| s.needs_confirmation) {
            let text =
                copy::rule_review(copy::app_language(), &mono.label(), &status.changed_lines);
            let id = mono.id.clone();
            cards.push(
                card()
                    .child(setting(
                        &text.headline,
                        Some(&text.line),
                        action("mono-browser-rules-confirm", text.confirm)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.browser_rules_confirm(&id, cx)
                            }))
                            .into_any_element(),
                    ))
                    .into_any_element(),
            );
        }
        let grants = self.browser_ui.approval.grants.clone();
        let mut grants_card = card().child(setting(
            "Allowed in conversations",
            Some("Sites where you chose Allow in this conversation. They end with the conversation, and here."),
            if grants.is_empty() {
                div().into_any_element()
            } else {
                action("mono-browser-grants-revoke-all", "Revoke all")
                    .on_click(cx.listener(|this, _, _, cx| this.browser_revoke(None, cx)))
                    .into_any_element()
            },
        ));
        if grants.is_empty() {
            grants_card = grants_card.child(div().px_3().pb_2().child(note(
                "mono-browser-grants-empty",
                "Nothing is allowed right now.",
                0.6,
            )));
        }
        for (ix, (site, category)) in grants.iter().enumerate() {
            let target = site.clone();
            let row_id: SharedString = format!("mono-browser-grant-revoke-{ix}").into();
            grants_card = grants_card.child(setting(
                &format!("{site}: {}", category_label(*category)),
                None,
                crate::modal::ghost_button(row_id.clone(), "Revoke")
                    .debug_selector({
                        let row_id = row_id.clone();
                        move || row_id.to_string()
                    })
                    .border_1()
                    .border_color(content(0.1))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browser_revoke(Some(target.clone()), cx)
                    }))
                    .into_any_element(),
            ));
        }
        if let Some(error) = error {
            grants_card = grants_card.child(div().px_3().pb_2().child(note(
                "mono-browser-approval-error",
                error,
                0.7,
            )));
        }
        cards.push(grants_card.into_any_element());
        cards
    }
}
