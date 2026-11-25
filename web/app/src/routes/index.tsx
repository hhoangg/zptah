// `/`: restores the last place of the last used desktop (N-12), else goes to /settings/desktops (-restoreLastPlace.ts).
// On the official host a signed-out browser never reaches this route by a page load: the Worker serves the landing
// page at `/` when there is no session cookie. Owner: WS-L (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { restoreLastPlace } from "./-restoreLastPlace";

export const Route = createFileRoute("/")({
  beforeLoad: restoreLastPlace,
});
