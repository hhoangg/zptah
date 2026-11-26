//! A web device's taps on a visual in the conversation (`visual.dispatch`), run in that device's own copy of the
//! program.
//!
//! The web draws a visual from the tree the desktop rendered, so a tab or a slider there must come back here to run
//! its handler. Each `(device, conversation, turn, item)` gets its own [`VisualRuntime`], apart from the one the
//! desktop's chat shows: a remote tap changes nothing on the desktop and nothing another device sees. The runtime has
//! no filesystem, network or shell, and every call stops after 60 ms. Each runtime holds at most 16 MiB, so the cache
//! (4 runtimes per device, 16 in all, the least recently used goes first) holds at most 16 x 16 MiB = 256 MiB.
//!
//! This runs on the app's foreground thread, so what a device may take of it is limited three ways: a count (a burst
//! of [`COUNT_BURST`], then [`COUNT_PER_SECOND`] a second), and the foreground time its dispatches really took (a
//! budget of [`DEVICE_MS_PER_SECOND`] ms a second for one device and [`TOTAL_MS_PER_SECOND`] ms for all devices
//! together, charged from a measured clock, so a slow program or a rebuild costs more than a quick tap). A handler
//! that is not the current one never makes a runtime build over and over: see [`Remote::dispatch`].
use monocode_core::visual::{STALE_CONTROL, VisualRuntime};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

/// Runtimes kept for one device, and for all devices together.
pub(crate) const MAX_PER_DEVICE: usize = 4;
pub(crate) const MAX_TOTAL: usize = 16;
/// A device may dispatch this many at once, then [`COUNT_PER_SECOND`] a second.
const COUNT_BURST: f64 = 5.0;
const COUNT_PER_SECOND: f64 = 10.0;
/// Foreground milliseconds a device may use: a bucket of [`DEVICE_MS_CAP`] that refills this much a second.
const DEVICE_MS_PER_SECOND: f64 = 150.0;
const DEVICE_MS_CAP: f64 = 300.0;
/// The same for every device together.
const TOTAL_MS_PER_SECOND: f64 = 300.0;
const TOTAL_MS_CAP: f64 = 600.0;
/// Devices whose allowance is remembered; this only bounds a flood of device ids.
const MAX_BUCKETS: usize = 256;

/// The longest text a control may send (`validate_event` in core refuses more; this bounds the JSON first).
pub(crate) const MAX_VALUE_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Key {
    pub device: String,
    pub conversation: String,
    pub turn: String,
    pub item: String,
}

struct Entry {
    key: Key,
    /// The visual document's id, as the transcript item names it.
    document: String,
    source: String,
    runtime: VisualRuntime,
    /// The first render of this runtime, as the web's snapshot has it.
    first: Value,
    /// A dispatch has worked on this runtime: its handlers are no longer the first render's.
    moved: bool,
}

/// What a dispatch answers.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// The tree after the event. `reset` is true when the handler is not one of the first render either (the device
    /// holds a tree this desktop never made): `tree` is then the first render, which the device shows instead.
    Tree {
        tree: Value,
        reset: bool,
    },
    Failed(String),
}

struct Bucket {
    taps: f64,
    ms: f64,
    at: Instant,
}

pub(crate) struct Remote {
    /// Least recently used first.
    entries: VecDeque<Entry>,
    buckets: HashMap<String, Bucket>,
    /// Foreground milliseconds left for all devices, and when that was last brought up to date.
    total: (f64, Instant),
    clock: Box<dyn Fn() -> Instant>,
    /// Runtimes made, for tests.
    #[cfg(test)]
    builds: usize,
}

impl Default for Remote {
    fn default() -> Self {
        Self::with_clock(Box::new(Instant::now))
    }
}

impl Remote {
    /// A cache that reads the time from `clock`, which a test controls.
    pub(crate) fn with_clock(clock: Box<dyn Fn() -> Instant>) -> Self {
        let now = clock();
        Self {
            entries: VecDeque::new(),
            buckets: HashMap::new(),
            total: (TOTAL_MS_CAP, now),
            clock,
            #[cfg(test)]
            builds: 0,
        }
    }

