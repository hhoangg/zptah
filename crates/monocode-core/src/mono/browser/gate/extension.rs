//! The gate's client for the zptah Browser extension: an [`Upstream`] that speaks MCP to the gate
//! and the link protocol (spec 5.5) to the extension. It replaces the Playwright server for a
//! Mono that uses the extension (spec 4.1, 7.1).
//!
//! This part is the read path: `browser_tabs` (list, select), `browser_snapshot`, `browser_read`,
//! `browser_find`, `browser_take_screenshot` and `browser_wait_for`. Results use the text shape
//! Playwright prints (`### Page`, `- Page URL: ...`, a fenced `yaml` snapshot, `- N: [Title](url)`
//! tab lines), so the gate's `page_url_of`, the tab parser, the recipe snapshot parser and the
//! defense layer keep working unchanged. A tool that is not implemented yet answers with a tool
//! error, never with a silent success.
//!
//! Safety rules kept here (the extension enforces the same ones; this is the second line):
//!
//! * A tab is read only when its address passes the Mono's origin lists ([`Origins`]): checked
//!   against the tab list before the request and again against the address the extension
//!   reports, so a tab that navigated in between is withheld, not shown.
//! * Browser pages (`chrome:`, the Web Store and the like) are never read.
//! * Nothing here activates, focuses or changes a tab: `browser_tabs select` only changes which
//!   tab the Mono's read tools use by default.
//! * App to extension frames stay under 1 MiB (Chrome's native messaging limit): requests carry
//!   only small fields. Replies come back over the socket (16 MiB frames), but a page reply that
//!   can be large (snapshot, read, screenshot) is still asked for with a `max_bytes` budget the
//!   extension must respect (`PAGE_BYTES`, `SNAPSHOT_BYTES`, `READ_BYTES`, `SCREENSHOT_BYTES`),
//!   to keep what reaches the agent and its context small. A larger answer is refused, not shown.
//! * Results carry no `structuredContent`: page data (titles, addresses, element names) must
//!   reach the agent only as text inside the gate's untrusted-content fence.
use super::super::link::protocol::{ErrorBody, ErrorCode, Method, TabInfo};
use super::super::links::manager::{LinkHandle, LinkManager, RequestError};
use super::guard::UNAVAILABLE_PREFIX;
use super::{tabs, Upstream};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

pub(crate) mod act;

/// The session id the gate sees: there is no real MCP session, the link is the session.
const SESSION_ID: &str = "zptah-extension";
/// Default and largest `max_bytes` asked of the extension for the text of a snapshot.
pub const SNAPSHOT_BYTES: u64 = 60_000;
/// Default `max_bytes` for `browser_read`.
pub const READ_BYTES: u64 = 40_000;
/// The largest `max_bytes` any page tool may ask for (the schema maximum). Replies travel over
/// the 16 MiB socket frame; this keeps one reply well inside it and inside the agent's context.
pub const PAGE_BYTES: u64 = 900_000;
/// `max_bytes` asked for a screenshot: the PNG the extension returns is at most this many bytes
/// (it scales the picture down to fit), so one picture stays small for the agent and the link.
pub const SCREENSHOT_BYTES: u64 = 700_000;
/// How long a request may take before the extension must answer `E_TIMEOUT`.
pub const DEFAULT_DEADLINE_MS: u64 = 30_000;
/// The longest a `browser_wait_for` may wait (spec 5.2).
pub const MAX_WAIT_MS: u64 = 600_000;
/// Extra time the link waits beyond the deadline it gave the extension, so the extension's own
/// `E_TIMEOUT` answer arrives before the app gives up.
const LINK_GRACE: Duration = Duration::from_secs(5);
/// Longest tab title shown in a list.
const MAX_TITLE_CHARS: usize = 200;

/// Where the live link of the Mono's profile comes from. `None`: the extension is not connected.
pub(crate) type LinkSource = Arc<dyn Fn() -> Option<LinkHandle> + Send + Sync>;

// ---- the Mono's origin rules ----

/// Which pages a Mono may read: the Mono's blocked and allowed origin lists (spec 6.5 rule 2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Origins {
    allowed: Vec<String>,
    blocked: Vec<String>,
}

/// Why a page is not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadRefusal {
    /// A browser page, a file or another address that is not http or https.
    Restricted,
    Blocked,
    /// The Mono has an allowed list and the page is not on it.
    NotAllowed,
}

impl Origins {
    /// Lists as saved in the Mono's browser setting. Entries that are not origins are dropped.
    pub fn new(allowed: &[String], blocked: &[String]) -> Self {
        let clean = |list: &[String]| {
            list.iter()
                .filter_map(|o| super::super::normalize_origin(o).ok())
                .collect()
        };
        Self {
            allowed: clean(allowed),
            blocked: clean(blocked),
        }
    }
    /// `Ok` when the page at `url` may be read.
    pub fn check(&self, url: &str) -> Result<(), ReadRefusal> {
        let Some(origin) = tabs::origin_of(url) else {
            // An empty page is harmless and a new tab starts as one; every other address that has
            // no web origin is a browser page.
            return if url == "about:blank" {
                Ok(())
            } else {
                Err(ReadRefusal::Restricted)
            };
        };
        if is_web_store(url) {
            return Err(ReadRefusal::Restricted);
        }
        if self.blocked.contains(&origin) {
            return Err(ReadRefusal::Blocked);
        }
        if !self.allowed.is_empty() && !self.allowed.contains(&origin) {
            return Err(ReadRefusal::NotAllowed);
        }
        Ok(())
    }
    /// Whether the page belongs in a tab list at all: a browser page is listed (without content),
    /// a page of an origin this Mono may not use is left out.
    fn listed(&self, url: &str) -> bool {
        !matches!(
            self.check(url),
            Err(ReadRefusal::Blocked | ReadRefusal::NotAllowed)
        )
    }
}

