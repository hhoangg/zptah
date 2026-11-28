// The Chief (web half): the pinned row, the proposal cards of its chat, the read-only profile and the relay label of a
// Mono chat, against the real link wired to the mock desktop (sessions/dev/mockLink.ts).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ChiefProposal } from "@monoptah/protocol";
import { CHIEF_FIXTURE_DIRECT_ID, CHIEF_FIXTURE_VIA_MONO_ID } from "@monoptah/mock-desktop";
import { IDBFactory } from "fake-indexeddb";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import type { MockDesktop } from "@monoptah/mock-desktop";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLink, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { resetMonoSeen } from "@/features/sessions/monoSeen";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
import { reloadMonoPrefs } from "@/features/monos/monoPrefs";
import { reloadMonoAccents } from "@/features/monos/monoAccent";
import "@/features/monos/register";
import { routeLine, stateText } from "./chief";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  stubTranscriptLayout();
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

let previous: PersistAdapter;
let link: MockLink;
beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  localStorage.clear();
  reloadMonoPrefs();
  reloadMonoAccents();
  resetMonoSeen();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  localStorage.clear();
});

function mount(path: string, options: MockLinkOptions = {}) {
  link = connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
      options.seed?.(desktop);
    },
  });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}${path}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, queryClient };
}

/** "Allow Send from the web" is off by default (as on the real desktop): tests that click Send turn it on. */
const WEB_SEND: MockLinkOptions = { seed: (desktop) => (desktop.world.chief.allowWebSend = true) };

/** A Chief turn whose `chief.propose` results returned these proposal ids (the turn's `chiefProposals`). */
const chiefTurn = (ids: string[]) => (desktop: MockDesktop) => {
  desktop.world.sessions.get("chief")!.turns.push({
    id: "chief-t9",
    stampAt: 1,
    completedAt: 2,
    chiefProposals: ids,
    items: [{ type: "text", id: "chief-a9", text: "Here is what I propose" }],
  });
};

const cardOf = async (id: string) => {
  const cards = await screen.findAllByTestId("chief-card");
  const card = cards.find((candidate) => candidate.textContent?.includes(id === CHIEF_FIXTURE_VIA_MONO_ID ? "Run the parser tests" : "Also run the lint step"));
  if (!card) throw new Error(`no card for ${id}`);
  return card;
};

const proposal = (patch: Partial<ChiefProposal>): ChiefProposal => ({
  id: "p_x",
  createdAt: 1,
  route: { type: "direct" },
  sessionTitle: "Fix login",
  project: "app",
  instruction: "Run it",
  notes: [],
  state: "awaitingOwner",
  open: true,
  kind: "command",
  ...patch,
});

describe("Chief text helpers", () => {
  it("draws the route line with the arrow and the middle dot", () => {
    expect(routeLine(proposal({ route: { type: "viaMono", monoId: "m1", monoName: "Ada" }, sessionNumber: 3, branch: "main" }))).toBe("Ada → #3 Fix login · app · main");
    expect(routeLine(proposal({}))).toBe("Directly → Fix login · app");
    expect(routeLine(proposal({ kind: "start", route: { type: "viaMono", monoId: "m1", monoName: "Ada" } }))).toBe("Ada → new session · app");
    expect(routeLine(proposal({ kind: "start", sessionTitle: "", branch: "main" }))).toBe("Directly → new session · app");
  });

  it("words every state", () => {
    const via = { type: "viaMono", monoId: "m1", monoName: "Ada" } as const;
    expect(stateText(proposal({ state: "awaitingOwner" }))).toBe("Waiting for your Send");
    expect(stateText(proposal({ state: "queuedAtMono", route: via }))).toBe("Waiting for Ada");
    expect(stateText(proposal({ state: "queuedAtMono" }))).toBe("Waiting for the app");
    expect(stateText(proposal({ state: "queuedInSession" }))).toBe("Queued in session");
    expect(stateText(proposal({ state: "sent" }))).toBe("Sent");
    expect(stateText(proposal({ state: "refused", reason: "Session is gone" }))).toBe("Refused: Session is gone");
    expect(stateText(proposal({ state: "notAttempted" }))).toBe("Not sent");
    expect(stateText(proposal({ state: "failed", reason: "Timed out" }))).toBe("Failed: Timed out");
    expect(stateText(proposal({ state: "cancelled" }))).toBe("Cancelled");
  });
});

