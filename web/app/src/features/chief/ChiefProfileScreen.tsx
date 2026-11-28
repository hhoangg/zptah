// The Chief's profile at `/d/:desktopId/monos/chief/profile`. Who it is, what runs it, whether Send works from the web
// and how many commands wait. A Control device edits exactly its name, mascot, emoji, color, model, effort and the
// delivery notification (`chief.update`); a read-only device sees the same details without controls. Whether the Chief is
// on, what runs it, its account, "Send from the web" and its soul are text only here: a remote device must not widen its
// own power, so those change in the desktop app. A device narrowed to some projects or Monos never sees the Chief.
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { ChiefProfileResult, ParamsOf } from "@monoptah/protocol";
import { catalogKey, useLinkMutation, useLinkQuery } from "@/data";
import { MonoAvatar } from "@/features/monos/MonoAvatar";
import { MONO_MASCOT_NAMES, MONO_NAME_MAX, MONO_SWATCHES, MONO_SWATCH_NAMES } from "@/features/monos/monoLook";
import { MonoMascot } from "@/features/monos/MonoMascot";
import { BACK_LINK_CLASS, BackIcon, Group, MonoPage, RefreshButton } from "@/features/monos/MonoPage";
import { RadioGroup, radioTabIndex } from "@/features/monos/RadioGroup";
import { useReadOnly } from "@/features/sessions/WorkspaceFrame";
import { cn } from "@/lib/utils";
import { Button } from "@/ui/button";
import { Checkbox } from "@/ui/Checkbox";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Input } from "@/ui/input";
import { Notice } from "@/ui/Notice";
import { CHIEF_ID, useChiefCatalog } from "./chief";

const STATUS_TEXT: Record<string, string> = { idle: "Idle", working: "Working", "needs-you": "Needs you" };

function Line({ label, value, testId }: { label: string; value: string; testId?: string }) {
  return (
    <div className="flex items-baseline justify-between gap-3 py-1.5">
      <dt className="text-small text-muted-text">{label}</dt>
      <dd className="min-w-0 text-right text-body wrap-anywhere" data-testid={testId}>
        {value}
      </dd>
    </div>
  );
}

type Update = ParamsOf<"chief.update">;

/** The edit fields start from the profile; the form remounts (by this key) when the desktop's answer changes. */
const formKey = (profile: ChiefProfileResult): string =>
  JSON.stringify([profile.name, profile.mascot, profile.emoji ?? null, profile.color, profile.harness.model ?? null, profile.harness.effort ?? null, profile.notifyOnDelivery]);

/** Only what changed, as a `chief.update` body; empty when nothing did. `null` resets emoji, model and effort. */
export function chiefChanges(
  profile: ChiefProfileResult,
  draft: { name: string; mascot: string; emoji: string; color: string; model: string; effort: string; notify: boolean },
): Update {
  const change: Update = {};
  const name = draft.name.trim();
  if (name !== "" && name !== profile.name) change.name = name;
  if (draft.mascot !== profile.mascot) change.mascot = draft.mascot;
  const emoji = draft.emoji.trim();
  if (emoji !== (profile.emoji ?? "")) change.emoji = emoji === "" ? null : emoji;
  if (draft.color.toLowerCase() !== profile.color.toLowerCase()) change.color = draft.color;
  const model = draft.model.trim();
  if (model !== (profile.harness.model ?? "")) change.model = model === "" ? null : model;
  const effort = draft.effort.trim();
  if (effort !== (profile.harness.effort ?? "")) change.effort = effort === "" ? null : effort;
  if (draft.notify !== profile.notifyOnDelivery) change.notifyOnDelivery = draft.notify;
  return change;
}