/// The extension stores are never read (spec 6.1).
fn is_web_store(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or("");
    let path = parsed.path();
    host == "chromewebstore.google.com"
        || (host == "chrome.google.com" && path.starts_with("/webstore"))
        || (host == "microsoftedge.microsoft.com" && path.starts_with("/addons"))
}

fn refusal_text(refusal: ReadRefusal) -> &'static str {
    match refusal {
        ReadRefusal::Restricted => "This is a browser page (or a file): zptah Browser never reads it.",
        ReadRefusal::Blocked => "This site is on this Mono's blocked list, so zptah Browser does not read it.",
        ReadRefusal::NotAllowed => "This Mono may only use the sites on its allowed list, and this page is not one of them.",
    }
}

// ---- the upstream ----

#[derive(Default)]
struct State {
    /// The tab list the agent saw last (what `browser_tabs select` indexes into).
    listed: Vec<TabInfo>,
    /// Every tab seen in a list, by id, for the address check before a read.
    known: HashMap<u64, TabInfo>,
    /// The tab the read tools use when the agent names none.
    selected: Option<u64>,
    /// Tabs the user stopped this Mono on with the in-page Stop button, with the title they had.
    /// Every call on such a tab fails until the user gives it back (spec 10a).
    stopped_tabs: HashMap<u64, String>,
}

pub struct ExtensionUpstream {
    mono_id: String,
    link: LinkSource,
    origins: Mutex<Origins>,
    /// Where `filename` arguments may write (the gate's private output folder); none: ignored.
    output_dir: Mutex<Option<PathBuf>>,
    state: Mutex<State>,
    /// What the act tools need beyond the read path (see [`act`]).
    act: act::ActShared,
}

impl ExtensionUpstream {
    pub(crate) fn new(mono_id: impl Into<String>, link: LinkSource) -> Arc<Self> {
        let mono_id = mono_id.into();
        let upstream = Arc::new(Self {
            act: act::ActShared::new(&mono_id),
            mono_id,
            link,
            origins: Mutex::new(Origins::default()),
            output_dir: Mutex::new(None),
            state: Mutex::new(State::default()),
        });
        act::register(&upstream);
        upstream
    }

    /// The link of the paired profile whose Chrome profile directory is `profile_dir`: found
    /// through `manager` for every request (the browser may connect after the Mono starts), and
    /// remembered while it stays connected.
    pub(crate) fn for_profile(
        mono_id: impl Into<String>,
        manager: LinkManager,
        profile_dir: &str,
    ) -> Arc<Self> {
        let profile_dir = profile_dir.to_owned();
        let cached: Mutex<Option<String>> = Mutex::new(None);
        Self::new(
            mono_id,
            Arc::new(move || {
                let mut cached = lock(&cached);
                if let Some(handle) = cached.as_deref().and_then(|id| manager.handle(id)) {
                    return Some(handle);
                }
                let found = manager
                    .links()
                    .ok()?
                    .into_iter()
                    .find(|l| l.connected && l.profile_dir == profile_dir)?;
                *cached = Some(found.instance_id.clone());
                manager.handle(&found.instance_id)
            }),
        )
    }

    /// The link of the paired profile `instance_id` in `manager`. The Mono wiring that calls it
    /// lands with the approval and trust work (spec tickets T13, T15).
    #[allow(dead_code)]
    pub(crate) fn for_manager(
        mono_id: impl Into<String>,
        manager: LinkManager,
        instance_id: impl Into<String>,
    ) -> Arc<Self> {
        let instance_id = instance_id.into();
        Self::new(mono_id, Arc::new(move || manager.handle(&instance_id)))
    }

    /// Applies the Mono's origin lists to every call that follows.
    pub fn set_origins(&self, origins: Origins) {
        *lock(&self.origins) = origins;
    }
    /// Lets `filename` arguments of snapshot and screenshot write into `dir`.
    pub fn set_output_dir(&self, dir: PathBuf) {
        *lock(&self.output_dir) = Some(dir);
    }

    fn origins(&self) -> Origins {
        lock(&self.origins).clone()
    }

    // ---- one request to the extension ----

    fn ask(&self, method: Method, params: Value, deadline_ms: u64) -> Result<Value, String> {
        self.ask_raw(method, params, deadline_ms)
            .map_err(|e| request_error_text(e, method))
    }

    /// [`Self::ask`] that keeps the error as the link gave it (the act path needs the code).
    fn ask_raw(
        &self,
        method: Method,
        mut params: Value,
        deadline_ms: u64,
    ) -> Result<Value, RequestError> {
        if let Some(tab) = params["tab"].as_u64() {
            if let Some(error) = self.stopped_tab_error(tab) {
                return Err(error);
            }
        }
        let Some(link) = (self.link)().filter(LinkHandle::is_open) else {
            return Err(RequestError::Closed);
        };
        if let Some(map) = params.as_object_mut() {
            map.insert("deadline_ms".into(), json!(deadline_ms));
        }
        let wait = Duration::from_millis(deadline_ms) + LINK_GRACE;
        link.request(method, Some(&self.mono_id), params, wait)
    }

    /// The error every call on a tab gets after the user pressed the in-page Stop there.
    fn stopped_tab_error(&self, tab: u64) -> Option<RequestError> {
        let title = self.state().stopped_tabs.get(&tab).cloned()?;
        Some(RequestError::Remote(ErrorBody {
            code: ErrorCode::NotOwner,
            message: format!(
                "The user stopped you on the tab \"{}\" and took it back. Do not use that tab unless the user asks again.",
                clean_line(&title)
            ),
            detail: Some(json!({"user_stopped": true})),
        }))
    }

