// Canned workflows for the read-only Workflows screen (`workflows.list`, `workflows.runs`). Fake data only.
// One run carries a status and a node status a current desktop does not send yet, to show the screen
// tolerates what a later desktop adds.
import type { ResultOf } from "@monoptah/protocol";

const HOUR = 3_600_000;

interface FixtureWorkflow {
  summary: ResultOf<"workflows.list">["workflows"][number];
  runs: (now: number, sessionIds: string[]) => ResultOf<"workflows.runs">["runs"];
}

const WORKFLOWS: FixtureWorkflow[] = [
  {
    summary: {
      id: "wf-nightly",
      name: "Nightly checks",
      description: "Run the test suite and summarize failures.",
      enabled: true,
      triggers: [
        { id: "daily", type: "schedule", label: "Daily at 02:00" },
        { id: "manual", type: "manual", label: "Manual" },
      ],
      nextRunAt: null,
      lastRun: { status: "done", startedAt: 0 },
    },
    runs: (now, sessions) => [
      {
        id: "run-nightly-2",
        status: "done",
        caller: { kind: "schedule", triggerNode: "daily" },
        startedAt: now - 5 * HOUR,
        endedAt: now - 5 * HOUR + 7 * 60_000,
        durationMs: 7 * 60_000,
        summary: { title: "Nightly checks passed", text: "All 212 tests passed." },
        nodes: [
          { nodeId: "test", branch: "", status: "done", title: "Run tests", type: "command", output: { exitCode: 0, stdout: "212 passed" }, startedAt: now - 5 * HOUR, endedAt: now - 5 * HOUR + 60_000 },
          { nodeId: "triage", branch: "", status: "done", title: "Summarize", type: "session", output: { reply: "Nothing to fix." }, sessions: sessions.slice(0, 1) },
        ],
        sessions: sessions.slice(0, 1),
      },
      {
        id: "run-nightly-1",
        status: "doneWithFailures",
        caller: { kind: "manual", triggerNode: "manual" },
        startedAt: now - 29 * HOUR,
        endedAt: now - 29 * HOUR + 3 * 60_000,
        durationMs: 3 * 60_000,
        summary: { title: "Nightly checks finished with failures", text: "2 tests failed in the parser." },
        nodes: [
          { nodeId: "test", branch: "", status: "failed", title: "Run tests", type: "command", error: "Command exited with code 1", output: { exitCode: 1 } },
          { nodeId: "triage", branch: "", status: "skipped", title: "Summarize", type: "session" },
        ],
        sessions: [],
      },
    ],
  },
  {
    summary: {
      id: "wf-review",
      name: "Review new pull requests",
      enabled: false,
      triggers: [{ id: "pr", type: "event", label: "A pull request is opened" }],
      nextRunAt: null,
      lastRun: { status: "waitingForUser", startedAt: 0 },
    },
    runs: (now, sessions) => [
      {
        id: "run-review-1",
        // A status a later desktop may send: shown as plain text.
        status: "waitingForUser",
        caller: { kind: "mono", monoId: "mono-1", triggerNode: "pr" },
        startedAt: now - 2 * HOUR,
        nodes: [{ nodeId: "ask", branch: "", status: "waitingForUser", title: "Ask the owner", type: "session", sessions: sessions.slice(1, 2) }],
        sessions: sessions.slice(1, 2),
      },
    ],
  },
];

/** The list, with times relative to `now`. */
export function workflowsList(now: number): ResultOf<"workflows.list"> {
  return {
    workflows: WORKFLOWS.map(({ summary }, index) => ({
      ...summary,
      nextRunAt: summary.enabled ? now + 3 * HOUR : null,
      lastRun: summary.lastRun ? { status: summary.lastRun.status, startedAt: now - (index === 0 ? 5 : 2) * HOUR } : null,
    })),
  };
}

/** The runs of one workflow, linking the given session ids. An unknown id answers like a hidden workflow. */
export function workflowRuns(workflowId: string, now: number, sessionIds: string[]): ResultOf<"workflows.runs"> {
  const found = WORKFLOWS.find((workflow) => workflow.summary.id === workflowId);
  if (!found) return { workflow: null, runs: [] };
  return { workflow: { id: found.summary.id, name: found.summary.name }, runs: found.runs(now, sessionIds) };
}