describe("Chief in the lists", () => {
  it("pins the Chief first in the Mono home with a tag and a dot while commands wait", async () => {
    mount("/monos");
    const row = await screen.findByTestId("chief-row");
    const rows = screen.getAllByRole("link").filter((candidate) => candidate.getAttribute("data-testid") === "chief-row" || candidate.getAttribute("data-testid") === "mono-row");
    expect(rows[0]).toBe(row);
    expect(within(row).getByTestId("chief-tag")).toHaveTextContent("Chief");
    expect(within(row).getByTestId("chief-dot")).toHaveAccessibleName("2 commands waiting for you");
    expect(row).toHaveAttribute("href", `/d/${DESKTOP_ID}/monos/chief`);
  });

  it("shows no dot when nothing waits", async () => {
    mount("/monos", { seed: (desktop) => desktop.world.chief.proposals.forEach((p) => Object.assign(p, { open: false, state: "cancelled" })) });
    const row = await screen.findByTestId("chief-row");
    expect(within(row).queryByTestId("chief-dot")).toBeNull();
  });

  it("shows nothing when the Chief is off", async () => {
    mount("/monos", { seed: (desktop) => (desktop.world.chief.enabled = false) });
    await screen.findAllByTestId("mono-row");
    expect(screen.queryByTestId("chief-row")).toBeNull();
  });

  it("shows nothing on a desktop without the Chief", async () => {
    mount("/monos", { seed: (desktop) => (desktop.world.chief.supported = false) });
    await screen.findAllByTestId("mono-row");
    expect(screen.queryByTestId("chief-row")).toBeNull();
  });
});