function EditForm({ desktopId, profile, onSaved }: { desktopId: string; profile: ChiefProfileResult; onSaved: () => void }) {
  const queryClient = useQueryClient();
  const update = useLinkMutation("chief.update");
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState({
    name: profile.name,
    mascot: profile.mascot,
    emoji: profile.emoji ?? "",
    color: profile.color,
    model: profile.harness.model ?? "",
    effort: profile.harness.effort ?? "",
    notify: profile.notifyOnDelivery,
  });
  const change = chiefChanges(profile, draft);
  const dirty = Object.keys(change).length > 0;
  const set = (patch: Partial<typeof draft>) => setDraft((current) => ({ ...current, ...patch }));
  const color = draft.color.toLowerCase();
  const save = () => {
    setError(null);
    update.mutate(change, {
      onSuccess: () => {
        void queryClient.invalidateQueries({ queryKey: catalogKey(desktopId) });
        onSaved();
      },
      onError: (failure) => setError(failure.message || "Could not save."),
    });
  };

  return (
    <div data-testid="chief-edit" className="flex flex-col gap-4">
      <Group title="Name">
        <Input aria-label="Name" value={draft.name} maxLength={MONO_NAME_MAX} placeholder="Name" onChange={(event) => set({ name: event.target.value })} className="border-0 bg-transparent" />
      </Group>
      <Group title="Mascot">
        <RadioGroup aria-label="Mascot" className="grid grid-cols-5 gap-2 p-1.5">
          {MONO_MASCOT_NAMES.map((option, index) => (
            <button
              key={option}
              type="button"
              role="radio"
              aria-checked={option === draft.mascot}
              aria-label={option}
              tabIndex={radioTabIndex(option === draft.mascot, index, MONO_MASCOT_NAMES.includes(draft.mascot))}
              onClick={() => set({ mascot: option })}
              className={cn("grid aspect-square place-items-center rounded-card border-2 border-transparent hover:bg-selection", option === draft.mascot && "border-[var(--accent-color)]")}
            >
              <MonoMascot monoId={CHIEF_ID} mascot={option} color={draft.color} size={40} />
            </button>
          ))}
        </RadioGroup>
      </Group>
      <Group title="Emoji">
        <div className="flex items-center gap-2 px-1 py-1">
          <Input aria-label="Emoji" value={draft.emoji} maxLength={16} placeholder="Uses the mascot's emoji" onChange={(event) => set({ emoji: event.target.value })} className="border-0 bg-transparent" />
          {draft.emoji !== "" ? (
            <Button type="button" size="sm" variant="ghost" onClick={() => set({ emoji: "" })}>
              Use mascot emoji
            </Button>
          ) : null}
        </div>
      </Group>
      <Group title="Color">
        <RadioGroup aria-label="Color" className="flex flex-wrap gap-3 p-2.5">
          {MONO_SWATCHES.map((swatch, index) => {
            const selected = swatch === color;
            return (
              <button
                key={swatch}
                type="button"
                role="radio"
                aria-checked={selected}
                aria-label={MONO_SWATCH_NAMES[swatch]}
                title={MONO_SWATCH_NAMES[swatch]}
                tabIndex={radioTabIndex(selected, index, (MONO_SWATCHES as readonly string[]).includes(color))}
                onClick={() => set({ color: swatch })}
                className="grid size-11 place-items-center rounded-pill border-2 border-transparent pointer-fine:size-9"
                style={selected ? { borderColor: swatch } : undefined}
              >
                <span className="grid size-7 place-items-center rounded-pill text-inverse" style={{ background: swatch }}>
                  {selected ? <Icon name="check" size={14} /> : null}
                </span>
              </button>
            );
          })}
        </RadioGroup>
      </Group>
      <Group title="Model">
        <div className="flex flex-col gap-1 px-1 py-1">
          <Input aria-label="Model" value={draft.model} maxLength={256} placeholder="App default" onChange={(event) => set({ model: event.target.value })} className="border-0 bg-transparent" />
          <Input aria-label="Effort" value={draft.effort} maxLength={64} placeholder="Effort: app default" onChange={(event) => set({ effort: event.target.value })} className="border-0 bg-transparent" />
        </div>
      </Group>
      <Group title="Notifications">
        <Checkbox label="Notify me when a command is delivered" checked={draft.notify} onChange={(event) => set({ notify: event.target.checked })} />
      </Group>
      {error ? <Notice variant="error">{error}</Notice> : null}
      <div>
        <Button type="button" data-testid="chief-save" disabled={!dirty || update.isPending || draft.name.trim() === ""} onClick={save}>
          Save changes
        </Button>
      </div>
    </div>
  );
}

export function ChiefProfileScreen({ desktopId }: { desktopId: string }) {
  const query = useLinkQuery("chief.profile", {});
  const profile = query.data;
  const readOnly = useReadOnly();
  // A device narrowed to some projects or Monos has no Chief in its catalog: it never gets the controls.
  const visible = useChiefCatalog() !== undefined;
  const editable = !readOnly && visible && profile?.enabled === true;
  const text = (key: string): string | undefined => {
    const value = profile ? (profile as Record<string, unknown>)[key] : undefined;
    return typeof value === "string" && value !== "" ? value : undefined;
  };
  return (
    <MonoPage
      testId="chief-profile"
      title="Chief"
      back={
        <Link to="/d/$desktopId/monos/chief" params={{ desktopId }} aria-label="Back" className={BACK_LINK_CLASS}>
          <BackIcon />
        </Link>
      }
      actions={<RefreshButton onRefresh={() => void query.refetch()} refreshing={query.isFetching && !query.isPending} />}
    >
      {profile ? (
        <>
          <section aria-label="Identity" className="flex flex-col items-center gap-2 py-2 text-center">
            <MonoAvatar monoId={CHIEF_ID} name={profile.name} mascot={profile.mascot} emoji={profile.emoji} color={profile.color} working={profile.status === "working"} size={88} />
            <h2 className="max-w-full truncate text-headline font-semibold">{profile.name}</h2>
            <p className="text-small text-muted-text">{STATUS_TEXT[profile.status] ?? profile.status}</p>
          </section>
          {profile.enabled ? null : <Notice variant="info">The Chief is off on the desktop.</Notice>}
          <Group title="Details">
            <dl>
              <Line label="Enabled" value={profile.enabled ? "On" : "Off"} />
              <Line label="Runs on" value={profile.harness.line} testId="chief-harness" />
              {text("account") ? <Line label="Account" value={text("account") ?? ""} /> : null}
              {profile.harness.ok ? null : <Line label="Status" value="Not ready" />}
              <Line label="Send from the web" value={profile.allowWebSend ? "On" : "Off"} testId="chief-allow-web-send" />
              <Line label="Notify on delivery" value={profile.notifyOnDelivery ? "On" : "Off"} />
              <Line label="Open commands" value={String(profile.openCount)} testId="chief-open-count" />
              {text("soul") ? <Line label="Soul" value={text("soul") ?? ""} testId="chief-soul" /> : null}
            </dl>
          </Group>
          {editable ? <EditForm key={formKey(profile)} desktopId={desktopId} profile={profile} onSaved={() => void query.refetch()} /> : null}
          <p className="px-1 text-small text-muted-text" data-testid="chief-settings-note">
            {editable ? "Whether the Chief is on, what runs it, its account, Send from the web and its soul change in the desktop app." : "Change the Chief's settings in the desktop app."}
          </p>
        </>
      ) : query.isError ? (
        <Notice variant="error">{query.error.message || "Could not load the Chief."}</Notice>
      ) : (
        <DelayedSpinner label="Loading the Chief" className="m-6" />
      )}
    </MonoPage>
  );
}
