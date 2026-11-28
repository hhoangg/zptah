// The runs of one workflow /d/:desktopId/workflows/:workflowId: read-only.
import { createFileRoute } from "@tanstack/react-router";
import { WorkflowRunsScreen } from "@/features/workflows/WorkflowRunsScreen";
import { WorkspaceFrame } from "@/features/sessions/WorkspaceFrame";
import { RequireCapability } from "@/version/RequireCapability";

export const Route = createFileRoute("/d/$desktopId/workflows/$workflowId")({
  component: Page,
});

function Page() {
  const { desktopId, workflowId } = Route.useParams();
  return (
    <WorkspaceFrame desktopId={desktopId} title="Workflow runs" testId="workflow-runs-screen" narrow back={{ to: "/d/$desktopId/workflows", params: { desktopId }, label: "Back to workflows" }}>
      <RequireCapability name="workflows" fallback="notice">
        <WorkflowRunsScreen key={`${desktopId}/${workflowId}`} desktopId={desktopId} workflowId={workflowId} />
      </RequireCapability>
    </WorkspaceFrame>
  );
}