describe("Chief chat cards", () => {
  it("draws the route line, the text, the notes and the state chip of each card", async () => {
    mount("/monos/chief");
    const via = await cardOf(CHIEF_FIXTURE_VIA_MONO_ID);
    expect(via).toHaveTextContent("Example Mono → #1 Fix the example parser · Example Alpha · example-branch");
    expect(via).toHaveTextContent("Run the parser tests and report");
    expect(via).toHaveTextContent("Will wait in the session queue");
    expect(via).toHaveTextContent("The Mono is busy and will take it after its current turn");
    expect(within(via).getByTestId("chief-state")).toHaveTextContent("Waiting for your Send");
    const direct = await cardOf(CHIEF_FIXTURE_DIRECT_ID);
    expect(direct).toHaveTextContent("Directly → #2 Run the example checks · Example Alpha · main");
    expect(direct).toHaveTextContent("The session's queue is paused, so it waits until you resume it");
    const sent = (await screen.findAllByTestId("chief-card")).find((card) => card.dataset.state === "sent")!;
    expect(within(sent).getByTestId("chief-state")).toHaveTextContent("Sent");
    expect(within(sent).queryByRole("button", { name: "Send" })).toBeNull();
  });

  it("draws a proposal that starts new work as a new session in its project, with the same chips and buttons", async () => {
    mount("/monos/chief", {
      ...WEB_SEND,
      seed: (desktop) => {
        WEB_SEND.seed?.(desktop);
        desktop.world.addChiefProposal({
          id: "p_0000beef",
          kind: "start",
          route: { type: "viaMono", monoId: "m1", monoName: "Example Mono" },
          sessionTitle: "",
          project: "Example Alpha",
          instruction: "Add a rate limit to the login endpoint",
          notes: ["monoBusy"],
        });
      },
    });
    const card = (await screen.findAllByTestId("chief-card")).find((candidate) => candidate.textContent?.includes("Add a rate limit"))!;
    expect(card).toHaveTextContent("Example Mono → new session · Example Alpha");
    expect(card).toHaveTextContent("The Mono is busy and will take it after its current turn");
    expect(within(card).getByTestId("chief-state")).toHaveTextContent("Waiting for your Send");
    expect(within(card).getByRole("button", { name: "Send" })).toBeEnabled();
    expect(within(card).getByRole("button", { name: "Edit" })).toBeEnabled();
    expect(within(card).getByRole("button", { name: "Cancel" })).toBeEnabled();
    await userEvent.click(within(card).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(within(card).getByTestId("chief-state")).toHaveTextContent("Waiting for Example Mono"));
  });

  it("offers Send, Edit and Cancel to a Control device when Send from the web is on", async () => {
    mount("/monos/chief", WEB_SEND);
    const card = await cardOf(CHIEF_FIXTURE_VIA_MONO_ID);
    expect(within(card).getByRole("button", { name: "Send" })).toBeEnabled();
    expect(within(card).getByRole("button", { name: "Edit" })).toBeEnabled();
    expect(within(card).getByRole("button", { name: "Cancel" })).toBeEnabled();
    expect(within(card).queryByTestId("chief-confirm-desktop")).toBeNull();
  });

  it("says Confirm on the desktop, with no Send, while Send from the web is off", async () => {
    mount("/monos/chief", { seed: (desktop) => (desktop.world.chief.allowWebSend = false) });
    const card = await cardOf(CHIEF_FIXTURE_VIA_MONO_ID);
    expect(within(card).getByTestId("chief-confirm-desktop")).toHaveTextContent("Confirm on the desktop");
    expect(within(card).queryByRole("button", { name: "Send" })).toBeNull();
    // Edit and Cancel are Control actions, not Send ones.
    expect(within(card).getByRole("button", { name: "Edit" })).toBeInTheDocument();
  });

  it("gives a read-only device no Send, Edit or Cancel", async () => {
    mount("/monos/chief", { permission: "read" });
    const card = await cardOf(CHIEF_FIXTURE_VIA_MONO_ID);
    expect(within(card).getByTestId("chief-confirm-desktop")).toHaveTextContent("Confirm on the desktop");
    expect(within(card).queryByRole("button", { name: "Send" })).toBeNull();
    expect(within(card).queryByRole("button", { name: "Edit" })).toBeNull();
    expect(within(card).queryByRole("button", { name: "Cancel" })).toBeNull();
  });

  it("Send queues a via-Mono card at the Mono and a direct card in the session", async () => {
    const user = userEvent.setup();
    mount("/monos/chief", WEB_SEND);
    await user.click(within(await cardOf(CHIEF_FIXTURE_VIA_MONO_ID)).getByRole("button", { name: "Send" }));
    await waitFor(async () => expect(within(await cardOf(CHIEF_FIXTURE_VIA_MONO_ID)).getByTestId("chief-state")).toHaveTextContent("Waiting for Example Mono"));
    await user.click(within(await cardOf(CHIEF_FIXTURE_DIRECT_ID)).getByRole("button", { name: "Send" }));
    await waitFor(async () => expect(within(await cardOf(CHIEF_FIXTURE_DIRECT_ID)).getByTestId("chief-state")).toHaveTextContent("Queued in session"));
    expect(link.desktop.world.chief.proposals.filter((p) => p.open)).toHaveLength(0);
  });

  it("Edit saves the new text, and Send after an edit sends the edited text", async () => {
    const user = userEvent.setup();
    mount("/monos/chief", WEB_SEND);
    let card = await cardOf(CHIEF_FIXTURE_VIA_MONO_ID);
    await user.click(within(card).getByRole("button", { name: "Edit" }));
    const field = within(card).getByRole("textbox", { name: "Command text" });
    await user.clear(field);
    await user.type(field, "Run only the fast tests");
    await user.click(within(card).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(link.desktop.world.chief.proposals.find((p) => p.id === CHIEF_FIXTURE_VIA_MONO_ID)?.instruction).toBe("Run only the fast tests"));
    await waitFor(() => expect(screen.getByText("Run only the fast tests")).toBeInTheDocument());

    card = await cardOf(CHIEF_FIXTURE_DIRECT_ID);
    await user.click(within(card).getByRole("button", { name: "Edit" }));
    const second = within(card).getByRole("textbox", { name: "Command text" });
    await user.clear(second);
    await user.type(second, "Skip lint");
    await user.click(within(card).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(link.desktop.world.chief.proposals.find((p) => p.id === CHIEF_FIXTURE_DIRECT_ID)).toMatchObject({ instruction: "Skip lint", state: "queuedInSession" }));
  });

  it("refuses a Send of text the device never saw, and shows why", async () => {
    const user = userEvent.setup();
    mount("/monos/chief", WEB_SEND);
    const card = await cardOf(CHIEF_FIXTURE_VIA_MONO_ID);
    // The owner edited the text on the desktop; this device still shows the old one.
    link.desktop.world.chief.proposals.find((p) => p.id === CHIEF_FIXTURE_VIA_MONO_ID)!.instruction = "Changed on the desktop";
    await user.click(within(card).getByRole("button", { name: "Send" }));
    expect(await within(card).findByTestId("chief-card-error")).toHaveTextContent("The text changed. Review the command again");
    expect(link.desktop.world.chief.proposals.find((p) => p.id === CHIEF_FIXTURE_VIA_MONO_ID)).toMatchObject({ open: true, state: "awaitingOwner" });
  });

  it("Cancel marks the card Cancelled", async () => {
    const user = userEvent.setup();
    mount("/monos/chief");
    await user.click(within(await cardOf(CHIEF_FIXTURE_DIRECT_ID)).getByRole("button", { name: "Cancel" }));
    await waitFor(async () => expect(within(await cardOf(CHIEF_FIXTURE_DIRECT_ID)).getByTestId("chief-state")).toHaveTextContent("Cancelled"));
  });

  it("shows the error of a refused call on the card", async () => {
    const user = userEvent.setup();
    mount("/monos/chief", WEB_SEND);
    const card = await cardOf(CHIEF_FIXTURE_DIRECT_ID);
    // Handled on the desktop meanwhile, without a frame yet.
    Object.assign(link.desktop.world.chief.proposals.find((p) => p.id === CHIEF_FIXTURE_DIRECT_ID)!, { open: false, state: "sent" });
    await user.click(within(card).getByRole("button", { name: "Send" }));
    expect(await within(card).findByTestId("chief-card-error")).toHaveTextContent("This command was already handled");
  });

  it("follows a frame: a new card shows up and a Send from the web turned off takes the button away", async () => {
    mount("/monos/chief", WEB_SEND);
    await cardOf(CHIEF_FIXTURE_VIA_MONO_ID);
    link.desktop.world.addChiefProposal({ id: "p_0000beef", instruction: "A fresh command", sessionTitle: "Fresh" });
    expect(await screen.findByText("A fresh command")).toBeInTheDocument();
    link.desktop.world.setChief({ allowWebSend: false });
    // The composer has a Send button too: look in the cards only.
    await waitFor(() => expect(within(screen.getByTestId("chief-cards")).queryByRole("button", { name: "Send" })).toBeNull());
  });

  it("links the chat header to the Chief profile", async () => {
    mount("/monos/chief");
    expect(await screen.findByRole("link", { name: "Chief profile" })).toHaveAttribute("href", `/d/${DESKTOP_ID}/monos/chief/profile`);
    expect(await screen.findByRole("heading", { level: 1, name: "Chief" })).toBeInTheDocument();
  });
});

