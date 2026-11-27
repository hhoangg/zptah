//! An opt-in benchmark for switching between conversations, driven from the
//! app itself so no one has to click. It does nothing unless
//! `MONOCODE_PERF_SWITCH` is set: every hook starts with `enabled()`, a cached
//! environment check, and returns before it touches any state.
//!
//! - `MONOCODE_PERF_SWITCH_LOG=<file>` appends the result lines there (stderr
//!   otherwise); `_LABEL` names the build in them.
//! - `_TARGET` and `_FILLER` are the titles of the long chat and of the short
//!   chat shown while the long one is away (`Perf long chat`, `Perf filler`).
//! - `_TRIALS` (20) and `_WARMUP` (2) count the measured and discarded trials
//!   of each scenario; `_SCENARIOS` lists them (`uncached,evicted,anchor,hit`).
//! - `_ANCHOR_TURNS` (15) is how many prompts from the end the anchor
//!   scenario scrolls to; `_EXIT` (1) quits the app after the last scenario.
//!
//! The hooks sit at the same logical points whatever the loading code does
//! (job done, install done, status Ready, first painted frame), so two builds
//! report comparable numbers. `shell/perf_switch.rs` is the driver and
//! `transcript/perf_switch.rs` reads the transcript. scripts/perf/switch-run.sh
//! runs the whole thing on a generated database.
use gpui::EntityId;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io::Write,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicI64, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("MONOCODE_PERF_SWITCH")
            .is_some_and(|value| !value.is_empty() && value != "0")
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Scenario {
    /// The conversation entity does not exist: a first open.
    Uncached,
    /// The entity lives but its history was released.
    Evicted,
    /// Like `Evicted`, after scrolling to a prompt in the middle.
    Anchor,
    /// The history is still loaded.
    Hit,
}
impl Scenario {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Scenario::Uncached => "uncached",
            Scenario::Evicted => "evicted",
            Scenario::Anchor => "anchor",
            Scenario::Hit => "hit",
        }
    }
    fn parse(name: &str) -> Option<Self> {
        [Self::Uncached, Self::Evicted, Self::Anchor, Self::Hit]
            .into_iter()
            .find(|scenario| scenario.name() == name.trim())
    }
}

pub(crate) struct Config {
    pub label: String,
    pub target: String,
    pub filler: String,
    pub trials: usize,
    pub warmup: usize,
    pub scenarios: Vec<Scenario>,
    pub anchor_turns: usize,
    pub exit: bool,
    log: Option<PathBuf>,
}
pub(crate) fn config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let text = |name: &str, default: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| default.into())
        };
        let number = |name: &str, default: usize| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(default)
        };
        let scenarios: Vec<_> = text(
            "MONOCODE_PERF_SWITCH_SCENARIOS",
            "uncached,evicted,anchor,hit",
        )
        .split(',')
        .filter_map(Scenario::parse)
        .collect();
        Config {
            label: text("MONOCODE_PERF_SWITCH_LABEL", "build"),
            target: text("MONOCODE_PERF_SWITCH_TARGET", "Perf long chat"),
            filler: text("MONOCODE_PERF_SWITCH_FILLER", "Perf filler"),
            trials: number("MONOCODE_PERF_SWITCH_TRIALS", 20).max(1),
            warmup: number("MONOCODE_PERF_SWITCH_WARMUP", 2),
            scenarios,
            anchor_turns: number("MONOCODE_PERF_SWITCH_ANCHOR_TURNS", 15).max(1),
            exit: number("MONOCODE_PERF_SWITCH_EXIT", 1) != 0,
            log: std::env::var_os("MONOCODE_PERF_SWITCH_LOG")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
        }
    })
}

/// Appends one result line to the log file, or prints it.
pub(crate) fn log(line: &str) {
    if let Some(path) = &config().log
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        && writeln!(file, "{line}").is_ok()
    {
        return;
    }
    eprintln!("{line}");
}

/// What the top of the list shows: the source row (by content), which part of
/// it, and how far the list is scrolled into it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reading {
    hash: u64,
    part: usize,
    offset: f32,
}
impl Reading {
    pub(crate) fn new(role: &str, text: &str, part: usize, offset: f32) -> Self {
        let mut hasher = DefaultHasher::new();
        role.hash(&mut hasher);
        text.hash(&mut hasher);
        Self {
            hash: hasher.finish(),
            part,
            offset,
        }
    }
    /// The same row and part, within a pixel.
    fn offset_from(&self, expected: &Self) -> Option<f32> {
        (self.hash == expected.hash && self.part == expected.part)
            .then_some(self.offset - expected.offset)
    }
}

/// The trial being timed. Everything is measured from `t0`.
struct Armed {
    id: i64,
    /// The transcripts whose frames count: the chat's own, and a preview of it.
    transcripts: Vec<EntityId>,
    t0: Instant,
    first_paint: Option<Duration>,
    ready: Option<Duration>,
    job: Option<Duration>,
    install: Option<Duration>,
    rows: usize,
    reading: Option<Reading>,
}
static ARMED: Mutex<Option<Armed>> = Mutex::new(None);
static PINNED: AtomicI64 = AtomicI64::new(i64::MIN);

