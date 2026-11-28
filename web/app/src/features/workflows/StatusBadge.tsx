import { Badge } from "@/ui/badge";
import { statusLabel } from "./copy";

const DESTRUCTIVE = new Set(["failed"]);
const BRAND = new Set(["running", "waitingApproval", "waitingForUser"]);

/** The status as text, so colour is never the only signal. A status this app does not know shows as plain words. */
export function StatusBadge({ status, scope }: { status: string; scope: string }) {
  const variant = DESTRUCTIVE.has(status) ? "destructive" : BRAND.has(status) ? "brand" : "outline";
  return (
    <Badge variant={variant} role="status" aria-label={`${scope}: ${statusLabel(status)}`} className="min-h-6 px-2" data-status={status}>
      {statusLabel(status)}
    </Badge>
  );
}
