#!/usr/bin/env python3
"""Write a database with one very long synthetic chat for the switch benchmark.

    scripts/perf/long-chat-fixture.py --db DIR/zptah.db \
        --schema crates/monocode-core/src/store/shared_schema.sql --cwd /some/project

The database holds two sessions in zptah's own schema, so the app opens it
without any migration:

  "Perf long chat"  thousands of blocks (user prompts, reasoning, tool results,
                    Markdown replies) in one sessions.blocks_json document
  "Perf filler"     a short chat, newer, to switch away to

Everything is synthetic text from a seeded generator: the same flags always
write the same chat, and nothing comes from any real conversation. The script
refuses to overwrite a file (without --force) and to write inside the folder of
an installed zptah or Monoptah.
"""
import argparse
import json
import math
import os
import random
import sqlite3
import sys
from pathlib import Path

PROTECTED_DIRS = {
    "app.zptah.desktop",
    "zptah Development",
    "com.monoptah.desktop",
    "Monoptah Development",
    "com.monocode.desktop",
    "Monocode Development",
    "zptah",
    "Monoptah",
    ".zptah-host",
    ".monocode-host",
}
MAX_OUTPUT = 400 * 1024
EPOCH_MS = 1_760_000_000_000

WORDS = (
    "alpha beta gamma delta cache index parser buffer stream window layout render queue worker "
    "batch shard token vector matrix signal handler config module record column offset cursor "
    "session branch commit merge resolve compile verify measure replay stable latency throughput "
    "caf\u00e9 na\u00efve \u6f22\u5b57 \u0434\u0430\u043d\u043d\u044b\u0435 \U0001F642 \u2192 \u2713"
).split()
LANGS = ("rust", "python", "ts", "sh", "json")


def refuse_protected(path: Path) -> None:
    resolved = path.expanduser().resolve()
    for part in resolved.parts:
        if part in PROTECTED_DIRS:
            sys.exit(f"Refusing to write inside an installed app's data folder: {resolved}")
    for key in ("XDG_DATA_HOME", "LOCALAPPDATA", "APPDATA"):
        base = os.environ.get(key)
        if base:
            base_path = Path(base).resolve()
            try:
                rest = resolved.relative_to(base_path)
            except ValueError:
                continue
            if rest.parts and rest.parts[0] in PROTECTED_DIRS:
                sys.exit(f"Refusing to write inside an installed app's data folder: {resolved}")


class Text:
    """Seeded synthetic text of a requested length."""

    def __init__(self, rng: random.Random):
        self.rng = rng
        self.lines = {
            "log": [self.log_line(i) for i in range(1500)],
            "code": [self.code_line(i) for i in range(1500)],
            "grep": [self.grep_line(i) for i in range(1500)],
            "diff": [self.diff_line(i) for i in range(1500)],
        }

    def words(self, n: int) -> str:
        return " ".join(self.rng.choice(WORDS) for _ in range(n))

    def log_line(self, i: int) -> str:
        r = self.rng
        return (
            f"2026-03-{1 + i % 28:02d}T{i % 24:02d}:{r.randrange(60):02d}:{r.randrange(60):02d}Z "
            f"{r.choice(['INFO', 'WARN', 'DEBUG'])} worker-{r.randrange(16)} "
            f"{self.words(r.randrange(4, 12))} took {r.randrange(1, 900)}ms \"quoted\" path=C:\\\\tmp\\\\f{i}"
        )

    def code_line(self, i: int) -> str:
        r = self.rng
        indent = "    " * r.randrange(4)
        return f"{i + 1:>5}\t{indent}fn {r.choice(WORDS)}_{i}(value: &str) -> usize {{ value.len() + {r.randrange(99)} }} // {self.words(4)}"

    def grep_line(self, i: int) -> str:
        r = self.rng
        return f"src/{r.choice(WORDS)}/{r.choice(WORDS)}_{i % 40}.rs:{r.randrange(1, 3000)}: {self.words(r.randrange(5, 14))}"

    def diff_line(self, i: int) -> str:
        sign = self.rng.choice("+- ")
        return f"{sign}    let {self.rng.choice(WORDS)}_{i} = \"{self.words(3)}\"; // {self.words(3)}"

    def of(self, kind: str, size: int) -> str:
        pool = self.lines[kind]
        out, total = [], 0
        while total < size:
            line = self.rng.choice(pool)
            out.append(line)
            total += len(line.encode("utf-8")) + 1
        return "\n".join(out)

    def markdown(self, size: int) -> str:
        r = self.rng
        parts, total = [], 0
        while total < size:
            kind = r.random()
            if kind < 0.45:
                part = self.words(r.randrange(20, 70)).capitalize() + "."
            elif kind < 0.65:
                part = "\n".join(f"- **{self.words(2)}**: {self.words(r.randrange(4, 12))}" for _ in range(r.randrange(3, 8)))
            elif kind < 0.85:
                lang = r.choice(LANGS)
                part = f"```{lang}\n{self.of('code', r.randrange(200, 900))}\n```"
            else:
                rows = "\n".join(f"| {self.words(1)} | {r.randrange(1000)} | {self.words(3)} |" for _ in range(r.randrange(3, 7)))
                part = f"| name | count | note |\n|---|---:|---|\n{rows}"
            parts.append(part)
            total += len(part.encode("utf-8")) + 2
        return "\n\n".join(parts)


