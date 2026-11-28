// `/d/:desktopId/monos/chief`: the Chief chat. A static segment: it wins over `$monoId`, and a real Mono id is generated, so it never equals `chief`.
import { createFileRoute } from "@tanstack/react-router";
import { MonoScreen } from "@/features/monos/MonoScreen";

export const Route = createFileRoute("/d/$desktopId/monos/chief/")({
  component: Page,
});

function Page() {
  const { desktopId } = Route.useParams();
  return <MonoScreen key={`${desktopId}/chief`} desktopId={desktopId} monoId="chief" chief />;
}
