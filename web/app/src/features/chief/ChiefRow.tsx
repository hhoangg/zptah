// The Chief's row, pinned first in the Mono home list and in the sidebar's Monos list: the avatar, the name with a small
// "Chief" tag, and a dot while commands wait for the owner's Send. It opens the Chief chat at `/d/:desktopId/monos/chief`.
import { Link } from "@tanstack/react-router";
import type { ChiefCatalog } from "@monoptah/protocol";
import { MonoAvatar } from "@/features/monos/MonoAvatar";
import { cn } from "@/lib/utils";
import { CHIEF_ID, chiefName, waitingLabel } from "./chief";

function ChiefTag() {
  return (
    <span data-testid="chief-tag" className="shrink-0 rounded-pill border border-stroke px-1.5 text-micro font-medium text-muted-text">
      Chief
    </span>
  );
}

function OpenDot({ count }: { count: number }) {
  return count > 0 ? <span role="img" aria-label={waitingLabel(count)} data-testid="chief-dot" className="size-2.5 shrink-0 rounded-pill bg-[var(--accent-color)]" /> : null;
}

function detailOf(chief: ChiefCatalog): string {
  if (chief.openCount > 0) return waitingLabel(chief.openCount);
  return chief.status === "working" ? "Working..." : "Nothing waiting for you";
}

export function ChiefHomeRow({ desktopId, chief }: { desktopId: string; chief: ChiefCatalog }) {
  const name = chiefName(chief);
  return (
    <Link to="/d/$desktopId/monos/chief" params={{ desktopId }} data-testid="chief-row" className="flex min-h-16 items-center gap-3 rounded-card px-2 py-2 hover:bg-fill">
      <MonoAvatar monoId={CHIEF_ID} name={name} mascot={chief.mascot} emoji={chief.emoji} color={chief.color} working={chief.status === "working"} size={40} />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2">
          <span className="min-w-0 truncate text-row font-semibold">{name}</span>
          <ChiefTag />
        </span>
        <span className={cn("block truncate text-small text-muted-text", chief.openCount > 0 && "text-caution")}>{detailOf(chief)}</span>
      </span>
      <OpenDot count={chief.openCount} />
    </Link>
  );
}

export function ChiefNavRow({ desktopId, chief, active, onNavigate }: { desktopId: string; chief: ChiefCatalog; active: boolean; onNavigate?: () => void }) {
  const name = chiefName(chief);
  return (
    <Link
      to="/d/$desktopId/monos/chief"
      params={{ desktopId }}
      onClick={onNavigate}
      aria-current={active ? "page" : undefined}
      data-testid="chief-row"
      className={cn("flex min-h-16 items-center gap-3.5 rounded-[16px] px-2.5 py-2.5 hover:bg-content-a04", active && "bg-selection hover:bg-selection")}
    >
      <MonoAvatar monoId={CHIEF_ID} name={name} mascot={chief.mascot} emoji={chief.emoji} color={chief.color} working={chief.status === "working"} size={40} />
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex items-center gap-2">
          <span className="min-w-0 truncate text-row font-semibold" title={name}>{name}</span>
          <ChiefTag />
        </span>
        <span className="flex items-center gap-2">
          <span className={cn("min-w-0 flex-1 truncate text-label text-muted-text", chief.openCount > 0 && "text-caution")}>{detailOf(chief)}</span>
          <OpenDot count={chief.openCount} />
        </span>
      </span>
    </Link>
  );
}
