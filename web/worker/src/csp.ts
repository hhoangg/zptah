// The CSP and the companion security headers of security.md 8.1. Dependency free on purpose: the
// app build (web/app) imports this file to generate `_headers`, so the policy exists in one place.

/**
 * The Content-Security-Policy of security.md 8.1, verbatim, with `HOST` substituted: some browsers do
 * not match `'self'` to `wss:` on the same host. `host` must be a bare `host[:port]` (a URL host).
 */
export function buildCsp(host: string): string {
  return [
    "default-src 'none'",
    "script-src 'self'",
    // The hash is the SHA-256 of the empty string: mermaid inserts one empty <style> into its scratch SVG for every diagram (its
    // theme CSS is diverted by the Trusted Types policy and goes only into the blob: image), and an empty element is still
    // checked. It permits that and nothing else: an element with any text is not matched. See security.md 8.1 and 8.2.
    "style-src 'self' 'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='",
    "style-src-attr 'unsafe-inline'",
    "img-src 'self' data: blob:",
    "frame-src blob:",
    "font-src 'self'",
    `connect-src 'self' wss://${host}`,
    "worker-src 'self'",
    "manifest-src 'self'",
    // Not 'none': the file preview frame is a blob: document that inherits this policy, and WebKit then refuses to load it
    // (a blank frame) because its parent is not allowed by frame-ancestors. 'self' still blocks every other origin.
    "frame-ancestors 'self'",
    "base-uri 'none'",
    "form-action 'self'",
    "object-src 'none'",
    "require-trusted-types-for 'script'",
    // `default` is the app's refuse-everything policy (app/src/trusted-types.ts). `dompurify` is the one extra name: mermaid
    // sanitizes its SVG with DOMPurify, which makes this policy (identity createHTML) for its own DOMParser input. Nothing else
    // may create a policy, and every other HTML sink still goes through `default` and throws. See security.md 8.2.
    "trusted-types default dompurify",
    "upgrade-insecure-requests",
  ].join("; ");
}

/** Companion headers of security.md 8.1, on every response of the Worker. */
export const COMPANION_HEADERS: Readonly<Record<string, string>> = {
  "Strict-Transport-Security": "max-age=63072000; includeSubDomains; preload",
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "no-referrer",
  "Permissions-Policy": "camera=(), microphone=(self), geolocation=(), payment=(), usb=()",
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Resource-Policy": "same-origin",
};
