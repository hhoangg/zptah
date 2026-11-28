// The Workflows entry of the workspace drawer, only when the desktop advertises `workflows`.
import { Link } from "@tanstack/react-router";
import { Icon } from "@/ui/Icon";
import { useCapability } from "@/version";
import { WORKFLOWS_NAV_LABEL } from "./copy";

export function WorkflowsNavItem({ desktopId, onNavigate }: { desktopId: string; onNavigate?: () => void }) {
  const enabled = useCapability("workflows");
  if (!enabled) return null;
  return (
    <Link to="/d/$desktopId/workflows" params={{ desktopId }} onClick={onNavigate} className="flex min-h-11 items-center gap-3 rounded-row px-2.5 text-row hover:bg-content-a04 [&.active]:bg-selection">
      <Icon name="branch" size={20} />
      {WORKFLOWS_NAV_LABEL}
    </Link>
  );
}
