// Workflows list /d/:desktopId/workflows: read-only (no canvas, no editing).
import { createFileRoute } from "@tanstack/react-router";
import { WorkflowsScreen } from "@/features/workflows/WorkflowsScreen";
import { WorkspaceFrame } from "@/features/sessions/WorkspaceFrame";
import { RequireCapability } from "@/version/RequireCapability";

export const Route = createFileRoute("/d/$desktopId/workflows/")({
  component: Page,
});

function Page() {
  const { desktopId } = Route.useParams();
  return (
    <WorkspaceFrame desktopId={desktopId} title="Workflows" subtitle="What runs on this desktop, and how it went" testId="workflows-screen" narrow>
      <RequireCapability name="workflows" fallback="notice">
        <WorkflowsScreen key={desktopId} desktopId={desktopId} />
      </RequireCapability>
    </WorkspaceFrame>
  );
}
