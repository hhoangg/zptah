// Navigations to `/`. The Worker answers them (the landing page without a session cookie, the app with one), so they go to
// the network first. They are denylisted from the cached-shell fallback (routing.ts), which would hide the landing page,
// but an app installed before `start_url` became `/app` (iOS never refreshes it) still starts at `/` and must open offline:
// when the network fails, answer with the precached shell, which then redirects to the last place or to /login.

export interface RootNavigationDeps {
  fetch: (request: Request) => Promise<Response>;
  /** The precached shell document (`/index.html`), or undefined when it is not cached. */
  matchShell: () => Promise<Response | undefined>;
}

export function createRootNavigationHandler(deps: RootNavigationDeps): (request: Request) => Promise<Response> {
  return async (request) => {
    try {
      return await deps.fetch(request);
    } catch (error) {
      const shell = await deps.matchShell();
      if (shell) return shell;
      throw error;
    }
  };
}
