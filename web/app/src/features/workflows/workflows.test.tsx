// The read-only Workflows screens over the mock desktop: the list, the runs of one workflow, tolerance of statuses
// and fields this app does not know, the hidden-workflow answer and the capability gate.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_HANDLERS, FixtureError } from "@monoptah/mock-desktop";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
import { durationLabel, outputText, statusLabel } from "./copy";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({
    matches: query.includes("min-width: 640px") && query.includes("pointer: fine"),
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as never;
  stubTranscriptLayout();
  window.scrollTo = vi.fn() as never;
});

const original = { ...FIXTURE_HANDLERS };
let previous: PersistAdapter;

beforeEach(() => {
  localStorage.clear();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  Object.assign(FIXTURE_HANDLERS, original);
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  localStorage.clear();
});

function mount(path = "workflows", options: MockLinkOptions = {}) {
  connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
      options.seed?.(desktop);
    },
  });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}/${path}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

const rowOf = async (name: string) => {
  const heading = await screen.findByRole("heading", { name });
  return within(heading.closest("li") as HTMLElement);
};

describe("the list", () => {
  it("shows name, triggers, on or off, next run and the status of the last run", async () => {
    mount();
    const nightly = await rowOf("Nightly checks");
    expect(nightly.getByText(/Daily at 02:00, Manual/)).toBeInTheDocument();
    expect(nightly.getByText(/On - Next run/)).toBeInTheDocument();
    expect(nightly.getByText("Done")).toBeInTheDocument();
    expect(nightly.getByRole("link")).toHaveAttribute("href", `/d/${DESKTOP_ID}/workflows/wf-nightly`);

    const review = await rowOf("Review new pull requests");
    expect(review.getByText(/Off - Paused/)).toBeInTheDocument();
    // A status this app does not know shows as plain words.
    expect(review.getByText("Waiting for you")).toBeInTheDocument();
  });

  it("shows the empty state", async () => {
    FIXTURE_HANDLERS["workflows.list"] = (() => ({ workflows: [] })) as never;
    mount();
    expect(await screen.findByTestId("workflows-empty")).toHaveTextContent("No workflows to show");
  });

  it("shows an error with a retry", async () => {
    const user = userEvent.setup();
    FIXTURE_HANDLERS["workflows.list"] = (() => {
      throw new FixtureError("boom");
    }) as never;
    mount();
    expect(await screen.findByText("Could not load the workflows.")).toBeInTheDocument();
    FIXTURE_HANDLERS["workflows.list"] = (() => ({ workflows: [] })) as never;
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByTestId("workflows-empty")).toBeInTheDocument();
  });

  it("shows the update notice, and no data, when the desktop has no workflows capability", async () => {
    mount("workflows", { capabilities: ["cloud"] });
    expect(await screen.findByTestId("capability-notice")).toHaveTextContent("Update zptah on your desktop to use Workflows");
    expect(screen.queryByTestId("workflow-row")).toBeNull();
  });

  it("says when the run history could not be read instead of saying never run", async () => {
    FIXTURE_HANDLERS["workflows.list"] = (() => ({
      workflows: [
        { id: "a", name: "Broken history", enabled: true, triggers: [], lastRunUnreadable: true },
        { id: "b", name: "Fresh", enabled: true, triggers: [] },
      ],
    })) as never;
    mount();
    expect((await rowOf("Broken history")).getByText("Run history unreadable")).toBeInTheDocument();
    expect((await rowOf("Broken history")).queryByText("Never run")).toBeNull();
    expect((await rowOf("Fresh")).getByText("Never run")).toBeInTheDocument();
  });

  it("does not make every status badge a live region", async () => {
    mount();
    await rowOf("Nightly checks");
    expect(document.querySelectorAll('[role="status"]')).toHaveLength(0);
    expect(screen.getAllByText("Done").length).toBeGreaterThan(0);
  });

  it("tolerates fields it does not know", async () => {
    FIXTURE_HANDLERS["workflows.list"] = (() => ({
      workflows: [{ id: "w", name: "Odd", enabled: true, triggers: [{ id: "t", type: "later", label: "Something new", extra: 1 }], futureField: { a: 1 }, lastRun: { status: "queuedSomewhere", startedAt: 1 } }],
      more: true,
    })) as never;
    mount();
    const row = await rowOf("Odd");
    expect(row.getByText("Queued somewhere")).toBeInTheDocument();
    expect(row.getByText(/Something new/)).toBeInTheDocument();
  });
});

