# Streaming profiles

A repeatable stream for measuring the UI's CPU while a reply arrives: no
agent, no network, the same text at the same pace every run.

- `fake-claude-stream.py` stands in for `claude`. Each message gets one long
  Markdown reply (prose, lists, code fences, tables) streamed as text deltas,
  every 40 ms by default. `REPLAY_SECONDS`, `REPLAY_INTERVAL_MS` and
  `REPLAY_CHARS` change its length and pace. The test
  `provider::process_tests::perf_replay_streams_a_whole_claude_turn` keeps it in
  step with the parser.
- `stream-profile.sh [binary] [seconds]` opens a separate Monocode window on
  throwaway data with the replay as Claude, waits for you to send a message,
  then profiles that window with Monocode Performance's `--profile`.

```sh
scripts/perf/stream-profile.sh                                   # the installed app
scripts/perf/stream-profile.sh target/release/monocode-gpui 30   # a local build
```

- `frame-run.sh [binary] [seconds]` does the same hands-off:
  `MONOCODE_PERF_PROMPT` makes the first chat switch to the replayed Claude
  and send a message by itself, and `MONOCODE_FRAME_LOG` writes one line per
  second. It prints the log and its averages. `PROFILE=1` also samples stacks
  over the same window.

```sh
scripts/perf/frame-run.sh crates/monocode-gpui/target/release/monocode-gpui 20
PROFILE=1 scripts/perf/frame-run.sh crates/monocode-gpui/target/release/monocode-gpui 15
```

A frame log line reads:

```
fps=39 layout=34 paint=5 refresh=0 draw=3.35/5.10ms busy=12.7% main=14.8% views=330/30
  by: session_mark::SessionMark=60 transcript::TranscriptView=39 ... | measure:markdown=7417 shape:lines=91
```

`fps` splits into frames where content changed (`layout`), animation-only
frames (`paint`) and whole-window redraws (`refresh`). `draw` is the average
and slowest `Window::draw`, `busy` the share of the second spent drawing,
`main` the main thread's CPU. `views` is entity views rendered vs. cached ones
reused. `by` names the views whose own `notify` asked for those frames. After
`|` come counts the app adds with `gpui::frame_stats::count` / `add`: fresh
row measurements in microseconds by row kind, and lines and characters GPUI
had to shape because its line cache missed. Any run of the app takes
`MONOCODE_FRAME_LOG=<file>`, not just the replay.

Compare a change with the report from before it (CPU, main thread, and the
view and layout rows). Profile a release build; a debug build's numbers say
little about release.

## Tool-heavy live transcript

`live-transcript-run.py` is macOS-only (it uses libproc, `lsappinfo` and
`mach_timebase_info`). It measures a stable window after the text stream
starts. `--tools tools` sends 24 Read results of 48 KiB each first, and
`--tools agent` adds a delegated Agent result. Use `--tools text` as the
shorter-path control. `--stage live-scroll` also drives scrolling before
the reply ends. `settled` and `settled-scroll` measure after the reply.

```sh
python3 scripts/perf/live-transcript-run.py path/to/release/monocode-gpui /tmp/replay-one --tools agent
python3 scripts/perf/live-transcript-run.py path/to/release/monocode-gpui /tmp/replay-two --tools agent --stage live-scroll
```

The runner uses a fake CLI and an ephemeral home, project and database.
It verifies the persisted reply digest and delivered tool results before
keeping the process CPU, memory footprint, foreground observations, frame
log, patch timings and aggregate parse/rebuild counts. Compare binaries
with the same fixture, stage, cadence and foreground state. The release
baseline must include the same `MONOCODE_PERF_TRANSCRIPT=1` probes as the
candidate so both record the same measurements.

## Switching to a long conversation

`switch-run.sh [binary] [label]` measures how long the app takes to show a
very long chat when you switch back to it, with nobody clicking.
`long-chat-fixture.py` writes a database in zptah's own schema with a
"Perf long chat" (3200 blocks, 40 prompts, about 36 MiB, synthetic text only,
seeded) and a short "Perf filler". The app is started on it with
`MONOCODE_PERF_SWITCH=1`, which makes it open the two chats in turn:

- `uncached`: the chat's entity does not exist, a first open
- `evicted`: its history was released while another chat was shown
- `anchor`: like `evicted`, after scrolling 15 prompts back from the end;
  `anchor=ok` means the same row is at the top, within 1 px
- `hit`: its history is still loaded

```sh
scripts/perf/switch-run.sh crates/monocode-gpui/target/release/monocode-gpui base
scripts/perf/switch-run.sh /path/to/candidate/monocode-gpui candidate
```

Each trial logs a line and each scenario a summary (the app quits when done;
the log path is printed):

```
perf-switch v=1 label=base scenario=uncached trial=3 warmup=0 session=1 rows=9533 first_paint_ms=412.0 ready_ms=388.1 job_ms=301.5 install_ms=84.2 evicted=0 anchor=- anchor_dpx=- timeout=0
perf-switch summary v=1 label=base scenario=uncached n=20 first_paint_p50=412.0 first_paint_p95=455.3 first_paint_max=470.1 ready_p50=388.0 ready_p95=430.2 anchor_ok=- timeouts=0
```

`first_paint_ms` and `ready_ms` count from the moment the switch starts to
the first frame that shows the chat's history and to its "Ready" status.
`job_ms` is the store read and preparation on the worker, `install_ms` the
hand-over to the transcript on the main thread, and `rows` the display rows
that frame laid out. Percentiles are nearest-rank. The `MONOCODE_PERF_SWITCH_*`
settings (log file, label, titles, trials, warmup, scenarios, anchor turns,
exit) are listed at the top of `crates/monocode-gpui/src/perf_switch.rs`; the
script also takes `TRIALS`, `WARMUP`, `SCENARIOS`, `ANCHOR_TURNS`, `BLOCKS` and
`TARGET_MIB`. Nothing runs, and nothing is measured, when
`MONOCODE_PERF_SWITCH` is unset. Compare two builds that both contain these
probes, run back to back on an idle machine.