fn armed<R>(with: impl FnOnce(&mut Armed) -> R) -> Option<R> {
    ARMED.lock().ok()?.as_mut().map(with)
}

/// Starts timing a switch to session `id`. `transcript` is its transcript
/// when the entity already exists; otherwise the install hook learns it.
pub(crate) fn arm(id: i64, transcript: Option<EntityId>) {
    if let Ok(mut state) = ARMED.lock() {
        *state = Some(Armed {
            id,
            transcripts: transcript.into_iter().collect(),
            t0: Instant::now(),
            first_paint: None,
            ready: None,
            job: None,
            install: None,
            rows: 0,
            reading: None,
        });
    }
}

/// The state of the trial in progress: first paint seen, ready seen.
pub(crate) fn progress() -> (bool, bool) {
    armed(|trial| (trial.first_paint.is_some(), trial.ready.is_some())).unwrap_or_default()
}

/// Everything one trial measured, ready to log.
pub(crate) struct Outcome {
    pub scenario: Scenario,
    pub trial: usize,
    pub warmup: bool,
    pub session: i64,
    pub rows: usize,
    pub first_paint: Option<Duration>,
    pub ready: Option<Duration>,
    pub job: Option<Duration>,
    pub install: Option<Duration>,
    pub evicted: bool,
    /// For the anchor scenario: how far the reading landed from where it was left.
    pub anchor: Option<Option<f32>>,
    pub timeout: bool,
}

/// Stops timing and returns the trial. `expected` is the reading left behind.
pub(crate) fn finish(
    scenario: Scenario,
    trial: usize,
    warmup: bool,
    evicted: bool,
    expected: Option<Reading>,
    timeout: bool,
) -> Option<Outcome> {
    let state = ARMED.lock().ok()?.take()?;
    let hit = scenario == Scenario::Hit;
    Some(Outcome {
        scenario,
        trial,
        warmup,
        session: state.id,
        rows: state.rows,
        first_paint: state.first_paint,
        // A hit loads nothing: it is ready when it paints.
        ready: state.ready.or(state.first_paint.filter(|_| hit)),
        job: state.job,
        install: state.install,
        evicted,
        anchor: expected.map(|expected| {
            state
                .reading
                .and_then(|reading| reading.offset_from(&expected))
        }),
        timeout,
    })
}

fn ms(duration: Option<Duration>) -> String {
    duration.map_or("-".into(), |d| format!("{:.1}", d.as_secs_f64() * 1000.))
}

impl Outcome {
    pub(crate) fn anchor_ok(&self) -> Option<bool> {
        self.anchor
            .map(|dpx| dpx.is_some_and(|dpx| dpx.abs() <= 1.))
    }
    pub(crate) fn line(&self, label: &str) -> String {
        let anchor = match self.anchor {
            None => "- anchor_dpx=-".to_string(),
            Some(None) => "miss anchor_dpx=-".to_string(),
            Some(Some(dpx)) => format!(
                "{} anchor_dpx={dpx:.1}",
                if dpx.abs() <= 1. { "ok" } else { "miss" }
            ),
        };
        format!(
            "perf-switch v=1 label={label} scenario={} trial={} warmup={} session={} rows={} first_paint_ms={} ready_ms={} job_ms={} install_ms={} evicted={} anchor={anchor} timeout={}",
            self.scenario.name(),
            self.trial,
            u8::from(self.warmup),
            self.session,
            self.rows,
            ms(self.first_paint),
            ms(self.ready),
            ms(self.job),
            ms(self.install),
            u8::from(self.evicted),
            u8::from(self.timeout),
        )
    }
}

/// Nearest-rank percentile of an ascending list.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// The summary line of one scenario's measured trials.
pub(crate) fn summary(label: &str, scenario: Scenario, outcomes: &[Outcome]) -> String {
    let measured: Vec<_> = outcomes.iter().filter(|o| !o.warmup).collect();
    let sorted = |pick: fn(&Outcome) -> Option<Duration>| {
        let mut values: Vec<f64> = measured
            .iter()
            .filter(|o| !o.timeout)
            .filter_map(|o| pick(o))
            .map(|d| d.as_secs_f64() * 1000.)
            .collect();
        values.sort_by(f64::total_cmp);
        values
    };
    let stat = |values: &[f64], p: f64| {
        if values.is_empty() {
            "-".to_string()
        } else {
            format!("{:.1}", percentile(values, p))
        }
    };
    let paint = sorted(|o| o.first_paint);
    let ready = sorted(|o| o.ready);
    let checked: Vec<_> = measured.iter().filter_map(|o| o.anchor_ok()).collect();
    let anchor = if checked.is_empty() {
        "-".to_string()
    } else {
        format!(
            "{}/{}",
            checked.iter().filter(|ok| **ok).count(),
            checked.len()
        )
    };
    format!(
        "perf-switch summary v=1 label={label} scenario={} n={} first_paint_p50={} first_paint_p95={} first_paint_max={} ready_p50={} ready_p95={} anchor_ok={anchor} timeouts={}",
        scenario.name(),
        paint.len(),
        stat(&paint, 0.5),
        stat(&paint, 0.95),
        stat(&paint, 1.),
        stat(&ready, 0.5),
        stat(&ready, 0.95),
        measured.iter().filter(|o| o.timeout).count(),
    )
}