    // ---- tab bookkeeping ----

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// Reads the tab list, keeps it and returns the part this Mono may see.
    fn refresh(&self, all: bool) -> Result<Vec<TabInfo>, String> {
        let result = self.ask(Method::TabsList, json!({"all": all}), DEFAULT_DEADLINE_MS)?;
        let tabs: Vec<TabInfo> = serde_json::from_value(result["tabs"].clone())
            .map_err(|_| "The extension sent a tab list that could not be read.".to_owned())?;
        if let Some(link) = (self.link)() {
            let now = crate::time::now_ms();
            super::super::ownership::with_table(link.instance_id(), |t| t.sync(&tabs, all, now));
        }
        let origins = self.origins();
        let visible: Vec<TabInfo> = tabs
            .into_iter()
            .filter(|tab| origins.listed(&tab.url))
            .collect();
        let mut state = self.state();
        if all {
            state.known.clear();
        }
        for tab in &visible {
            state.known.insert(tab.tab, tab.clone());
        }
        state.listed = visible.clone();
        Ok(visible)
    }

    /// The tab a read tool works on: the one named in `args`, else the selected one. Its address
    /// is checked against the Mono's origin lists.
    fn resolve_tab(&self, args: &Value) -> Result<TabInfo, String> {
        let id =
            match args.get("tab") {
                None | Some(Value::Null) => self.state().selected,
                Some(value) => Some(value.as_u64().ok_or(
                    "`tab` must be a tab id from browser_tabs list (the number after tab=).",
                )?),
            };
        let Some(id) = id else {
            return Err("No tab is selected. Call browser_tabs with action list (and all true to see the tabs the user has open), then browser_tabs with action select, or pass `tab`.".into());
        };
        let known = self.state().known.get(&id).cloned();
        let info = match known {
            Some(info) => info,
            None => {
                self.refresh(true)?;
                self.state().known.get(&id).cloned().ok_or_else(|| {
                    format!("No tab with id {id} is open (or this Mono may not use it). Call browser_tabs list with all true.")
                })?
            }
        };
        self.origins()
            .check(&info.url)
            .map_err(|r| refusal_text(r).to_owned())?;
        Ok(info)
    }

    /// Checks the address a reply came from: the page may have moved since the list.
    fn check_reply_url(&self, url: &str) -> Result<(), String> {
        self.origins().check(url).map_err(|r| {
            format!(
                "{} (the tab is now on a page this Mono may not read; the content was withheld.)",
                refusal_text(r)
            )
        })
    }

    // ---- tools ----

    fn call_tool(&self, name: &str, args: &Value) -> Value {
        let outcome = match name {
            "browser_tabs" => self.tabs_tool(args),
            "browser_snapshot" => self.snapshot_tool(args),
            "browser_read" => self.read_tool(args),
            "browser_find" => self.find_tool(args),
            "browser_take_screenshot" => self.screenshot_tool(args),
            "browser_wait_for" => self.wait_tool(args),
            name if act::is_act_tool(name) => self.act_tool(name, args),
            other => Err(format!(
                "{other} is not available with the zptah Browser extension yet."
            )),
        };
        match outcome {
            Ok(result) => result,
            Err(message) => error_result(message),
        }
    }

    fn tabs_tool(&self, args: &Value) -> Result<Value, String> {
        match args["action"].as_str().unwrap_or("list") {
            "list" => {
                let all = args["all"].as_bool().unwrap_or(false);
                let listed = self.refresh(all)?;
                Ok(self.list_result(&listed, all))
            }
            "select" => {
                let index = args["index"]
                    .as_u64()
                    .ok_or("browser_tabs select needs `index`: the number at the start of a line of the last list.")?;
                let tab = self
                    .state()
                    .listed
                    .get(index as usize)
                    .cloned()
                    .ok_or_else(|| format!("There is no tab {index} in the last list. Call browser_tabs list first."))?;
                self.state().selected = Some(tab.tab);
                let listed = self.state().listed.clone();
                Ok(self.list_result(&listed, true))
            }
            action @ ("new" | "close" | "adopt" | "release") => self.own_tabs_tool(action, args),
            other => Err(format!("Unknown browser_tabs action `{other}`.")),
        }
    }

    /// `### Open tabs` and one `- N: [(current) ][Title](url) [tab=ID] [...]` line per tab, the
    /// shape the gate's tab parser reads (a marker after the address starts with ` [`).
    fn list_result(&self, listed: &[TabInfo], all: bool) -> Value {
        let selected = self.state().selected;
        let mut text = String::from("### Open tabs\n");
        if listed.is_empty() {
            text.push_str(if all {
                "(no tabs)\n"
            } else {
                "(this Mono has no tabs yet; call browser_tabs list with all true to see the tabs the user has open)\n"
            });
        }
        for (index, tab) in listed.iter().enumerate() {
            let current = if Some(tab.tab) == selected {
                "(current) "
            } else {
                ""
            };
            let owner = match &tab.owner {
                None => "user",
                Some(owner) if *owner == self.mono_id => "yours",
                Some(_) => "another mono",
            };
            text.push_str(&format!(
                "- {index}: {current}[{}]({}) [tab={}] [{owner}]\n",
                clean_title(&tab.title),
                tab.url,
                tab.tab
            ));
        }
        text_result(text)
    }

