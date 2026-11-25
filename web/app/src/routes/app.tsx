// `/app`: the installed app's start_url (public/manifest.webmanifest). Same as `/` (the last place of the last used
// desktop), but the Worker always serves the SPA here, so a signed-out launch reaches the auth guard and `/login`
// instead of the landing page that `/` shows without a session cookie.
import { createFileRoute } from "@tanstack/react-router";
import { restoreLastPlace } from "./-restoreLastPlace";

export const Route = createFileRoute("/app")({
  beforeLoad: restoreLastPlace,
});
