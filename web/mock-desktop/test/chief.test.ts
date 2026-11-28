import { WatchFrameSchema } from "@monoptah/protocol";
import { describe, expect, it } from "vitest";
import { CHIEF_FIXTURE_DIRECT_ID, CHIEF_FIXTURE_RELAY_TEXT, CHIEF_FIXTURE_VIA_MONO_ID, fingerprint } from "../src/index.ts";
import { setup } from "./harness.ts";

const DIRECT_TEXT = "Also run the lint step before you finish";

describe("the Chief", () => {
  it("lists the catalog entry, the proposals newest first and the profile", async () => {
    const { client } = await setup();
    const catalog = await client.call("workspace.list");
    expect(catalog.chief).toMatchObject({ enabled: true, name: "Chief", status: "needs-you", openCount: 2 });
    const list = await client.call("chief.proposals");
    expect(list).toMatchObject({ enabled: true, openCount: 2, allowWebSend: false });
    expect(list.proposals.map((p) => p.id)).toEqual([CHIEF_FIXTURE_DIRECT_ID, CHIEF_FIXTURE_VIA_MONO_ID, "p_0000abcf"]);
    expect(list.proposals.some((p) => p.route.type === "viaMono")).toBe(true);
    expect(await client.call("chief.profile")).toMatchObject({ enabled: true, name: "Chief", allowWebSend: false, harness: { ok: true, line: "Claude Code" } });
  });

  it("serves the conversation `chief` with the extra on a read and on the first watch frame, then only on change", async () => {
    const s = await setup();
    const read = await s.client.call("conversation.read", { conversationId: "chief" });
    expect(read.chief).toMatchObject({ openCount: 2, allowWebSend: false });
    const watch = await s.client.openWatch({ conversationId: "chief" });
    await s.relay.settled();
    expect(watch.frames).toHaveLength(1);
    expect(WatchFrameSchema.safeParse(watch.frames[0]).success).toBe(true);
    expect(watch.frames[0]?.chief?.proposals).toHaveLength(3);
    await s.client.call("chief.proposal.cancel", { proposalId: CHIEF_FIXTURE_DIRECT_ID });
    await s.relay.settled();
    expect(watch.frames).toHaveLength(2);
    expect(watch.frames[1]?.chief?.openCount).toBe(1);
    // The Chief is not a coding session and not a Mono in the lists.
    expect((await s.client.call("workspace.list")).sessions.some((session) => session.id === "chief")).toBe(false);
  });

  it("send queues at the Mono or in the session, an edit text rides along, and a handled card is refused", async () => {
    const { client, desktop } = await setup();
    desktop.world.setChief({ allowWebSend: true });
    const via = await client.call("chief.proposal.send", { proposalId: CHIEF_FIXTURE_VIA_MONO_ID, expectedInstruction: CHIEF_FIXTURE_RELAY_TEXT, instruction: "Run the fast tests" });
    expect(via.proposal).toMatchObject({ state: "queuedAtMono", open: false, instruction: "Run the fast tests" });
    expect(via.proposal.confirmedAt).toBeTypeOf("number");
    expect((await client.call("chief.proposal.send", { proposalId: CHIEF_FIXTURE_DIRECT_ID, expectedInstruction: DIRECT_TEXT })).proposal.state).toBe("queuedInSession");
    const again = await client.request("chief.proposal.send", { proposalId: CHIEF_FIXTURE_DIRECT_ID, expectedInstruction: DIRECT_TEXT });
    expect(again).toMatchObject({ ok: false, error: "This command was already handled" });
    expect((await client.call("workspace.list")).chief).toMatchObject({ openCount: 0, status: "idle" });
  });

  it("refuses a send whose expected text is not the stored text, and leaves the card open", async () => {
    const { client, desktop } = await setup();
    desktop.world.setChief({ allowWebSend: true });
    const stale = await client.request("chief.proposal.send", { proposalId: CHIEF_FIXTURE_VIA_MONO_ID, expectedInstruction: "An older text" });
    expect(stale).toMatchObject({ ok: false, error: "The text changed. Review the command again" });
    expect((await client.call("chief.proposals")).proposals.find((p) => p.id === CHIEF_FIXTURE_VIA_MONO_ID)).toMatchObject({ open: true });
  });

  it("carries a proposal that starts new work, and a direct one is sent at once because no session exists to queue in", async () => {
    const { client, desktop } = await setup();
    desktop.world.setChief({ allowWebSend: true });
    desktop.world.addChiefProposal({ id: "p_0000b001", kind: "start", route: { type: "viaMono", monoId: "m1", monoName: "Example Mono" }, sessionTitle: "", instruction: "Start the work" });
    desktop.world.addChiefProposal({ id: "p_0000b002", kind: "start", sessionTitle: "", instruction: "Start it directly" });
    const listed = await client.call("chief.proposals");
    expect(listed.proposals.filter((p) => p.kind === "start").map((p) => p.project)).toEqual(["Example Alpha", "Example Alpha"]);
    expect((await client.call("chief.proposal.send", { proposalId: "p_0000b001", expectedInstruction: "Start the work" })).proposal.state).toBe("queuedAtMono");
    expect((await client.call("chief.proposal.send", { proposalId: "p_0000b002", expectedInstruction: "Start it directly" })).proposal).toMatchObject({ state: "sent", open: false });
  });

  it("edits and cancels an open proposal", async () => {
    const { client } = await setup();
    expect((await client.call("chief.proposal.edit", { proposalId: CHIEF_FIXTURE_DIRECT_ID, instruction: "New text" })).proposal).toMatchObject({ instruction: "New text", open: true });
    expect((await client.call("chief.proposal.cancel", { proposalId: CHIEF_FIXTURE_DIRECT_ID })).proposal).toMatchObject({ state: "cancelled", open: false });
  });

  it("a read-only device may read but gets read_only on send, edit and cancel", async () => {
    const { client } = await setup({ permission: "read" });
    expect((await client.call("chief.proposals")).proposals.length).toBe(3);
    for (const [method, params] of [
      ["chief.proposal.send", { proposalId: CHIEF_FIXTURE_VIA_MONO_ID, expectedInstruction: CHIEF_FIXTURE_RELAY_TEXT }],
      ["chief.proposal.edit", { proposalId: CHIEF_FIXTURE_VIA_MONO_ID, instruction: "x" }],
      ["chief.proposal.cancel", { proposalId: CHIEF_FIXTURE_VIA_MONO_ID }],
    ] as const) {
      expect(await client.request(method, params), method).toMatchObject({ ok: false, code: "read_only" });
    }
  });

  it("send answers disabled while Send from the web is off, and every chief.* answers disabled while the Chief is off", async () => {
    const s = await setup();
    s.desktop.world.setChief({ allowWebSend: false });
    expect(await s.client.request("chief.proposal.send", { proposalId: CHIEF_FIXTURE_VIA_MONO_ID, expectedInstruction: CHIEF_FIXTURE_RELAY_TEXT })).toMatchObject({ ok: false, code: "disabled" });
    expect((await s.client.call("chief.proposal.edit", { proposalId: CHIEF_FIXTURE_VIA_MONO_ID, instruction: "x" })).proposal.instruction).toBe("x");
    s.desktop.world.setChief({ enabled: false });
    expect((await s.client.call("workspace.list")).chief).toEqual({ enabled: false, openCount: 0 });
    expect(await s.client.request("chief.proposals")).toMatchObject({ ok: false, code: "disabled" });
    expect(await s.client.request("conversation.read", { conversationId: "chief" })).toMatchObject({ ok: false, code: "disabled" });
  });

  it("an older desktop has no chief key, and a narrowed device never reaches the Chief", async () => {
    const s = await setup();
    s.desktop.world.setChief({ supported: false });
    expect("chief" in (await s.client.call("workspace.list"))).toBe(false);
    s.desktop.world.setChief({ supported: true });
    s.desktop.setDevice(fingerprint(s.deviceKey.publicKey), { selection: { projects: ["proj-alpha"], monos: null } });
    expect("chief" in (await s.client.call("workspace.list"))).toBe(false);
    expect(await s.client.request("chief.proposals")).toMatchObject({ ok: false, code: "not_selected" });
    expect(await s.client.request("chief.proposal.send", { proposalId: CHIEF_FIXTURE_VIA_MONO_ID, expectedInstruction: CHIEF_FIXTURE_RELAY_TEXT })).toMatchObject({ ok: false, code: "not_selected" });
    expect(await s.client.request("conversation.read", { conversationId: "chief" })).toMatchObject({ ok: false, code: "not_selected" });
  });

  it("Send from the web is off by default", async () => {
    const { client } = await setup();
    expect(await client.request("chief.proposal.send", { proposalId: CHIEF_FIXTURE_VIA_MONO_ID, expectedInstruction: CHIEF_FIXTURE_RELAY_TEXT })).toMatchObject({ ok: false, code: "disabled" });
  });

  it("chief.update changes the allowed fields, returns the profile and the catalog follows", async () => {
    const s = await setup();
    const profile = await s.client.call("chief.update", { name: " Boss ", emoji: "x", color: "AABBCC", model: "model-x", effort: "high", notifyOnDelivery: false });
    expect(profile).toMatchObject({ name: "Boss", emoji: "x", color: "#aabbcc", notifyOnDelivery: false, allowWebSend: false, harness: { model: "model-x", effort: "high" } });
    expect(await s.client.call("chief.profile")).toEqual(profile);
    expect((await s.client.call("workspace.list")).chief).toMatchObject({ name: "Boss", emoji: "x", color: "#aabbcc" });
    const reset = await s.client.call("chief.update", { emoji: null, model: null, effort: null });
    expect("emoji" in reset).toBe(false);
    expect(reset.harness).toMatchObject({ model: "example-model-1" });
  });

  it("chief.update refuses forbidden and unknown keys, an empty object, a read-only device and a narrowed device", async () => {
    const s = await setup();
    for (const params of [{}, { enabled: false }, { allowWebSend: true }, { name: "A", harness: "codex" }, { account: "a" }, { soul: "x" }, { other: 1 }]) {
      expect(await s.client.request("chief.update", params as never), JSON.stringify(params)).toMatchObject({ ok: false, error: "Invalid parameters" });
    }
    expect(s.desktop.world.chief).toMatchObject({ enabled: true, allowWebSend: false, name: "Chief" });
    const ro = await setup({ permission: "read" });
    expect(await ro.client.request("chief.update", { name: "Boss" })).toMatchObject({ ok: false, code: "read_only" });
    s.desktop.setDevice(fingerprint(s.deviceKey.publicKey), { selection: { projects: ["proj-alpha"], monos: null } });
    expect(await s.client.request("chief.update", { name: "Boss" })).toMatchObject({ ok: false, code: "not_selected" });
  });

  it("a Mono chat carries a relayed user turn with chiefRelay", async () => {
    const { client } = await setup();
    const read = await client.call("conversation.read", { conversationId: "mono:mono-example" });
    const item = read.turns.flatMap((turn) => turn.items).find((candidate) => "chiefRelay" in candidate);
    expect(item).toMatchObject({ type: "user", text: CHIEF_FIXTURE_RELAY_TEXT, chiefRelay: { proposalId: CHIEF_FIXTURE_VIA_MONO_ID, chiefName: "Chief" } });
  });
});