def model(harness: str = "claude"):
    return {"harness": harness, "id": f"{harness}:perf-model", "name": "Perf Model"}


def user_block(i, turn, text, rng, attachments):
    ts = EPOCH_MS + turn * 600_000
    block = {
        "id": f"u{i}",
        "role": "user",
        "text": text,
        "sentAt": ts,
        "startedAt": ts + 20,
        "durationMs": rng.randrange(20_000, 400_000),
        "turnModel": model(),
        "turnMetrics": {
            "inputTokens": rng.randrange(1_000, 90_000),
            "outputTokens": rng.randrange(100, 8_000),
            "cacheReadTokens": rng.randrange(0, 80_000),
            "cacheHitPercent": round(rng.uniform(20, 99), 2),
        },
    }
    if attachments:
        block["attachments"] = [
            {"id": f"att{i}-{k}", "path": f"/tmp/perf-attachment-{i}-{k}.png", "mimeType": "image/png", "kind": "image", "size": rng.randrange(10_000, 900_000)}
            for k in range(attachments)
        ]
    return block


def tool_block(i, rng, size, text):
    kind, name = rng.choice([("execute", "Bash"), ("read", "Read"), ("edit", "Edit"), ("search", "Grep")])
    source = {"execute": "log", "read": "code", "edit": "diff", "search": "grep"}[kind]
    path = f"src/{rng.choice(WORDS)}/{rng.choice(WORDS)}_{i % 50}.rs"
    title = {"execute": f"Run test {i % 97}", "read": f"Read {path}", "edit": f"Edit {path}", "search": f"Grep {rng.choice(WORDS)}"}[kind]
    raw = {
        "execute": {"command": f"cargo test --lib module_{i % 97} -- --nocapture"},
        "read": {"file_path": path},
        "edit": {"file_path": path, "old_string": "old", "new_string": "new"},
        "search": {"pattern": rng.choice(WORDS), "path": "src"},
    }[kind]
    return {
        "id": f"t{i}",
        "role": "tool",
        "text": title,
        "tool": {
            "callId": f"call-{i}",
            "kind": kind,
            "nativeName": name,
            "title": title,
            "status": "completed",
            "rawInput": raw,
            "detail": text.of(source, size),
            "preview": {"path": path, "title": title},
        },
    }


