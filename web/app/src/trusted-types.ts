// Trusted Types `default` policy of security.md 8.2. With the CSP directives
// `require-trusted-types-for 'script'; trusted-types default dompurify`, every string that reaches a DOM or
// script sink goes through this policy: HTML and script text are refused, script URLs are allowed
// only for the service worker and same-origin /assets/*.js.
//
// The one exception is a Mermaid render (security.md 8.2, "Mermaid"). While a render holds `captureMermaidStyle`, and only then:
//  - the string that starts with that render's own `#<id>{` selector is its theme CSS, which mermaid writes with
//    `style.innerHTML`: it is stored and handed back to the renderer (the sink receives an empty string), because a runtime
//    <style> would be blocked by `style-src 'self'` anyway and the CSS belongs in the blob image;
//  - any other string (mermaid sets icons, KaTeX math and HTML labels with d3 `.html()`) goes through the `sanitize` function
//    the renderer gave, DOMPurify with a strict config, and the sink receives its result.
// Outside a capture everything still throws. Captures are keyed by render id and ended by their render
// settling or, for one that hangs, by `releaseMermaidCapture` at its timeout: a late write then throws.

type Capture = { prefix: string; rules: string[]; sanitize: (html: string) => string };
const held = new Map<string, Capture>();

/**
 * Runs `render` while the policy diverts the theme CSS of the Mermaid render `id` and sanitizes its other HTML writes with
 * `sanitize`. Returns the diverted CSS (empty where Trusted Types do not exist: there the library writes it into the element
 * itself).
 */
export async function captureMermaidStyle<T>(id: string, render: () => Promise<T>, sanitize: (html: string) => string): Promise<{ value: T; rules: string }> {
  const mine: Capture = { prefix: `#${id}{`, rules: [], sanitize };
  held.set(id, mine);
  try {
    const value = await render();
    return { value, rules: mine.rules.join("\n") };
  } finally {
    held.delete(id);
  }
}

/** Ends the capture of render `id` at once (a render that timed out): later writes throw again. Safe to call twice. */
export function releaseMermaidCapture(id: string): void {
  held.delete(id);
}

function divert(input: string): string | null {
  let any: Capture | undefined;
  for (const capture of held.values()) {
    any = capture;
    if (input.startsWith(capture.prefix)) {
      capture.rules.push(input);
      return "";
    }
  }
  return any ? any.sanitize(input) : null;
}

export function installTrustedTypesPolicy(): void {
  window.trustedTypes?.createPolicy("default", {
    createHTML: (input) => {
      const diverted = divert(input);
      if (diverted !== null) return diverted;
      throw new TypeError("HTML sinks are disabled");
    },
    createScript: () => { throw new TypeError("Script sinks are disabled"); },
    createScriptURL: (url) => {
      const u = new URL(url, location.href);
      const ok = u.pathname === "/sw.js" || (u.pathname.startsWith("/assets/") && u.pathname.endsWith(".js"));
      if (u.origin === location.origin && ok) return u.href;
      throw new TypeError("Blocked script URL");
    },
  });
}