    fn snapshot_tool(&self, args: &Value) -> Result<Value, String> {
        let tab = self.resolve_tab(args)?;
        let mut params = json!({
            "tab": tab.tab,
            "mode": match args["mode"].as_str() {
                None | Some("full") => "full",
                Some("interactive") => "interactive",
                Some(other) => return Err(format!("`mode` is full or interactive, not `{other}`.")),
            },
            "max_bytes": SNAPSHOT_BYTES,
        });
        if let Some(target) = non_empty(&args["target"]) {
            if !is_ref(target) {
                return Err(format!("`target` must be a ref from a snapshot, such as e12; this browser has no selectors (got `{target}`)."));
            }
            params["scope_ref"] = json!(target);
        }
        if let Some(depth) = args["depth"].as_u64() {
            params["depth"] = json!(depth.min(1000));
        }
        if let Some(from) = args["from"].as_u64() {
            params["from"] = json!(from);
            // A later page of the same snapshot: the extension refuses it when the page changed.
            if let Some(generation) = args["generation"].as_u64() {
                params["generation"] = json!(generation);
            }
        }
        let result = self.ask(Method::PageSnapshot, params, DEFAULT_DEADLINE_MS)?;
        let page = PageText::parse(&result, "text")?;
        self.check_reply_url(&page.url)?;
        self.note_read(&page.url, &page.body);
        let generation = result["generation"].as_u64().unwrap_or(0);
        let dropped = result["hidden_dropped"].as_u64().unwrap_or(0);

        let mut text = page.header();
        text.push_str("### Snapshot\n");
        match self.save(args, "yml", page.body.as_bytes()) {
            Some(name) => text.push_str(&format!("- [Snapshot]({name})\n")),
            None => text.push_str(&format!("```yaml\n{}\n```\n", fence_safe(&page.body))),
        }
        let mut notes = page.visibility_notes();
        if dropped > 0 {
            notes.push(format!(
                "{dropped} hidden, transparent or off-screen text items were left out of the snapshot."
            ));
        }
        if page.truncated {
            notes.push(page.more_hint("browser_snapshot", Some(generation)));
        }
        text.push_str(&notes_block(&notes));
        let out = text_result(text);
        Ok(out)
    }

    fn read_tool(&self, args: &Value) -> Result<Value, String> {
        let tab = self.resolve_tab(args)?;
        let mut params = json!({"tab": tab.tab, "max_bytes": READ_BYTES});
        if let Some(from) = args["from"].as_u64() {
            params["from"] = json!(from);
        }
        let result = self.ask(Method::PageRead, params, DEFAULT_DEADLINE_MS)?;
        let page = PageText::parse(&result, "markdown")?;
        self.check_reply_url(&page.url)?;
        self.note_read(&page.url, &page.body);
        let mut text = page.header();
        text.push_str("### Content\n");
        text.push_str(&page.body);
        text.push('\n');
        let mut notes = page.visibility_notes();
        if page.truncated {
            notes.push(page.more_hint("browser_read", None));
        }
        text.push_str(&notes_block(&notes));
        let out = text_result(text);
        Ok(out)
    }

    fn find_tool(&self, args: &Value) -> Result<Value, String> {
        let query = args["query"]
            .as_str()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or("browser_find needs `query`: the text or role to look for.")?;
        let tab = self.resolve_tab(args)?;
        let mut params =
            json!({"tab": tab.tab, "query": query.chars().take(1024).collect::<String>()});
        if let Some(max) = args["max"].as_u64() {
            params["max"] = json!(max.clamp(1, 1000));
        }
        let result = self.ask(Method::PageFind, params, DEFAULT_DEADLINE_MS)?;
        let url = str_field(&result, "url");
        let title = str_field(&result, "title");
        self.check_reply_url(&url)?;
        let matches = result["matches"]
            .as_array()
            .ok_or("The extension sent a find result that could not be read.")?;
        let mut text = page_header(&url, &title);
        text.push_str("### Matches\n");
        if matches.is_empty() {
            text.push_str("No match.\n");
        }
        for found in matches {
            let one = |key: &str| str_field(found, key);
            text.push_str(&format!(
                "- {} {} [ref={}]: {}\n",
                clean_line(&one("role")),
                json_string(&one("name")),
                clean_line(&one("ref")),
                clean_line(&one("context")),
            ));
        }
        let out = text_result(text);
        Ok(out)
    }

    fn screenshot_tool(&self, args: &Value) -> Result<Value, String> {
        let tab = self.resolve_tab(args)?;
        let mut params = json!({"tab": tab.tab, "max_bytes": SCREENSHOT_BYTES});
        if let Some(target) = non_empty(&args["target"]) {
            if !is_ref(target) {
                return Err(format!(
                    "`target` must be a ref from a snapshot, such as e12 (got `{target}`)."
                ));
            }
            params["ref"] = json!(target);
        }
        if let Some(full) = args["fullPage"].as_bool() {
            params["full_page"] = json!(full);
        }
        let result = self.ask(Method::PageScreenshot, params, DEFAULT_DEADLINE_MS)?;
        let b64 = result["png_b64"].as_str().unwrap_or("");
        // Cheap bound before decoding: the extension was told the budget.
        if b64.len() as u64 > SCREENSHOT_BYTES.div_ceil(3) * 4 + 4 {
            return Err("The extension sent a screenshot larger than the size it was asked for; it was dropped.".into());
        }
        let png = B64
            .decode(b64)
            .map_err(|_| "The extension sent a screenshot that is not valid base64.".to_owned())?;
        if png.len() as u64 > SCREENSHOT_BYTES {
            return Err("The extension sent a screenshot larger than the size it was asked for; it was dropped.".into());
        }
        if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(
                "The extension sent a screenshot that is not a PNG; it was dropped.".into(),
            );
        }
        let url = result["url"]
            .as_str()
            .map(str::to_owned)
            .ok_or("The extension sent a screenshot without the page address; it was dropped.")?;
        let title = result["title"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or(tab.title.clone());
        self.check_reply_url(&url)?;
        let (width, height) = (
            result["width"].as_u64().unwrap_or(0),
            result["height"].as_u64().unwrap_or(0),
        );
        let method = result["method"].as_str().unwrap_or("visible");
        let scale = result["scale_pct"].as_u64().filter(|s| *s < 100);
        let mut text = page_header(&url, &title);
        text.push_str("### Screenshot\n");
        text.push_str(&format!(
            "- {width}x{height} PNG of tab {} ({method} capture{})\n",
            tab.tab,
            scale
                .map(|s| format!(", scaled to {s}%"))
                .unwrap_or_default(),
        ));
        if let Some(name) = self.save(args, "png", &png) {
            text.push_str(&format!("- [Screenshot]({name})\n"));
        }
        let mut out = text_result(text);
        out["content"]
            .as_array_mut()
            .expect("content is an array")
            .push(json!({"type": "image", "data": b64, "mimeType": "image/png"}));
        Ok(out)
    }