def long_chat(args, rng, text):
    turns = args.prompts
    per_turn = max(3, args.blocks // turns)
    # Block kinds per turn: a prompt, then ~20% reasoning, ~55% tool, ~25% reply.
    plan = []
    for turn in range(turns):
        count = per_turn if turn < turns - 1 else args.blocks - per_turn * (turns - 1)
        kinds = ["user"]
        for _ in range(count - 2):
            x = rng.random()
            kinds.append("reasoning" if x < 0.2 else "tool" if x < 0.75 else "assistant")
        kinds.append("assistant")
        plan.append(kinds)
    sigma = (math.log(64 * 1024) - math.log(6 * 1024)) / 1.2816
    sizes = {}
    fixed = 0
    for turn, kinds in enumerate(plan):
        for ix, kind in enumerate(kinds):
            if kind == "tool":
                sizes[(turn, ix)] = min(MAX_OUTPUT, int(rng.lognormvariate(math.log(6 * 1024), sigma)))
            elif kind == "user":
                fixed += 7_000 if turn % 10 == 3 else 300
            elif kind == "reasoning":
                fixed += 1_500
            else:
                fixed += 2_500
    # JSON escaping and the metadata around each output add about 6%.
    budget = int(args.target_mib * 1024 * 1024 * 0.94) - fixed
    for _ in range(6):
        total = sum(sizes.values())
        scale = budget / max(total, 1)
        sizes = {k: max(200, min(MAX_OUTPUT, int(v * scale))) for k, v in sizes.items()}
    blocks = []
    for turn, kinds in enumerate(plan):
        for ix, kind in enumerate(kinds):
            n = len(blocks)
            if kind == "user":
                body = text.words(rng.randrange(8, 40)).capitalize() + "?"
                if turn % 10 == 3:
                    body = text.markdown(7_000)
                blocks.append(user_block(n, turn, body, rng, 2 if turn % 7 == 2 else 0))
            elif kind == "tool":
                blocks.append(tool_block(n, rng, sizes[(turn, ix)], text))
            elif kind == "reasoning":
                blocks.append({"id": f"r{n}", "role": "reasoning", "text": text.markdown(rng.randrange(400, 2500))})
            else:
                blocks.append({"id": f"a{n}", "role": "assistant", "text": text.markdown(rng.randrange(800, 4500))})
    return blocks


def filler_chat(rng, text):
    blocks = [
        user_block(0, 0, "A short question " + text.words(5), rng, 0),
        {"id": "a1", "role": "assistant", "text": text.markdown(600)},
        user_block(2, 1, "Another one " + text.words(5), rng, 0),
        tool_block(3, rng, 900, text),
        {"id": "a4", "role": "assistant", "text": text.markdown(500)},
        {"id": "a5", "role": "assistant", "text": text.markdown(300)},
    ]
    return blocks


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--db", required=True, help="database file to create (usually DIR/zptah.db)")
    ap.add_argument("--schema", default="crates/monocode-core/src/store/shared_schema.sql")
    ap.add_argument("--cwd", default="/tmp/perf-project")
    ap.add_argument("--blocks", type=int, default=3200)
    ap.add_argument("--prompts", type=int, default=40)
    ap.add_argument("--target-mib", type=float, default=36)
    ap.add_argument("--seed", type=int, default=23)
    ap.add_argument("--long-title", default="Perf long chat")
    ap.add_argument("--filler-title", default="Perf filler")
    ap.add_argument("--force", action="store_true", help="replace an existing file")
    args = ap.parse_args()
    if args.prompts < 2 or args.blocks < args.prompts * 3:
        sys.exit("--blocks must be at least three per prompt, and --prompts at least 2")
    db = Path(args.db).expanduser()
    refuse_protected(db)
    if db.exists():
        if not args.force:
            sys.exit(f"{db} exists; pass --force to replace it")
        for suffix in ("", "-wal", "-shm"):
            Path(str(db) + suffix).unlink(missing_ok=True)
    db.parent.mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)
    text = Text(rng)
    long_blocks = long_chat(args, rng, text)
    filler_blocks = filler_chat(rng, text)
    conn = sqlite3.connect(db)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.executescript(Path(args.schema).read_text())
    model_id = "claude:perf-model"
    for sid, title, blocks, updated in (
        ("00000000-0000-4000-8000-000000000001", args.long_title, long_blocks, EPOCH_MS + 1_000_000),
        ("00000000-0000-4000-8000-000000000002", args.filler_title, filler_blocks, EPOCH_MS + 2_000_000),
    ):
        conn.execute(
            "INSERT INTO sessions(id,cwd,harness,model,model_settings,runtime_mode,title,"
            "provider_session_id,blocks_json,created_at,updated_at,has_user_message) "
            "VALUES(?,?,?,?,?,?,?,?,?,?,?,1)",
            (sid, args.cwd, "claude", model_id, "{}", "auto-accept-edits", title, None,
             json.dumps(blocks, ensure_ascii=False, separators=(",", ":")), EPOCH_MS, updated),
        )
    conn.commit()
    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    size = conn.execute("SELECT length(CAST(blocks_json AS BLOB)) FROM sessions WHERE title=?", (args.long_title,)).fetchone()[0]
    conn.close()
    print(f"{db}: {args.long_title}: {len(long_blocks)} blocks, {args.prompts} prompts, {size / 1048576:.1f} MiB; "
          f"{args.filler_title}: {len(filler_blocks)} blocks")


if __name__ == "__main__":
    main()