describe("the runs of a workflow", () => {
  it("shows status, caller, duration, summary, steps with output and error, and session links", async () => {
    mount("workflows/wf-nightly");
    const runs = await screen.findAllByTestId("workflow-run");
    expect(runs).toHaveLength(2);
    const first = within(runs[0]!);
    expect(first.getByText(/Schedule - 7 min 0 s/)).toBeInTheDocument();
    expect(first.getByText("Nightly checks passed")).toBeInTheDocument();
    expect(first.getByText("All 212 tests passed.")).toBeInTheDocument();
    const steps = first.getAllByTestId("workflow-node");
    expect(within(steps[0]!).getByText("Run tests")).toBeInTheDocument();
    expect(within(steps[0]!).getByText(/212 passed/)).toBeInTheDocument();
    const links = within(steps[1]!).getAllByRole("link", { name: "Open session" });
    expect(links).toHaveLength(1);
    expect(links[0]!.getAttribute("href")).toMatch(new RegExp(`^/d/${DESKTOP_ID}/s/.+`));

    const second = within(runs[1]!);
    expect(second.getByText("Done with failures")).toBeInTheDocument();
    expect(second.getByText("Command exited with code 1")).toBeInTheDocument();
    expect(second.getByText("Skipped")).toBeInTheDocument();
  });

  it("shows a status and a node status it does not know as words", async () => {
    mount("workflows/wf-review");
    const run = within(await screen.findByTestId("workflow-run"));
    expect(run.getAllByText("Waiting for you")).toHaveLength(2);
    expect(run.getByText("A Mono", { exact: false })).toBeInTheDocument();
  });

  it("says so on a run whose outputs or last steps the desktop left out", async () => {
    FIXTURE_HANDLERS["workflows.runs"] = (() => ({
      workflow: { id: "w", name: "X" },
      runs: [
        { id: "r1", status: "done", caller: { kind: "manual" }, startedAt: 2, outputsDropped: true, nodesTruncated: true, nodes: [{ nodeId: "n", status: "done" }] },
        { id: "r2", status: "done", caller: { kind: "manual" }, startedAt: 1, nodes: [{ nodeId: "n", status: "done", output: { a: 1 } }] },
      ],
    })) as never;
    mount("workflows/w");
    const runs = await screen.findAllByTestId("workflow-run");
    expect(within(runs[0]!).getByText("Outputs left out (too large)")).toBeInTheDocument();
    expect(within(runs[0]!).getByText("Later steps left out (too large)")).toBeInTheDocument();
    expect(within(runs[1]!).queryByText(/left out/)).toBeNull();
  });

  it("answers like a missing workflow when the desktop hides it", async () => {
    mount("workflows/wf-hidden");
    expect(await screen.findByTestId("workflow-missing")).toHaveTextContent("Workflow not found");
  });

  it("shows the empty state of a workflow that never ran", async () => {
    FIXTURE_HANDLERS["workflows.runs"] = (() => ({ workflow: { id: "w", name: "Fresh" }, runs: [] })) as never;
    mount("workflows/w");
    expect(await screen.findByTestId("runs-empty")).toHaveTextContent("No runs yet");
  });

  it("renders text from the desktop as plain text, never as markup", async () => {
    FIXTURE_HANDLERS["workflows.runs"] = (() => ({
      workflow: { id: "w", name: "X" },
      runs: [
        {
          id: "r",
          status: "done",
          caller: { kind: "manual" },
          startedAt: 1,
          summary: { title: "<img src=x onerror=alert(1)>", text: "<b>bold</b>" },
          nodes: [{ nodeId: "n", status: "done", output: { html: "<script>alert(1)</script>" } }],
        },
      ],
    })) as never;
    mount("workflows/w");
    const run = await screen.findByTestId("workflow-run");
    expect(run.querySelector("img, script, b")).toBeNull();
    expect(run).toHaveTextContent("<img src=x onerror=alert(1)>");
  });
});

describe("formats", () => {
  it("labels statuses, known or not", () => {
    expect(statusLabel("doneWithFailures")).toBe("Done with failures");
    expect(statusLabel("waitingForUser")).toBe("Waiting for you");
    expect(statusLabel("someNewState")).toBe("Some new state");
  });
  it("formats durations", () => {
    expect(durationLabel(45_000)).toBe("45 s");
    expect(durationLabel(192_000)).toBe("3 min 12 s");
    expect(durationLabel(7_500_000)).toBe("2 h 5 min");
    expect(durationLabel(undefined)).toBe("");
  });
  it("caps and cleans output text", () => {
    expect(outputText("a‮b")).toBe("ab");
    expect(outputText({ a: 1 })).toContain('"a": 1');
    expect([...outputText("x".repeat(5_000))]).toHaveLength(2_000);
    expect(outputText(undefined)).toBe("");
  });
});