    fn wait_tool(&self, args: &Value) -> Result<Value, String> {
        let tab = self.resolve_tab(args)?;
        let mut params = json!({"tab": tab.tab});
        let mut conditions = 0;
        for (arg, field) in [("text", "text"), ("textGone", "text_gone")] {
            if let Some(text) = non_empty(&args[arg]) {
                params[field] = json!(text.chars().take(4096).collect::<String>());
                conditions += 1;
            }
        }
        let mut waits = 0;
        if let Some(seconds) = args["time"].as_f64().filter(|s| s.is_finite() && *s >= 0.0) {
            let ms = (seconds * 1000.0).round() as u64;
            if ms > MAX_WAIT_MS {
                return Err(format!("`time` is at most {} seconds.", MAX_WAIT_MS / 1000));
            }
            params["time_ms"] = json!(ms);
            waits = ms;
            conditions += 1;
        }
        if conditions == 0 {
            return Err("browser_wait_for needs `text`, `textGone` or `time`.".into());
        }
        // A fixed wait is part of the request: the deadline must outlast it.
        let deadline = (waits + DEFAULT_DEADLINE_MS).min(MAX_WAIT_MS);
        let result = self.ask(Method::PageWait, params, deadline)?;
        let url = str_field(&result, "url");
        let title = result["title"].as_str().unwrap_or("").to_owned();
        self.check_reply_url(&url)?;
        let met = result["met"].as_bool().unwrap_or(false);
        let mut text = page_header(&url, &title);
        text.push_str(if met {
            "### Result\nThe condition was met.\n"
        } else {
            "### Result\nTimed out: the condition was not met.\n"
        });
        let mut out = text_result(text);
        // An unmet condition is a tool error on purpose (as in Playwright): the agent must not
        // read "no error" as "the page is ready".
        out["isError"] = json!(!met);
        Ok(out)
    }

    /// Writes `bytes` to the file `args.filename` names, when that is a plain file inside the
    /// output folder (the gate already forced it there). Returns the file name for the link.
    fn save(&self, args: &Value, extension: &str, bytes: &[u8]) -> Option<String> {
        let dir = lock(&self.output_dir).clone()?;
        let path = Path::new(args["filename"].as_str()?);
        let name = path.file_name()?.to_str()?.to_owned();
        if path.parent()? != dir || !name.ends_with(&format!(".{extension}")) {
            return None;
        }
        std::fs::create_dir_all(&dir).ok()?;
        // Never write through a link, and never over an existing file.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(path).ok()?;
        std::io::Write::write_all(&mut file, bytes).ok()?;
        Some(name)
    }
}

impl Upstream for ExtensionUpstream {
    fn post(
        &self,
        _url: &str,
        _session: Option<&str>,
        message: &Value,
    ) -> Result<(Option<Value>, Option<String>), String> {
        let Some(id) = message.get("id").filter(|id| !id.is_null()) else {
            // A notification (`notifications/initialized`): nothing to answer.
            return Ok((None, None));
        };
        let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
        match message["method"].as_str().unwrap_or("") {
            "initialize" => Ok((
                Some(ok(json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "zptah-browser-extension", "version": env!("CARGO_PKG_VERSION")},
                }))),
                Some(SESSION_ID.to_owned()),
            )),
            "ping" => Ok((Some(ok(json!({}))), None)),
            "tools/list" => Ok((Some(ok(json!({"tools": tool_definitions()}))), None)),
            "tools/call" => {
                let params = &message["params"];
                let name = params["name"].as_str().unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                Ok((Some(ok(self.call_tool(name, &args))), None))
            }
            _ => Ok((
                Some(
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}}),
                ),
                None,
            )),
        }
    }

    fn session_survives(&self, _error: &str) -> bool {
        // There is no server session to lose: the link comes and goes by itself.
        true
    }

    fn run_started(&self, session: i64, user_message: &str) {
        self.begin_run(session, user_message);
    }

    fn run_ended(&self, session: i64) {
        self.end_run(session);
    }

    fn injection_found(&self, session: i64) {
        self.flag_injection(session);
    }
}

// ---- a page reply ----

/// The common part of a snapshot or read reply.
struct PageText {
    url: String,
    title: String,
    body: String,
    truncated: bool,
    next_from: Option<u64>,
    total_bytes: Option<u64>,
    hidden: bool,
    unrendered: bool,
}
impl PageText {
    fn parse(result: &Value, body_field: &str) -> Result<Self, String> {
        let bad = || "The extension sent a page result that could not be read.".to_owned();
        let body = result[body_field].as_str().ok_or_else(bad)?.to_owned();
        if body.len() as u64 > PAGE_BYTES {
            return Err(
                "The extension sent more text than it was asked for; it was dropped.".into(),
            );
        }
        Ok(Self {
            url: result["url"].as_str().ok_or_else(bad)?.to_owned(),
            title: str_field(result, "title"),
            body,
            truncated: result["truncated"].as_bool().unwrap_or(false),
            next_from: result["next_from"].as_u64(),
            total_bytes: result["total_bytes"].as_u64(),
            hidden: result["visibility"] == "hidden",
            unrendered: result["unrendered"].as_bool().unwrap_or(false),
        })
    }
    fn header(&self) -> String {
        page_header(&self.url, &self.title)
    }
    /// The F7 hint: a hidden tab can render little or nothing (spec 6.4, "Background rendering").
    fn visibility_notes(&self) -> Vec<String> {
        if self.hidden && self.unrendered {
            vec!["This tab is in the background and the page looks unrendered: sites often draw little or nothing until the tab is shown, so the page is probably not empty. Ask the user to show the tab, then read it again.".to_owned()]
        } else if self.hidden {
            vec!["This tab is in the background.".to_owned()]
        } else {
            Vec::new()
        }
    }
    fn more_hint(&self, tool: &str, generation: Option<u64>) -> String {
        let total = self
            .total_bytes
            .map(|t| format!(" of {t} bytes"))
            .unwrap_or_default();
        let from = self.next_from.unwrap_or(self.body.len() as u64);
        let generation = generation
            .map(|g| format!(" and generation={g}"))
            .unwrap_or_default();
        format!("The result is cut after {} bytes{total}. Call {tool} again with from={from}{generation} to read the rest.", self.body.len())
    }
}

