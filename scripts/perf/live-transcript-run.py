#!/usr/bin/env python3
"""Run one isolated Native transcript benchmark and keep numeric evidence."""

import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import statistics
import subprocess
import tempfile
import time


def wait_for(process, condition, timeout, message):
    deadline = time.monotonic() + timeout
    while not condition():
        if process.poll() is not None or time.monotonic() >= deadline:
            raise RuntimeError(message)
        time.sleep(0.1)


def footprint_and_cpu(pid):
    # Darwin's rusage_info_v2 (libproc.h). It starts with a 16-byte UUID.
    info = (ctypes.c_uint64 * 21)()
    if LIBPROC.proc_pid_rusage(pid, 2, ctypes.byref(info)) != 0:
        raise RuntimeError("Could not sample the replay process")
    return (info[2] + info[3]) * TIMEBASE_NS, info[9]


def timebase_ns():
    # mach_timebase_info_data_t contains a numerator and denominator.
    ratio = (ctypes.c_uint32 * 2)()
    if ctypes.CDLL(None).mach_timebase_info(ctypes.byref(ratio)) != 0 or not ratio[1]:
        raise RuntimeError("Could not read the Darwin timebase")
    return ratio[0] / ratio[1]


def foreground(pid):
    tool = "/usr/bin/lsappinfo"
    front = subprocess.run([tool, "front"], capture_output=True, text=True, check=False)
    if not front.stdout.strip():
        return False
    details = subprocess.run(
        [tool, "info", "-only", "pid", front.stdout.strip()],
        capture_output=True, text=True, check=False,
    )
    return re.search(rf"\bpid\s*=\s*{pid}\b", details.stdout) is not None


def interval(process, seconds):
    started = time.monotonic()
    cpu_start, footprint = footprint_and_cpu(process.pid)
    memory = [footprint]
    focus = [foreground(process.pid)]
    for index in range(1, seconds + 1):
        time.sleep(max(0, started + index - time.monotonic()))
        cpu_end, footprint = footprint_and_cpu(process.pid)
        ended = time.monotonic()
        memory.append(footprint)
        focus.append(foreground(process.pid))
    return {
        "process_cpu_pct": (cpu_end - cpu_start) / (ended - started) / 1e7,
        "footprint_mib": statistics.median(memory) / 2**20,
        "foreground_observations": sum(focus),
        "foreground_samples": len(focus),
    }


def stored_turn(database):
    # An ordinary local chat keeps its whole history in sessions.blocks_json, a
    # JSON array of display blocks. An assistant block holds reply text. A tool
    # block holds the result in tool.detail and, for a delegated run, agentRun.
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as db:
        stored = db.execute(
            "SELECT blocks_json FROM sessions ORDER BY created_at,id"
        ).fetchall()
    assistant = []
    tools = []
    for (raw,) in stored:
        for block in json.loads(raw):
            if block.get("draft") is True or block.get("internal") is True:
                continue
            role = block.get("role")
            if role == "assistant" and not block.get("monoCard"):
                assistant.append(block.get("text") or "")
            elif role == "tool":
                meta = block.get("tool") or {}
                is_agent = isinstance(block.get("agentRun"), dict) or meta.get("kind") == "agent"
                tools.append({
                    "output": meta.get("detail") or (meta.get("preview") or {}).get("output") or "",
                    "panel": {"kind": "agent"} if is_agent else None,
                })
    return hashlib.sha256("".join(assistant).encode()).hexdigest(), tools


def stored_digest_is(database, expected):
    # Polling only: the database or its table may not exist yet.
    try:
        return stored_turn(database)[0] == expected
    except sqlite3.OperationalError:
        return False


def log_slice(path, first=0, last=None):
    with path.open("rb") as log:
        starts_at_line = True
        if first:
            log.seek(first - 1)
            starts_at_line = log.read(1) == b"\n"
        data = log.read() if last is None else log.read(last - first)
    if not starts_at_line:
        data = data.partition(b"\n")[2]
    if data and not data.endswith(b"\n"):
        data = data.rpartition(b"\n")[0]
    return data.decode(errors="replace").splitlines()


def records(lines, prefix):
    return [
        {key: int(value) for key, value in re.findall(r"(\w+)=(\d+)", line)}
        for line in lines if line.startswith(prefix)
    ]


