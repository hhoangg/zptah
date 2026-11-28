// The Workflows list of one desktop: name, triggers, enabled, next run and the status of the last run. Read-only:
// no canvas, no editing, no run button.
import { Link } from "@tanstack/react-router";
import { useLinkQuery } from "@/data";
import { Badge } from "@/ui/badge";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { EmptyState } from "@/ui/EmptyState";
import { Notice } from "@/ui/Notice";
import { nextRunLabel, safeText, timeLabel } from "./copy";
import { StatusBadge } from "./StatusBadge";

export function WorkflowsScreen({ desktopId }: { desktopId: string }) {
  const query = useLinkQuery("workflows.list", {});
  if (query.data === undefined) {
    if (query.isError) {
      return (
        <div className="flex flex-col items-start gap-3 py-2">
          <Notice variant="error" className="w-full">
            Could not load the workflows.
          </Notice>
          <Button type="button" variant="outline" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      );
    }
    return (
      <div className="flex min-h-13 items-center justify-center py-2">
        <DelayedSpinner label="Loading workflows" />
      </div>
    );
  }
  const { workflows } = query.data;
  if (workflows.length === 0) {
    return <EmptyState icon="branch" data-testid="workflows-empty" title="No workflows to show" description="Workflows you can see on this desktop appear here. Make and edit them in zptah on the desktop." />;
  }
  return (
    <div className="flex flex-col gap-4">
      {query.isError ? <Notice variant="error">Could not refresh the list. Showing the last result.</Notice> : null}
      <ul aria-label="Workflows" className="flex flex-col gap-3">
        {workflows.map((workflow) => (
          <li key={workflow.id} data-testid="workflow-row" data-workflow={workflow.id} className="rounded-card bg-surface">
            <Link to="/d/$desktopId/workflows/$workflowId" params={{ desktopId, workflowId: workflow.id }} className="flex flex-col gap-2 rounded-card p-4 hover:bg-content-a04">
              <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-2">
                <h2 className="min-w-0 wrap-anywhere text-body font-semibold text-content">{safeText(workflow.name, 80) || "Unnamed workflow"}</h2>
                {workflow.lastRun ? <StatusBadge status={workflow.lastRun.status} /> : workflow.lastRunUnreadable ? <Badge variant="destructive">Run history unreadable</Badge> : <Badge variant="outline">Never run</Badge>}
              </div>
              {workflow.description ? <p className="wrap-anywhere text-small text-muted-text">{safeText(workflow.description, 200)}</p> : null}
              <p className="text-small text-muted-text">{workflow.triggers.length === 0 ? "No trigger" : workflow.triggers.map((trigger) => safeText(trigger.label, 60)).join(", ")}</p>
              <p className="text-small text-muted-text">
                {workflow.enabled ? "On" : "Off"} - {nextRunLabel(workflow.nextRunAt, workflow.enabled)}
                {workflow.lastRun ? ` - Last run ${timeLabel(workflow.lastRun.startedAt)}` : ""}
              </p>
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}