fn page_header(url: &str, title: &str) -> String {
    format!(
        "### Page\n- Page URL: {}\n- Page Title: {}\n",
        clean_line(url),
        clean_line(title)
    )
}

fn notes_block(notes: &[String]) -> String {
    if notes.is_empty() {
        return String::new();
    }
    let mut out = String::from("### Notes\n");
    for note in notes {
        out.push_str(&format!("- {note}\n"));
    }
    out
}

// ---- small helpers ----

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn text_result(text: String) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": false})
}

fn error_result(message: String) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}

fn not_connected() -> String {
    format!("{UNAVAILABLE_PREFIX} the zptah Browser extension is not connected. Ask the user to open Chrome with the zptah Browser extension and connect it in zptah.")
}

fn request_error_text(error: RequestError, method: Method) -> String {
    match error {
        RequestError::Closed => not_connected(),
        RequestError::Timeout => format!(
            "The zptah Browser extension did not answer {} in time.",
            method.as_str()
        ),
        RequestError::TooLarge => "The request was too large for the browser link.".to_owned(),
        RequestError::Invalid(e) => format!("The browser request was refused: {e}."),
        RequestError::Remote(body) => remote_text(&body),
    }
}

/// What an error code of the extension means for the agent.
fn remote_text(body: &ErrorBody) -> String {
    // The gate's own words (the user's Stop on a tab) need no hint around them.
    if body.detail.as_ref().is_some_and(|d| d["user_stopped"] == true) {
        return body.message.clone();
    }
    let hint = match body.code {
        ErrorCode::TabGone => "That tab is closed. Call browser_tabs list again.",
        ErrorCode::StaleRef => "That element reference is out of date. Take a new browser_snapshot.",
        ErrorCode::RestrictedUrl => "That is a browser page; zptah Browser never reads it.",
        ErrorCode::OriginBlocked => "That site is not allowed for this Mono.",
        ErrorCode::Stopped => "The user stopped the browser with the Stop button. Do not try to continue; tell the user.",
        ErrorCode::UnknownMono => "The browser does not know this Mono yet. Tell the user to reconnect the browser.",
        ErrorCode::TooLarge => "The result was too large to send.",
        ErrorCode::Timeout => "The page did not answer in time.",
        ErrorCode::Unsupported => "That is not possible here.",
        ErrorCode::Incognito => "Private windows are never used.",
        ErrorCode::NotOwner => "That tab is not yours. Act only in tabs of your own (browser_tabs list shows them); the user's tabs are read only.",
        ErrorCode::StalePrepare => "The page changed before the action ran, so nothing was done. Take a new browser_snapshot and try again.",
        ErrorCode::PasswordField => "Typing into a password field is never allowed. Stop and tell the user to sign in themselves.",
        ErrorCode::NeedsApproval => "The browser wants the user's approval for this action. Nothing was done.",
        ErrorCode::DebuggerDenied => "This action needs the browser's debugging permission, which the user has not given. Tell the user to allow it in the zptah Browser options page.",
        ErrorCode::DebuggerBusy => "Another tool (such as DevTools) is attached to that tab. Tell the user to close it.",
        ErrorCode::DialogSuspected => "The page may be showing a dialog (an alert or a confirm) that the browser cannot see. Tell the user to answer it in the tab; do not repeat the action.",
        ErrorCode::Busy => "The browser is busy. Try again in a moment.",
        _ => "",
    };
    let message = clean_line(&body.message);
    if hint.is_empty() {
        format!("The browser said {}: {message}", body.code.as_str())
    } else {
        format!("{hint} ({}: {message})", body.code.as_str())
    }
}

