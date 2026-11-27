//! Opt-in counters for the streaming transcript replay. Never record content.
use monocode_core::{
    tool::{ToolCall, tasks::Panel},
    transcript::Row,
};
use std::{cell::Cell, sync::OnceLock, time::Instant};

#[derive(Clone, Copy)]
pub(super) enum Site {
    Agent,
    Phase,
    Row,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Parse {
    pub calls: usize,
    pub bytes: usize,
    pub legacy_calls: usize,
    pub legacy_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Patch {
    start: usize,
    end: usize,
    streaming: bool,
    assistant_only: bool,
    tool_rows: usize,
    tool_bytes: usize,
    pub source_start: usize,
    source_rebuilt: usize,
    display_start: usize,
    display_rebuilt: usize,
    pub pinned: usize,
    parse: [Parse; 3],
}

thread_local! {
    static PATCH: Cell<Option<Patch>> = const { Cell::new(None) };
}

/// True when `MONOCODE_PERF_TRANSCRIPT=1`; read once per process.
pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MONOCODE_PERF_TRANSCRIPT").as_deref() == Ok("1"))
}

pub(super) fn begin(start: usize, rows: &[Row], streaming: bool) -> Option<Instant> {
    if !cfg!(test) && !enabled() {
        return None;
    }
    let mut patch = Patch {
        start,
        end: start + rows.len(),
        streaming,
        assistant_only: true,
        ..Default::default()
    };
    for row in rows {
        patch.assistant_only &= row.role == "assistant";
        if row.role == "tool" {
            patch.tool_rows += 1;
            patch.tool_bytes += row.text.len();
        }
    }
    PATCH.set(Some(patch));
    Some(Instant::now())
}

fn record(f: impl FnOnce(&mut Patch)) {
    if let Some(mut patch) = PATCH.get() {
        f(&mut patch);
        PATCH.set(Some(patch));
    }
}

pub(super) fn parse(site: Site, text: &str) -> serde_json::Result<ToolCall> {
    record(|patch| {
        let counters = &mut patch.parse[site as usize];
        counters.calls += 1;
        counters.bytes += text.len();
    });
    serde_json::from_str(text)
}

pub(super) fn panel(site: Site, call: &ToolCall) -> Option<Panel> {
    if call.panel.is_none() {
        record(|patch| {
            let counters = &mut patch.parse[site as usize];
            counters.legacy_calls += 1;
            counters.legacy_bytes += call.input.len();
        });
    }
    call.panel()
}

pub(super) fn rebuilt(
    source_start: usize,
    source_end: usize,
    display_start: usize,
    display_end: usize,
    pinned: usize,
) {
    record(|patch| {
        patch.source_start = source_start;
        patch.source_rebuilt = source_end - source_start;
        patch.display_start = display_start;
        patch.display_rebuilt = display_end - display_start;
        patch.pinned = pinned;
    });
}

pub(super) fn finish(started: Option<Instant>) {
    let Some(started) = started else { return };
    let micros = started.elapsed().as_micros();
    let Some(patch) = PATCH.take() else { return };
    if cfg!(test) {
        return;
    }
    let [agent, phase, row] = patch.parse;
    eprintln!(
        "transcript-patch start={} end={} streaming={} assistant_only={} tool_rows={} tool_bytes={} \
         source_start={} source_rebuilt={} display_start={} display_rebuilt={} pinned={} us={} \
         agent_calls={} agent_bytes={} agent_legacy_calls={} agent_legacy_bytes={} \
         phase_calls={} phase_bytes={} phase_legacy_calls={} phase_legacy_bytes={} row_calls={} row_bytes={}",
        patch.start,
        patch.end,
        u8::from(patch.streaming),
        u8::from(patch.assistant_only),
        patch.tool_rows,
        patch.tool_bytes,
        patch.source_start,
        patch.source_rebuilt,
        patch.display_start,
        patch.display_rebuilt,
        patch.pinned,
        micros,
        agent.calls,
        agent.bytes,
        agent.legacy_calls,
        agent.legacy_bytes,
        phase.calls,
        phase.bytes,
        phase.legacy_calls,
        phase.legacy_bytes,
        row.calls,
        row.bytes,
    );
}