    /// Whether the device may dispatch now: it has a tap left and foreground time left, and so has everyone together.
    /// Takes the tap.
    pub(crate) fn allow(&mut self, device: &str) -> bool {
        let now = (self.clock)();
        let (left, at) = &mut self.total;
        *left = (*left + now.saturating_duration_since(*at).as_secs_f64() * TOTAL_MS_PER_SECOND)
            .min(TOTAL_MS_CAP);
        *at = now;
        let total_left = *left;
        if self.buckets.len() >= MAX_BUCKETS && !self.buckets.contains_key(device) {
            // A device that is not limited needs no memory: the idle ones go, then everything if all are busy.
            self.buckets.retain(|_, b| {
                now.saturating_duration_since(b.at) < Duration::from_secs(2)
                    && (b.taps < COUNT_BURST || b.ms < DEVICE_MS_CAP)
            });
            if self.buckets.len() >= MAX_BUCKETS {
                self.buckets.clear();
            }
        }
        let b = self.buckets.entry(device.to_owned()).or_insert(Bucket {
            taps: COUNT_BURST,
            ms: DEVICE_MS_CAP,
            at: now,
        });
        let elapsed = now.saturating_duration_since(b.at).as_secs_f64();
        b.taps = (b.taps + elapsed * COUNT_PER_SECOND).min(COUNT_BURST);
        b.ms = (b.ms + elapsed * DEVICE_MS_PER_SECOND).min(DEVICE_MS_CAP);
        b.at = now;
        if b.taps < 1.0 || b.ms <= 0.0 || total_left <= 0.0 {
            return false;
        }
        b.taps -= 1.0;
        true
    }

    /// Charges foreground time a dispatch of `device` took. The balance may go below zero (a single call is cut
    /// off at 60 ms, a rebuild takes longer), by at most one bucket, and the device waits it off.
    pub(crate) fn charge(&mut self, device: &str, cost: Duration) {
        let ms = cost.as_secs_f64() * 1000.0;
        if let Some(b) = self.buckets.get_mut(device) {
            b.ms = (b.ms - ms).max(-DEVICE_MS_CAP);
        }
        self.total.0 = (self.total.0 - ms).max(-TOTAL_MS_CAP);
    }

    /// The source of the runtime kept for `key`, when it is the one of document `document`: what a conversation that
    /// is not open (its rows do not change) answers with, so a tap does not read the whole session again.
    pub(crate) fn cached_source(&self, key: &Key, document: &str) -> Option<(String, String)> {
        self.entries
            .iter()
            .find(|e| e.key == *key && e.document == document)
            .map(|e| (e.document.clone(), e.source.clone()))
    }

    /// Runs `handler` with `value` in the runtime of `key`, made from `source` when there is none (or it was made
    /// from another source). The time it takes is charged to the device.
    ///
    /// A handler that the runtime does not know is stale. The first render's handlers are always `1:*`, so a device
    /// that lost its view (reopened chat, another tab) and sends `1:0` to a runtime that has moved on gets that
    /// tap applied on a fresh runtime. A handler that is not the fresh runtime's either answers its first render with
    /// `reset`. A runtime nothing has worked on yet is never rebuilt for a stale handler: its stored first render is
    /// the answer.
    pub(crate) fn dispatch(
        &mut self,
        key: Key,
        document: &str,
        source: &str,
        handler: &str,
        value: Value,
    ) -> Outcome {
        let device = key.device.clone();
        let started = (self.clock)();
        let outcome = self.run(key, document, source, handler, value);
        let cost = (self.clock)().saturating_duration_since(started);
        self.charge(&device, cost);
        outcome
    }

    fn run(
        &mut self,
        key: Key,
        document: &str,
        source: &str,
        handler: &str,
        value: Value,
    ) -> Outcome {
        let cached = self
            .entries
            .iter()
            .position(|e| e.key == key && e.source == source && e.document == document)
            .and_then(|at| self.entries.remove(at));
        let mut entry = match cached {
            Some(entry) => entry,
            None => {
                self.entries.retain(|e| e.key != key);
                match self.build(key, document, source) {
                    Ok(entry) => entry,
                    Err(error) => return Outcome::Failed(error),
                }
            }
        };
        match entry.runtime.dispatch(handler, value.clone()) {
            Ok(node) => match serde_json::to_value(node) {
                Ok(tree) => {
                    entry.moved = true;
                    self.keep(entry);
                    Outcome::Tree { tree, reset: false }
                }
                Err(error) => Outcome::Failed(error.to_string()),
            },
            Err(error) if error == STALE_CONTROL && !entry.moved => {
                let tree = entry.first.clone();
                self.keep(entry);
                Outcome::Tree { tree, reset: true }
            }
            Err(error) if error == STALE_CONTROL => {
                // The device's tree is from before this runtime moved on: try the tap on a fresh one.
                let mut fresh = match self.build(entry.key, document, source) {
                    Ok(fresh) => fresh,
                    Err(error) => return Outcome::Failed(error),
                };
                match fresh.runtime.dispatch(handler, value) {
                    Ok(node) => match serde_json::to_value(node) {
                        Ok(tree) => {
                            fresh.moved = true;
                            self.keep(fresh);
                            Outcome::Tree { tree, reset: false }
                        }
                        Err(error) => Outcome::Failed(error.to_string()),
                    },
                    Err(error) if error == STALE_CONTROL => {
                        let tree = fresh.first.clone();
                        self.keep(fresh);
                        Outcome::Tree { tree, reset: true }
                    }
                    Err(error) => {
                        if !fresh.runtime.is_stopped() {
                            self.keep(fresh);
                        }
                        Outcome::Failed(error)
                    }
                }
            }
            Err(error) => {
                // A refused value leaves the runtime as it was; a script that failed has stopped for good.
                if !entry.runtime.is_stopped() {
                    self.keep(entry);
                }
                Outcome::Failed(error)
            }
        }
    }