fn non_empty(value: &Value) -> Option<&str> {
    value.as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn str_field(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("").to_owned()
}

/// A ref the snapshot prints: `e12`, or `f3e12` inside a frame.
fn is_ref(text: &str) -> bool {
    let rest = text
        .strip_prefix('f')
        .map_or(text, |r| r.trim_start_matches(|c: char| c.is_ascii_digit()));
    rest.strip_prefix('e')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        && text.len() <= 64
}

/// One line of text: control characters (and so line breaks) become spaces.
fn clean_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A title for a tab line: brackets and parentheses would end the `[Title](url)` early.
fn clean_title(title: &str) -> String {
    let line = clean_line(title);
    line.chars()
        .map(|c| match c {
            '[' => '(',
            ']' => ')',
            c => c,
        })
        .take(MAX_TITLE_CHARS)
        .collect()
}

fn json_string(text: &str) -> String {
    serde_json::to_string(&clean_line(text)).unwrap_or_else(|_| "\"\"".into())
}

/// Page text must not close the fence of the yaml block it sits in.
fn fence_safe(text: &str) -> String {
    text.replace("```", "``\u{200b}`")
}

// ---- tool definitions ----

fn tab_property() -> Value {
    json!({
        "type": "integer",
        "description": "The tab to use: the number after tab= in a browser_tabs list line. Default: the tab chosen with browser_tabs select.",
    })
}

fn ref_property() -> Value {
    json!({"type": "string", "description": "The [ref=eN] of the element from a browser_snapshot of the same tab."})
}

/// The properties every action tool takes, besides its own.
fn act_props(own: Value) -> Value {
    let mut props = json!({
        "tab": tab_property(),
        "purpose": {"type": "string", "description": "One sentence: why you do this. The user sees it, quoted as your words; it never lowers how carefully the action is checked."},
        "declared_risk": {"type": "array", "items": {"type": "string", "enum": ["payment", "destructive", "account_security", "sharing", "send_unnamed", "post_public", "upload", "credential_entry", "terms"]}, "description": "Risks you know this action has. They can only add questions for the user."},
    });
    if let (Some(map), Some(own)) = (props.as_object_mut(), own.as_object()) {
        map.extend(own.clone());
    }
    props
}

fn tool_definitions() -> Vec<Value> {
    let object = |properties: Value, required: Value| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    vec![
        json!({
            "name": "browser_tabs",
            "title": "List, open, close, select or adopt browser tabs",
            "description": "List the tabs, choose the tab the other browser tools use, open a tab of your own, close or release one of your own, or adopt a tab of the user's. `list` shows this Mono's own tabs; with all true it shows every open tab of the user's browser profile that this Mono may use (the user's tabs are read only until adopted). `select` takes the index from the last list; it never brings a tab to the front. `new` opens a tab in your group without showing it. `close` closes one of your tabs; a tab of the user's is never closed. `release` gives a tab of yours back to the user. `adopt` takes a tab of the user's into your group, but only with `user_reference`: the words the user wrote that name the tab. If the user did not name the tab, ask them to name it or to drag it into your group.",
            "inputSchema": object(json!({
                "action": {"type": "string", "enum": ["list", "select", "new", "close", "release", "adopt"], "description": "list, select, new, close, release or adopt"},
                "all": {"type": "boolean", "description": "With list: show all open tabs of the profile, not only this Mono's tabs."},
                "index": {"type": "integer", "description": "With select, close, release or adopt: the number at the start of a line of the last list."},
                "tab": {"type": "integer", "description": "With close, release or adopt: the tab id (the number after tab=)."},
                "url": {"type": "string", "description": "With new: the address to open (http or https)."},
                "user_reference": {"type": "string", "description": "With adopt: words the user wrote in this task that name the tab, copied exactly (at least 2 words or 8 characters)."},
            }), json!(["action"])),
            "annotations": {"readOnlyHint": false},
        }),
        json!({
            "name": "browser_snapshot",
            "title": "Page snapshot",
            "description": "Capture the accessibility snapshot of a tab: roles, names and a [ref=eN] for each element. Better than a screenshot for reading and for later actions. A long page is cut; call again with from and generation to read the rest.",
            "inputSchema": object(json!({
                "tab": tab_property(),
                "target": {"type": "string", "description": "A ref such as e12: snapshot only that element."},
                "depth": {"type": "integer", "description": "How many levels below the target to show."},
                "mode": {"type": "string", "enum": ["full", "interactive"], "description": "interactive shows only controls and links."},
                "from": {"type": "integer", "description": "Continue a cut snapshot at this byte offset."},
                "generation": {"type": "integer", "description": "With from: the generation the cut snapshot reported."},
                "filename": {"type": "string", "description": "Save the snapshot to this file instead of returning it."},
            }), json!([])),
            "annotations": {"readOnlyHint": true},
        }),
        json!({
            "name": "browser_read",
            "title": "Read the page text",
            "description": "Read the main text of a tab as Markdown. Cheaper than a snapshot for articles and documents. A long page is cut; call again with from to read the rest.",
            "inputSchema": object(json!({
                "tab": tab_property(),
                "from": {"type": "integer", "description": "Continue a cut result at this byte offset."},
            }), json!([])),
            "annotations": {"readOnlyHint": true},
        }),
        json!({
            "name": "browser_find",
            "title": "Find on the page",
            "description": "Search a tab for text and return matching elements with their refs. Start the query with a role word (button, link, textbox, checkbox, radio, combobox, heading, tab, menuitem, ...) to match only that role: `button Buy` finds buttons whose name holds \"buy\"; a role word alone lists every element of that role.",
            "inputSchema": object(json!({
                "query": {"type": "string", "description": "Text or role to look for."},
                "tab": tab_property(),
                "max": {"type": "integer", "description": "At most this many matches."},
            }), json!(["query"])),
            "annotations": {"readOnlyHint": true},
        }),
        json!({
            "name": "browser_take_screenshot",
            "title": "Take a screenshot",
            "description": "Take a PNG screenshot of the visible part of a tab, or of one element. Only works for a tab that is showing; for a background tab use browser_snapshot, or ask the user to show the tab. Large pictures are scaled down.",
            "inputSchema": object(json!({
                "tab": tab_property(),
                "target": {"type": "string", "description": "A ref such as e12: capture only that element."},
                "fullPage": {"type": "boolean", "description": "Capture the whole page, not only the visible part."},
                "filename": {"type": "string", "description": "Also save the picture to this file."},
            }), json!([])),
            "annotations": {"readOnlyHint": true},
        }),
        json!({
            "name": "browser_click",
            "title": "Click",
            "description": "Click an element of a tab you own, by its ref from a browser_snapshot. The user is asked first when the click looks risky (payments, deleting, sharing, sending to someone they did not mention, and the like); a refusal is final.",
            "inputSchema": object(act_props(json!({
                "ref": ref_property(),
                "button": {"type": "string", "enum": ["left", "right", "middle"]},
                "doubleClick": {"type": "boolean"},
                "modifiers": {"type": "array", "items": {"type": "string"}, "description": "Keys held during the click, such as Shift or Control."},
            })), json!(["ref"])),
            "annotations": {"destructiveHint": true},
        }),
        json!({
            "name": "browser_type",
            "title": "Type text",
            "description": "Replace the value of a text field with `text`. Passwords are never typed: when a page needs one, stop and ask the user to sign in. One-time codes, card and ID numbers always ask the user.",
            "inputSchema": object(act_props(json!({
                "ref": ref_property(),
                "text": {"type": "string", "description": "The text to put in the field."},
                "submit": {"type": "boolean", "description": "Submit the form afterwards."},
                "slowly": {"type": "boolean", "description": "Type key by key (for fields that react to each key)."},
            })), json!(["ref", "text"])),
            "annotations": {"destructiveHint": true},
        }),
        json!({
            "name": "browser_fill_form",
            "title": "Fill several fields",
            "description": "Fill several fields of one form. Each field is judged on its own: passwords are refused, sensitive fields ask the user.",
            "inputSchema": object(act_props(json!({
                "fields": {"type": "array", "description": "The fields to fill.", "items": {"type": "object", "properties": {
                    "ref": ref_property(),
                    "type": {"type": "string", "enum": ["textbox", "checkbox", "radio", "combobox", "slider"]},
                    "value": {"description": "The value: text, or true/false for a checkbox."},
                }, "required": ["ref", "type", "value"]}},
            })), json!(["fields"])),
            "annotations": {"destructiveHint": true},
        }),
        json!({
            "name": "browser_select_option",
            "title": "Select an option",
            "description": "Select option(s) of a dropdown by their text or value.",
            "inputSchema": object(act_props(json!({
                "ref": ref_property(),
                "values": {"type": "array", "items": {"type": "string"}, "description": "The options to select."},
            })), json!(["ref", "values"])),
            "annotations": {"destructiveHint": true},
        }),
        json!({
            "name": "browser_press_key",
            "title": "Press a key",
            "description": "Press a key (Enter, Tab, Escape, ArrowDown, PageDown, a letter ...). PageDown, PageUp, Home, End and the arrows scroll. Enter can submit a form, so it is judged like a click on the submit button.",
            "inputSchema": object(act_props(json!({
                "key": {"type": "string", "description": "The key name."},
                "ref": ref_property(),
            })), json!(["key"])),
            "annotations": {"destructiveHint": true},
        }),
        json!({
            "name": "browser_hover",
            "title": "Hover",
            "description": "Point at an element (to open a menu). Never changes anything and never asks.",
            "inputSchema": object(act_props(json!({"ref": ref_property()})), json!(["ref"])),
            "annotations": {"readOnlyHint": false},
        }),
        json!({
            "name": "browser_drag",
            "title": "Drag and drop",
            "description": "Drag one element onto another. Needs the browser's debugging permission, which the user can give in the zptah Browser options page.",
            "inputSchema": object(act_props(json!({
                "startRef": ref_property(),
                "endRef": ref_property(),
            })), json!(["startRef", "endRef"])),
            "annotations": {"destructiveHint": true},
        }),
        json!({
            "name": "browser_mouse_wheel",
            "title": "Scroll",
            "description": "Scroll the page of a tab you own by deltaX and deltaY pixels (positive deltaY scrolls down).",
            "inputSchema": object(act_props(json!({
                "deltaX": {"type": "number"},
                "deltaY": {"type": "number"},
            })), json!([])),
            "annotations": {"readOnlyHint": false},
        }),
        json!({
            "name": "browser_navigate",
            "title": "Open an address",
            "description": "Open an http or https address in a tab you own. A site the user did not mention, or a bank or other sensitive site, asks the user first.",
            "inputSchema": object(act_props(json!({"url": {"type": "string", "description": "The address to open."}})), json!(["url"])),
        }),
        json!({
            "name": "browser_navigate_back",
            "title": "Go back",
            "description": "Go back one page in a tab you own.",
            "inputSchema": object(act_props(json!({})), json!([])),
        }),
        json!({
            "name": "browser_navigate_forward",
            "title": "Go forward",
            "description": "Go forward one page in a tab you own.",
            "inputSchema": object(act_props(json!({})), json!([])),
        }),
        json!({
            "name": "browser_handle_dialog",
            "title": "Answer a dialog",
            "description": "Accept or dismiss a JavaScript dialog (alert, confirm, prompt) of a tab you own. Needs the browser's debugging permission.",
            "inputSchema": object(act_props(json!({
                "accept": {"type": "boolean"},
                "promptText": {"type": "string", "description": "The text for a prompt dialog."},
            })), json!(["accept"])),
            "annotations": {"destructiveHint": true},
        }),
        json!({
            "name": "browser_file_upload",
            "title": "Upload files",
            "description": "Put files from the user's computer into a file input of a tab you own. The user is always asked first and sees the file names. Credential files are never uploaded.",
            "inputSchema": object(act_props(json!({
                "ref": ref_property(),
                "paths": {"type": "array", "items": {"type": "string"}, "description": "Absolute paths of the files (at most 20, 32 MiB in all)."},
            })), json!(["ref", "paths"])),
            "annotations": {"destructiveHint": true},
        }),
        json!({
            "name": "browser_wait_for",
            "title": "Wait",
            "description": "Wait for text to appear or disappear in a tab, or for some seconds.",
            "inputSchema": object(json!({
                "tab": tab_property(),
                "text": {"type": "string", "description": "Wait until this text appears."},
                "textGone": {"type": "string", "description": "Wait until this text disappears."},
                "time": {"type": "number", "description": "Wait this many seconds."},
            }), json!([])),
            "annotations": {"readOnlyHint": true},
        }),
    ]
}
