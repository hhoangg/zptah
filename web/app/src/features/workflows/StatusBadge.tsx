import { Badge } from "@/ui/badge";
import { statusLabel } from "./copy";

const DESTRUCTIVE = new Set(["failed"]);
const BRAND = new Set(["running", "waitingApproval", "waitingForUser"]);

/** The status as text, so colour is never the only signal. Not a live region: a list of badges would announce every one.  A status this app does not know shows as plain words. */
export function StatusBadge({ status }: { status: string }) {
  const variant = DESTRUCTIVE.has(status) ? "destructive" : BRAND.has(status) ? "brand" : "outline";
  return (
    <Badge variant={variant} className="min-h-6 px-2" data-status={status}>
      {statusLabel(status)}
    </Badge>
  );
}