def probe_summary(lines):
    patches = [
        row for row in records(lines, "transcript-patch ")
        if row["streaming"] and row["assistant_only"]
    ]
    sends = records(lines, "transcript-send ")
    result = {"assistant_patches": len(patches), "sends": len(sends)}
    if patches:
        times = sorted(row["us"] for row in patches)
        result.update({
            "p50_us": statistics.median(times),
            "p95_us": times[(95 * len(times) + 99) // 100 - 1],
            "stable_reordered_agent_max": max(row.get("pinned", 0) for row in patches),
            **{
                key: sum(row.get(key, 0) for row in patches)
                for key in patches[0] if key.endswith(("_calls", "_bytes", "_rebuilt"))
            },
        })
    result["sent_rows"] = sum(row["rows"] for row in sends)
    result["sent_bytes"] = sum(row["bytes"] for row in sends)
    return result


def run(args, output, scratch):
    home, project, data = (scratch / name for name in ("home", "project", "data"))
    for path in (home, project, data):
        path.mkdir()
    started, done = scratch / "text-started", scratch / "done"
    app_log, frame_log = output / "app.log", output / "frames.log"
    frame_log.touch()
    live = args.stage.startswith("live")
    env = {
        "HOME": str(home),
        "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "MONOCODE_NATIVE_DATA_DIR": str(data),
        "MONOCODE_CLAUDE_BINARY": str(Path(__file__).with_name("fake-claude-stream.py").resolve()),
        "MONOCODE_PERF_PROMPT": "Run the streaming performance fixture.",
        "MONOCODE_PERF_TRANSCRIPT": "1",
        "MONOCODE_FRAME_LOG": str(frame_log),
        "REPLAY_MARK": str(started),
        "REPLAY_DONE": str(done),
        "REPLAY_TOOLS": args.tools,
        "REPLAY_SECONDS": str(args.seconds + args.warmup + 3 if live else 12),
    }
    if args.stage.endswith("scroll"):
        env["MONOCODE_PERF_SCROLL"] = str(args.scroll_speed)
    if args.stage == "live-scroll":
        env["MONOCODE_PERF_SCROLL_LIVE"] = "1"
    with app_log.open("wb") as log:
        process = subprocess.Popen(
            [args.binary, "--dev-data", "--cwd", project],
            cwd=project, env=env, stdout=log, stderr=subprocess.STDOUT,
        )
        try:
            wait_for(process, started.exists, 45, f"Replay did not start. See {app_log}")
            if not live:
                wait_for(process, done.exists, 40, f"Replay did not finish. See {app_log}")
            time.sleep(args.warmup)
            log_start, frame_start = [p.stat().st_size for p in (app_log, frame_log)]
            result = interval(process, args.seconds)
            log_end, frame_end = [p.stat().st_size for p in (app_log, frame_log)]
            if live and done.exists():
                raise RuntimeError("Text replay ended inside the measurement interval")
            wait_for(process, done.exists, 30, f"Replay did not finish. See {app_log}")
            expected = started.read_text()
            database = data / "zptah.db"
            wait_for(
                process, lambda: stored_digest_is(database, expected), 15,
                "Complete fixture reply did not reach the isolated store",
            )
            digest, tools = stored_turn(database)
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    expected_count = {"text": 0, "tools": 24, "agent": 25}[args.tools]
    if len(tools) != expected_count or any(len(tool["output"]) != 49152 for tool in tools):
        raise RuntimeError("Unexpected delivered tool fixture count or output size")
    agents = sum((tool.get("panel") or {}).get("kind") == "agent" for tool in tools)
    if agents != (args.tools == "agent"):
        raise RuntimeError("Unexpected delivered Agent panel count")
    lines = log_slice(app_log, log_start, log_end)
    frames = log_slice(frame_log, frame_start, frame_end)
    (output / "window.log").write_text("\n".join(lines + frames) + "\n")
    fps = [int(match[1]) for line in frames if (match := re.search(r"\bfps=(\d+)", line))]
    main = [float(match[1]) for line in frames if (match := re.search(r"\bmain=([\d.]+)%", line))]
    probes = probe_summary(lines)
    if live and not probes["assistant_patches"]:
        raise RuntimeError(f"No streaming probes in the measurement interval. See {app_log}")
    result.update({
        "fixture": args.tools,
        "stage": args.stage,
        "seconds": args.seconds,
        "stored_reply_sha256": digest,
        "stored_tool_rows": len(tools),
        "stored_tool_output_bytes": sum(len(tool["output"].encode()) for tool in tools),
        "stored_agent_panels": agents,
        "reordered_agent_max": max(
            (row.get("pinned", 0) for row in records(log_slice(app_log), "transcript-patch ")),
            default=0,
        ),
        "mean_fps": statistics.mean(fps) if fps else 0,
        "mean_main_cpu_pct": statistics.mean(main) if main else None,
        **probes,
    })
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=lambda path: Path(path).resolve())
    parser.add_argument("output", type=Path, help="new directory for logs and summary")
    parser.add_argument("--tools", choices=("text", "tools", "agent"), default="agent")
    parser.add_argument("--stage", choices=("live", "live-scroll", "settled", "settled-scroll"),
                        default="live")
    parser.add_argument("--seconds", type=int, default=12)
    parser.add_argument("--warmup", type=float, default=4)
    parser.add_argument("--scroll-speed", type=float, default=1500)
    args = parser.parse_args()
    if args.seconds <= 0 or args.warmup < 0:
        parser.error("measurement must be positive and warm-up non-negative")
    args.output.mkdir(parents=True, exist_ok=False)
    LIBPROC = ctypes.CDLL("/usr/lib/libproc.dylib")
    TIMEBASE_NS = timebase_ns()
    with tempfile.TemporaryDirectory(prefix="zptah-live-transcript-") as directory:
        result = run(args, args.output, Path(directory))
    summary = json.dumps(result, sort_keys=True)
    (args.output / "summary.json").write_text(summary + "\n")
    print(summary)
