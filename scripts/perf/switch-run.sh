#!/bin/sh
# Measure switching back to a very long conversation, hands-off.
#
#   scripts/perf/switch-run.sh [path/to/monocode-gpui] [label]
#
# Writes a throwaway database with one long synthetic chat ("Perf long chat",
# see long-chat-fixture.py) and a short one ("Perf filler"), starts the app on
# it with MONOCODE_PERF_SWITCH=1, and lets the app switch between the two over
# and over (first open, evicted history, evicted at a scroll position, history
# still loaded). It prints one line per trial and a summary per scenario, then
# the app quits itself. Run it on a release build, with the machine otherwise
# idle, once per build, with the same label scheme (for example `base` and
# `candidate`), and compare the summaries.
#
# Settings (environment): TRIALS (20), WARMUP (2), SCENARIOS
# (uncached,evicted,anchor,hit), ANCHOR_TURNS (15), BLOCKS (3200),
# TARGET_MIB (36), WORK_DIR (a new temporary folder), TIMEOUT_SECONDS (900).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
app=${1:-"$root/crates/monocode-gpui/target/release/monocode-gpui"}
label=${2:-build}
replay="$here/fake-claude-stream.py"

[ -x "$app" ] || { echo "No Monocode binary at $app" >&2; exit 1; }

work=${WORK_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/monocode-switch-run.XXXXXX")}
mkdir -p "$work/data" "$work/project"
log="$work/switch.log"
: >"$log"

python3 "$here/long-chat-fixture.py" --db "$work/data/zptah.db" --force \
    --schema "$root/crates/monocode-core/src/store/shared_schema.sql" \
    --cwd "$work/project" --blocks "${BLOCKS:-3200}" --target-mib "${TARGET_MIB:-36}"

# Read the database once so it is in the page cache before the first trial.
cat "$work/data/zptah.db" >/dev/null

MONOCODE_NATIVE_DATA_DIR="$work/data" MONOCODE_CLAUDE_BINARY="$replay" \
    MONOCODE_PERF_SWITCH=1 MONOCODE_PERF_SWITCH_LOG="$log" \
    MONOCODE_PERF_SWITCH_LABEL="$label" \
    MONOCODE_PERF_SWITCH_TRIALS="${TRIALS:-20}" \
    MONOCODE_PERF_SWITCH_WARMUP="${WARMUP:-2}" \
    MONOCODE_PERF_SWITCH_SCENARIOS="${SCENARIOS:-uncached,evicted,anchor,hit}" \
    MONOCODE_PERF_SWITCH_ANCHOR_TURNS="${ANCHOR_TURNS:-15}" \
    "$app" --dev-data --cwd "$work/project" >"$work/app.log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true' EXIT INT TERM

waited=0
limit=${TIMEOUT_SECONDS:-900}
while kill -0 "$pid" 2>/dev/null; do
    waited=$((waited + 2))
    [ "$waited" -lt "$limit" ] || { echo "Timed out after ${limit}s; see $work/app.log" >&2; exit 1; }
    sleep 2
done
trap - EXIT INT TERM

if ! grep -q 'perf-switch summary' "$log"; then
    echo "No summary was written; see $work/app.log" >&2
    exit 1
fi
cat "$log"
echo
grep 'perf-switch summary' "$log"
echo "Log: $log"
exit 0