    /// A runtime of `source` after its first render.
    fn build(&mut self, key: Key, document: &str, source: &str) -> Result<Entry, String> {
        #[cfg(test)]
        {
            self.builds += 1;
        }
        let mut runtime = VisualRuntime::new(source)?;
        let first = serde_json::to_value(runtime.render()?).map_err(|e| e.to_string())?;
        Ok(Entry {
            key,
            document: document.to_owned(),
            source: source.to_owned(),
            runtime,
            first,
            moved: false,
        })
    }

    /// Stores `entry` as the most recently used, evicting the least recently used of its device and of all.
    fn keep(&mut self, entry: Entry) {
        let device = entry.key.device.clone();
        while self.count(&device) >= MAX_PER_DEVICE {
            let Some(at) = self.entries.iter().position(|e| e.key.device == device) else {
                break;
            };
            self.entries.remove(at);
        }
        while self.entries.len() >= MAX_TOTAL {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    pub(crate) fn count(&self, device: &str) -> usize {
        self.entries
            .iter()
            .filter(|e| e.key.device == device)
            .count()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Forgets a device's runtimes.
    pub(crate) fn forget(&mut self, device: &str) {
        self.entries.retain(|entry| entry.key.device != device);
        self.buckets.remove(device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{cell::Cell, rc::Rc};

    const TABS: &str = "const s=visual.state({tab:0});return () => visual.column([visual.tabs(['Bar','Line','Area'],s.tab,i=>s.tab=i),visual.text('tab '+s.tab)]);";
    const DOC: &str = "tabs";

    fn key(device: &str, item: &str) -> Key {
        Key {
            device: device.into(),
            conversation: "s1".into(),
            turn: "t1".into(),
            item: item.into(),
        }
    }

    fn handler(tree: &Value) -> String {
        tree["children"][0]["handler"].as_str().unwrap().to_owned()
    }

    fn first(source: &str) -> Value {
        serde_json::to_value(VisualRuntime::new(source).unwrap().render().unwrap()).unwrap()
    }

    fn tree(outcome: Outcome) -> (Value, bool) {
        match outcome {
            Outcome::Tree { tree, reset } => (tree, reset),
            Outcome::Failed(error) => panic!("{error}"),
        }
    }

    fn go(remote: &mut Remote, k: Key, handler: &str, value: Value) -> Outcome {
        remote.dispatch(k, DOC, TABS, handler, value)
    }

    /// A clock the test moves by hand.
    fn manual() -> (Remote, Rc<Cell<Duration>>) {
        let base = Instant::now();
        let at = Rc::new(Cell::new(Duration::ZERO));
        let read = at.clone();
        (Remote::with_clock(Box::new(move || base + read.get())), at)
    }

    #[test]
    fn a_tab_tap_returns_the_changed_tree_and_the_next_tap_uses_its_handler() {
        let mut remote = Remote::default();
        let initial = first(TABS);
        let (next, reset) = tree(go(
            &mut remote,
            key("a", "r1"),
            &handler(&initial),
            json!(2),
        ));
        assert!(!reset);
        assert_eq!(next["children"][0]["selected"], 2);
        assert_eq!(next["children"][1]["text"], "tab 2");
        let (again, reset) = tree(go(&mut remote, key("a", "r1"), &handler(&next), json!(1)));
        assert!(!reset);
        assert_eq!(again["children"][1]["text"], "tab 1");
    }

    #[test]
    fn one_devices_tap_does_not_change_another_devices_runtime_or_a_fresh_render() {
        let mut remote = Remote::default();
        let initial = first(TABS);
        tree(go(
            &mut remote,
            key("a", "r1"),
            &handler(&initial),
            json!(2),
        ));
        let (other, reset) = tree(go(
            &mut remote,
            key("b", "r1"),
            &handler(&initial),
            json!(1),
        ));
        assert!(!reset, "the second device starts from the first render");
        assert_eq!(other["children"][1]["text"], "tab 1");
        assert_eq!(first(TABS)["children"][1]["text"], "tab 0");
        assert_eq!(remote.count("a"), 1);
        assert_eq!(remote.count("b"), 1);
    }

    #[test]
    fn a_view_that_was_lost_taps_on_a_fresh_runtime_and_only_a_forged_handler_resets() {
        let mut remote = Remote::default();
        let initial = first(TABS);
        let (moved, _) = tree(go(
            &mut remote,
            key("a", "r1"),
            &handler(&initial),
            json!(2),
        ));
        // The device reopened the chat: it holds the first render again and taps with its handler. The tap is applied.
        let (again, reset) = tree(go(
            &mut remote,
            key("a", "r1"),
            &handler(&initial),
            json!(1),
        ));
        assert!(
            !reset,
            "the first render's handler is valid on a fresh runtime"
        );
        assert_eq!(again["children"][1]["text"], "tab 1");
        assert_ne!(again, moved);
        // A handler that no fresh runtime has answers the first render with reset.
        let (forged, reset) = tree(go(&mut remote, key("a", "r1"), "7:3", json!(1)));
        assert!(reset);
        assert_eq!(forged, initial);
        let (forged, reset) = tree(go(&mut remote, key("a", "r9"), "7:3", json!(1)));
        assert!(reset);
        assert_eq!(forged, initial);
        // After a reset the next tap works on the tree it got.
        let (next, reset) = tree(go(&mut remote, key("a", "r1"), &handler(&forged), json!(1)));
        assert!(!reset);
        assert_eq!(next["children"][1]["text"], "tab 1");
    }

    #[test]
    fn a_stale_handler_never_rebuilds_a_runtime_nothing_worked_on() {
        let mut remote = Remote::default();
        let initial = first(TABS);
        let (_, reset) = tree(go(&mut remote, key("a", "r1"), "7:3", json!(1)));
        assert!(reset);
        assert_eq!(remote.builds, 1);
        for _ in 0..20 {
            let (forged, reset) = tree(go(&mut remote, key("a", "r1"), "7:3", json!(1)));
            assert!(reset);
            assert_eq!(forged, initial);
        }
        assert_eq!(
            remote.builds, 1,
            "the stored first render answered every one"
        );
        // A runtime that has moved on is rebuilt once, then the failed retry leaves a runtime that has not.
        tree(go(
            &mut remote,
            key("a", "r2"),
            &handler(&initial),
            json!(1),
        ));
        assert_eq!(remote.builds, 2);
        tree(go(&mut remote, key("a", "r2"), "7:3", json!(1)));
        assert_eq!(remote.builds, 3);
        for _ in 0..20 {
            tree(go(&mut remote, key("a", "r2"), "7:3", json!(1)));
        }
        assert_eq!(remote.builds, 3);
    }

    #[test]
    fn a_value_the_control_cannot_take_fails_and_keeps_the_runtime() {
        let mut remote = Remote::default();
        let initial = first(TABS);
        let (moved, _) = tree(go(
            &mut remote,
            key("a", "r1"),
            &handler(&initial),
            json!(1),
        ));
        let Outcome::Failed(error) = go(&mut remote, key("a", "r1"), &handler(&moved), json!(9))
        else {
            panic!("a tab index out of range must fail");
        };
        assert!(error.contains("Invalid value"), "{error}");
        let (still, reset) = tree(go(&mut remote, key("a", "r1"), &handler(&moved), json!(0)));
        assert!(
            !reset,
            "the runtime and its handlers survive a refused value"
        );
        assert_eq!(still["children"][1]["text"], "tab 0");
    }

    #[test]
    fn a_script_that_fails_is_dropped_and_a_source_that_does_not_run_fails() {
        let mut remote = Remote::default();
        let source = "return () => visual.button('Boom',()=>{throw new Error('x')});";
        let initial = first(source);
        let Outcome::Failed(_) = remote.dispatch(
            key("a", "r1"),
            DOC,
            source,
            initial["handler"].as_str().unwrap(),
            Value::Null,
        ) else {
            panic!("a throwing handler must fail");
        };
        assert_eq!(remote.len(), 0, "a stopped runtime is not kept");
        assert!(matches!(
            remote.dispatch(
                key("a", "r2"),
                DOC,
                "this is not a program(",
                "1:0",
                Value::Null
            ),
            Outcome::Failed(_)
        ));
    }

    #[test]
    fn runtimes_are_bounded_per_device_and_in_all_and_the_least_recently_used_goes() {
        let mut remote = Remote::default();
        let initial = first(TABS);
        let h = handler(&initial);
        for item in 0..6 {
            tree(go(&mut remote, key("a", &format!("r{item}")), &h, json!(1)));
        }
        assert_eq!(remote.count("a"), MAX_PER_DEVICE);
        for device in 0..10 {
            for item in 0..MAX_PER_DEVICE {
                tree(go(
                    &mut remote,
                    key(&format!("d{device}"), &format!("r{item}")),
                    &h,
                    json!(1),
                ));
            }
        }
        assert_eq!(remote.len(), MAX_TOTAL);
        assert!(remote.cached_source(&key("a", "r5"), DOC).is_none());
        remote.forget("a");
        assert_eq!(remote.count("a"), 0);
    }

    #[test]
    fn a_changed_source_is_not_run_by_the_old_runtime_and_a_cached_source_answers_by_document() {
        let mut remote = Remote::default();
        let initial = first(TABS);
        tree(go(
            &mut remote,
            key("a", "r1"),
            &handler(&initial),
            json!(2),
        ));
        assert_eq!(
            remote.cached_source(&key("a", "r1"), DOC),
            Some((DOC.into(), TABS.into()))
        );
        assert!(remote.cached_source(&key("a", "r1"), "other").is_none());
        assert!(remote.cached_source(&key("b", "r1"), DOC).is_none());
        let other = TABS.replace("'Bar'", "'Pie'");
        let (next, reset) =
            tree(remote.dispatch(key("a", "r1"), DOC, &other, &handler(&initial), json!(1)));
        assert!(!reset);
        assert_eq!(next["children"][0]["labels"][0], "Pie");
        assert_eq!(next["children"][1]["text"], "tab 1");
        assert_eq!(remote.count("a"), 1);
    }

    #[test]
    fn a_device_may_burst_then_is_limited_to_its_rate_and_others_are_not_affected() {
        let (mut remote, at) = manual();
        let burst = (0..20).filter(|_| remote.allow("a")).count();
        assert_eq!(burst, COUNT_BURST as usize);
        assert!(remote.allow("b"), "another device has its own allowance");
        // 100 ms refills one dispatch at 10 a second.
        at.set(Duration::from_millis(100));
        assert!(remote.allow("a"));
        assert!(!remote.allow("a"));
        // A long pause never lets more than the burst through.
        at.set(Duration::from_secs(60));
        assert_eq!(
            (0..20).filter(|_| remote.allow("a")).count(),
            COUNT_BURST as usize
        );
    }

    #[test]
    fn foreground_time_is_charged_per_device_and_overall_and_refills() {
        let (mut remote, at) = manual();
        assert!(remote.allow("a"));
        // One rebuild-sized call uses the device's whole bucket: it waits, another device does not.
        remote.charge("a", Duration::from_millis(320));
        assert!(!remote.allow("a"), "the device is out of foreground time");
        assert!(remote.allow("b"));
        // 150 ms a second: after a second the debt (-20 ms) is paid and 130 ms is left.
        at.set(Duration::from_secs(1));
        assert!(remote.allow("a"));
        // Everyone together: the 600 ms bucket is used up by two devices, so a third waits too.
        let (mut remote, at) = manual();
        for device in ["x", "y"] {
            assert!(remote.allow(device));
            remote.charge(device, Duration::from_millis(300));
        }
        assert!(!remote.allow("z"), "all devices together used their budget");
        at.set(Duration::from_secs(1));
        assert!(remote.allow("z"));
    }

    #[test]
    fn a_dispatch_is_charged_the_time_the_clock_says_it_took() {
        let base = Instant::now();
        let slow = Rc::new(Cell::new(false));
        let ticks = Rc::new(Cell::new(0u64));
        let (is_slow, count) = (slow.clone(), ticks.clone());
        // The clock stands still, except that while `slow` is set every reading is 600 ms later than the one before.
        let mut remote = Remote::with_clock(Box::new(move || {
            if is_slow.get() {
                count.set(count.get() + 1);
            }
            base + Duration::from_millis(600 * count.get())
        }));
        assert!(remote.allow("a"));
        let initial = first(TABS);
        slow.set(true);
        tree(go(
            &mut remote,
            key("a", "r1"),
            &handler(&initial),
            json!(1),
        ));
        slow.set(false);
        // The call read the clock twice, 600 ms apart: more than the device's whole bucket, and the refill since is less.
        assert!(!remote.allow("a"));
        assert!(remote.allow("b"));
    }
}