describe("Chief profile", () => {
  it("shows the name, status, harness, Send from the web, open commands and where to change settings", async () => {
    mount("/monos/chief/profile");
    expect(await screen.findByRole("heading", { level: 2, name: "Chief" })).toBeInTheDocument();
    expect(await screen.findByTestId("chief-harness")).toHaveTextContent("Claude Code");
    expect(screen.getByTestId("chief-allow-web-send")).toHaveTextContent("Off");
    expect(screen.getByTestId("chief-open-count")).toHaveTextContent("2");
    expect(screen.getByTestId("chief-settings-note")).toHaveTextContent("desktop app");
  });
});

describe("Chief inline cards", () => {
  it("draws a card right after the turn that proposed it, and the rest in the list below, never twice", async () => {
    mount("/monos/chief", { seed: chiefTurn([CHIEF_FIXTURE_VIA_MONO_ID]) });
    const inline = await screen.findByTestId("chief-inline-cards");
    expect(inline).toHaveTextContent("Run the parser tests and report");
    expect(inline.previousElementSibling).toHaveTextContent("Here is what I propose");
    const list = screen.getByTestId("chief-cards");
    expect(list).toHaveTextContent("Also run the lint step");
    expect(list).not.toHaveTextContent("Run the parser tests and report");
    const all = screen.getAllByTestId("chief-card");
    expect(all.filter((card) => card.textContent?.includes("Run the parser tests and report"))).toHaveLength(1);
    expect(all).toHaveLength(3);
  });

  it("falls back to the list for a proposal whose turn is not loaded", async () => {
    mount("/monos/chief", { seed: chiefTurn(["p_not_in_this_chat"]) });
    await screen.findByText("Here is what I propose");
    expect(screen.queryByTestId("chief-inline-cards")).toBeNull();
    expect(within(await screen.findByTestId("chief-cards")).getAllByTestId("chief-card")).toHaveLength(3);
  });

  it("a card drawn inline still sends, and the list drops out when every card is inline", async () => {
    const user = userEvent.setup();
    mount("/monos/chief", {
      seed: (desktop) => {
        desktop.world.chief.allowWebSend = true;
        chiefTurn([CHIEF_FIXTURE_VIA_MONO_ID, CHIEF_FIXTURE_DIRECT_ID, "p_0000abcf"])(desktop);
      },
    });
    const inline = await screen.findByTestId("chief-inline-cards");
    expect(screen.queryByTestId("chief-cards")).toBeNull();
    await user.click(within(await cardOf(CHIEF_FIXTURE_VIA_MONO_ID)).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(inline).toHaveTextContent("Waiting for Example Mono"));
  });
});

