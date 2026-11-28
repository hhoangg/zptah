// The runs of one workflow: status, caller, start time, duration, the summary, the node list (status, output and
// error) and links to the run's sessions. Read-only. Statuses and fields this app does not know are shown as plain
// words and ignored, so a newer desktop never breaks the screen.
import { Link } from "@tanstack/react-router";
import type { ResultOf } from "@monoptah/protocol";
import { useLinkQuery } from "@/data";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { EmptyState } from "@/ui/EmptyState";
import { Notice } from "@/ui/Notice";
import { callerLabel, durationLabel, outputText, safeText, timeLabel } from "./copy";
import { StatusBadge } from "./StatusBadge";

type Run = ResultOf<"workflows.runs">["runs"][number];
type NodeRun = Run["nodes"][number];

export function WorkflowRunsScreen({ desktopId, workflowId }: { desktopId: string; workflowId: string }) {
  const query = useLinkQuery("workflows.runs", { workflowId });
  if (query.data === undefined) {
    if (query.isError) {
      return (
        <div className="flex flex-col items-start gap-3 py-2">
          <Notice variant="error" className="w-full">
            Could not load the runs.
          </Notice>
          <Button type="button" variant="outline" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      );
    }
    return (
      <div className="flex min-h-13 items-center justify-center py-2">
        <DelayedSpinner label="Loading runs" />
      </div>
    );
  }
  const { workflow, runs } = query.data;
  if (workflow === null) {
    return <EmptyState icon="branch" data-testid="workflow-missing" title="Workflow not found" description="It was removed, or this device may not see it." />;
  }
  if (runs.length === 0) {
    return <EmptyState icon="branch" data-testid="runs-empty" title="No runs yet" description={`${safeText(workflow.name, 80)} has not run.`} />;
  }
  return (
    <ul aria-label="Runs" className="flex flex-col gap-3">
      {runs.map((run) => (
        <RunRow key={run.id} desktopId={desktopId} run={run} />
      ))}
    </ul>
  );
}

function RunRow({ desktopId, run }: { desktopId: string; run: Run }) {
  const duration = durationLabel(run.durationMs);
  const summary = run.summary;
  return (
    <li data-testid="workflow-run" data-run={run.id} data-status={run.status} className="flex flex-col gap-3 rounded-card bg-surface p-4">
      <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-2">
        <div className="min-w-0">
          <h2 className="wrap-anywhere text-body font-semibold text-content">{timeLabel(run.startedAt)}</h2>
          <p className="text-meta text-muted-text">
            {callerLabel(run.caller.kind)}
            {duration ? ` - ${duration}` : ""}
          </p>
        </div>
        <StatusBadge status={run.status} />
      </div>
      {summary ? (
        <div className="text-small">
          <p className="wrap-anywhere font-medium text-content">{safeText(summary.title, 120)}</p>
          <p className="wrap-anywhere whitespace-pre-wrap text-muted-text">{outputText(summary.text)}</p>
        </div>
      ) : null}
      {run.error ? (
        <Notice variant="error" className="wrap-anywhere">
          {safeText(run.error, 300)}
        </Notice>
      ) : null}
      {run.outputsDropped ? <p className="text-small text-muted-text">Outputs left out (too large)</p> : null}
      {run.nodesTruncated ? <p className="text-small text-muted-text">Later steps left out (too large)</p> : null}
      <div>
        <h3 className="text-label font-medium text-muted-text">Steps</h3>
        {run.nodes.length === 0 ? (
          <p className="text-small text-muted-text">No steps ran.</p>
        ) : (
          <ul className="mt-1 flex flex-col divide-y divide-stroke">
            {run.nodes.map((node, index) => (
              <NodeRow key={`${node.nodeId}/${node.branch ?? ""}/${index}`} desktopId={desktopId} node={node} />
            ))}
          </ul>
        )}
      </div>
    </li>
  );
}

function NodeRow({ desktopId, node }: { desktopId: string; node: NodeRun }) {
  const output = outputText(node.output);
  const name = safeText(node.title, 80) || safeText(node.nodeId, 40);
  return (
    <li data-testid="workflow-node" data-node={node.nodeId} className="flex flex-col gap-1 py-2">
      <div className="flex items-center justify-between gap-2">
        <span className="min-w-0 wrap-anywhere text-small font-medium text-content">
          {name}
          {node.branch ? <span className="text-muted-text"> ({safeText(node.branch, 30)})</span> : null}
        </span>
        <StatusBadge status={node.status} />
      </div>
      {node.error ? <p className="wrap-anywhere text-small text-destructive-text">{safeText(node.error, 300)}</p> : null}
      {output ? (
        <details className="text-small">
          <summary className="min-h-8 cursor-pointer text-brand-text">Output</summary>
          <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap rounded-row bg-fill p-2 text-meta wrap-anywhere">{output}</pre>
        </details>
      ) : null}
      {node.sessions && node.sessions.length > 0 ? (
        <ul className="flex flex-wrap gap-x-3">
          {node.sessions.map((sessionId, index) => (
            <li key={sessionId}>
              <Link to="/d/$desktopId/s/$sessionId" params={{ desktopId, sessionId }} className="inline-flex min-h-8 items-center text-small text-brand-text hover:underline">
                {node.sessions && node.sessions.length > 1 ? `Open session ${index + 1}` : "Open session"}
              </Link>
            </li>
          ))}
        </ul>
      ) : null}
    </li>
  );
}
