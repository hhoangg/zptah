// `/d/:desktopId/monos/chief/profile`: the Chief's read-only profile.
import { createFileRoute } from "@tanstack/react-router";
import { ChiefProfileScreen } from "@/features/chief/ChiefProfileScreen";

export const Route = createFileRoute("/d/$desktopId/monos/chief/profile")({
  component: Page,
});

function Page() {
  const { desktopId } = Route.useParams();
  return <ChiefProfileScreen key={desktopId} desktopId={desktopId} />;
}