describe("Chief emoji", () => {
  const EMOJI = "\u{1F989}";
  it("shows the emoji in the rail row, the chat header and the profile", async () => {
    mount("/monos", { seed: (desktop) => (desktop.world.chief.emoji = EMOJI) });
    const row = await screen.findByTestId("chief-row");
    expect(within(row).getByTestId("mono-avatar")).toHaveAttribute("data-kind", "emoji");
    expect(within(row).getByTestId("mono-avatar")).toHaveTextContent(EMOJI);
  });

  it("shows it in the chat header and the profile", async () => {
    mount("/monos/chief", { seed: (desktop) => (desktop.world.chief.emoji = EMOJI) });
    await screen.findByRole("heading", { level: 1, name: "Chief" });
    await waitFor(() => expect(screen.getAllByTestId("mono-avatar").some((avatar) => avatar.textContent === EMOJI)).toBe(true));
  });

  it("keeps the mascot while the Chief has no emoji", async () => {
    mount("/monos");
    const row = await screen.findByTestId("chief-row");
    expect(within(row).getByTestId("mono-avatar")).not.toHaveAttribute("data-kind", "emoji");
  });
});

describe("Chief profile editing", () => {
  it("lets a Control device change the allowed fields through chief.update", async () => {
    const user = userEvent.setup();
    mount("/monos/chief/profile");
    const name = await screen.findByRole("textbox", { name: "Name" });
    await user.clear(name);
    await user.type(name, "Boss");
    await user.click(screen.getByRole("radio", { name: "Green" }));
    await user.type(screen.getByRole("textbox", { name: "Emoji" }), "\u{1F989}");
    const model = screen.getByRole("textbox", { name: "Model" });
    await user.clear(model);
    await user.type(model, "model-x");
    await user.click(screen.getByRole("checkbox", { name: /Notify me/ }));
    await user.click(screen.getByTestId("chief-save"));
    await waitFor(() => expect(link.desktop.world.chief).toMatchObject({ name: "Boss", color: "#39c363", emoji: "\u{1F989}", model: "model-x", notifyOnDelivery: false }));
    expect(link.desktop.world.chief).toMatchObject({ allowWebSend: false, enabled: true });
    expect(await screen.findByRole("heading", { level: 2, name: "Boss" })).toBeInTheDocument();
  });

  it("shows enabled, harness and Send from the web as text only", async () => {
    mount("/monos/chief/profile");
    await screen.findByTestId("chief-edit");
    expect(screen.getByTestId("chief-harness")).toHaveTextContent("Claude Code");
    expect(screen.getByTestId("chief-allow-web-send")).toHaveTextContent("Off");
    expect(screen.getAllByRole("checkbox")).toHaveLength(1);
    expect(screen.queryByRole("switch")).toBeNull();
  });

  it("shows the desktop's refusal inline", async () => {
    const user = userEvent.setup();
    mount("/monos/chief/profile");
    const name = await screen.findByRole("textbox", { name: "Name" });
    await user.type(name, "x");
    link.desktop.world.setChief({ enabled: false });
    await user.click(screen.getByTestId("chief-save"));
    expect(await screen.findByText("The Chief is off")).toBeInTheDocument();
  });

  it("gives a read-only device no controls", async () => {
    mount("/monos/chief/profile", { permission: "read" });
    expect(await screen.findByTestId("chief-harness")).toBeInTheDocument();
    expect(screen.queryByTestId("chief-edit")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByTestId("chief-save")).toBeNull();
  });

  it("gives a device without the Chief in its catalog (narrowed) no controls", async () => {
    mount("/monos/chief/profile", { seed: (desktop) => (desktop.world.chief.supported = false) });
    expect(await screen.findByTestId("chief-harness")).toBeInTheDocument();
    expect(screen.queryByTestId("chief-edit")).toBeNull();
  });
});

describe("Mono chat with a relayed command", () => {
  it("labels a user turn the Chief relayed with From you, via the Chief", async () => {
    mount("/monos/mono-example");
    const label = await screen.findByTestId("chief-relay-label");
    expect(label).toHaveTextContent("From you, via Chief");
    expect(label.parentElement).toHaveTextContent("Run the parser tests and report");
  });
});