/// Marks the start of a stretch to time; `None` when the benchmark is off.
pub(crate) fn clock() -> Option<Instant> {
    enabled().then(Instant::now)
}
/// The history job for session `id` finished (it runs on a worker thread).
pub(crate) fn job(id: i64, started: Option<Instant>) {
    if let Some(started) = started {
        armed(|trial| {
            if trial.id == id && trial.job.is_none() {
                trial.job = Some(started.elapsed());
            }
        });
    }
}
/// The loaded history was handed to the transcript of session `id`.
pub(crate) fn installed(id: i64, transcript: EntityId, started: Option<Instant>) {
    if let Some(started) = started {
        armed(|trial| {
            if trial.id == id && trial.install.is_none() {
                trial.install = Some(started.elapsed());
                if !trial.transcripts.contains(&transcript) {
                    trial.transcripts.push(transcript);
                }
            }
        });
    }
}
/// A display-only preview of session `id`'s history was made: its frames
/// count as the chat painting.
pub(crate) fn watch(id: i64, transcript: EntityId) {
    if !enabled() {
        return;
    }
    armed(|trial| {
        if trial.id == id && !trial.transcripts.contains(&transcript) {
            trial.transcripts.push(transcript);
        }
    });
}
/// Session `id` finished loading and shows "Ready".
pub(crate) fn ready(id: i64) {
    if !enabled() {
        return;
    }
    armed(|trial| {
        if trial.id == id && trial.ready.is_none() {
            trial.ready = Some(trial.t0.elapsed());
        }
    });
}
/// A transcript laid out its list. The first such frame of the timed
/// transcript that has messages is the first paint.
pub(crate) fn frame(
    transcript: EntityId,
    has_messages: bool,
    rows: usize,
    reading: impl FnOnce() -> Option<Reading>,
) {
    if !enabled() || !has_messages {
        return;
    }
    armed(|trial| {
        if trial.transcripts.contains(&transcript) && trial.first_paint.is_none() {
            trial.first_paint = Some(trial.t0.elapsed());
            trial.rows = rows;
            trial.reading = reading();
        }
    });
}

/// Keeps the history of `id` from being trimmed while inactive.
pub(crate) fn pin(id: i64) {
    PINNED.store(id, Ordering::Relaxed);
}
pub(crate) fn unpin() {
    PINNED.store(i64::MIN, Ordering::Relaxed);
}
pub(crate) fn pinned(id: i64) -> bool {
    enabled() && PINNED.load(Ordering::Relaxed) == id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let values: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&values, 0.5), 10.);
        assert_eq!(percentile(&values, 0.95), 19.);
        assert_eq!(percentile(&values, 1.), 20.);
        assert_eq!(percentile(&[7.], 0.95), 7.);
    }

    #[test]
    fn a_reading_matches_the_same_row_within_a_pixel() {
        let left = Reading::new("user", "Prompt", 2, 40.);
        assert_eq!(
            Reading::new("user", "Prompt", 2, 40.5).offset_from(&left),
            Some(0.5)
        );
        assert_eq!(
            Reading::new("user", "Prompt", 3, 40.).offset_from(&left),
            None
        );
        assert_eq!(
            Reading::new("user", "Other", 2, 40.).offset_from(&left),
            None
        );
    }

    #[test]
    fn the_summary_counts_only_measured_trials() {
        let outcome = |warmup, paint: u64| Outcome {
            scenario: Scenario::Hit,
            trial: 0,
            warmup,
            session: 1,
            rows: 3,
            first_paint: Some(Duration::from_millis(paint)),
            ready: Some(Duration::from_millis(paint)),
            job: None,
            install: None,
            evicted: false,
            anchor: None,
            timeout: false,
        };
        let outcomes = [outcome(true, 900), outcome(false, 10), outcome(false, 30)];
        let line = summary("test", Scenario::Hit, &outcomes);
        assert!(
            line.contains("n=2 first_paint_p50=10.0 first_paint_p95=30.0 first_paint_max=30.0")
        );
        assert!(line.contains("anchor_ok=-"));
        assert!(
            outcomes[1]
                .line("test")
                .contains("warmup=0 session=1 rows=3 first_paint_ms=10.0")
        );
    }
}
