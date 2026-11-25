import { describe, expect, it, vi } from "vitest";
import { createRootNavigationHandler } from "./rootNavigation";

const request = new Request("https://zptah.app/");

describe("root navigation handler", () => {
  it("answers from the network when it is reachable (the Worker picks landing or app)", async () => {
    const matchShell = vi.fn();
    const handler = createRootNavigationHandler({ fetch: () => Promise.resolve(new Response("landing")), matchShell });
    expect(await (await handler(request)).text()).toBe("landing");
    expect(matchShell).not.toHaveBeenCalled();
  });

  it("falls back to the precached shell when offline, so an old install can still open", async () => {
    const handler = createRootNavigationHandler({
      fetch: () => Promise.reject(new TypeError("offline")),
      matchShell: () => Promise.resolve(new Response("shell")),
    });
    expect(await (await handler(request)).text()).toBe("shell");
  });

  it("rethrows the network error when there is no cached shell", async () => {
    const handler = createRootNavigationHandler({ fetch: () => Promise.reject(new TypeError("offline")), matchShell: () => Promise.resolve(undefined) });
    await expect(handler(request)).rejects.toThrow("offline");
  });
});
