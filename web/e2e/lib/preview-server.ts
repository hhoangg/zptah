// Serves a production build of the dev preview page (web/app/vite.preview.config.ts) with the Worker's real security
// headers (`buildCsp` and COMPANION_HEADERS from web/worker/src/csp.ts), so an e2e spec runs the app code under the same
// Content-Security-Policy and Trusted Types as production. `vite dev` cannot: it injects inline scripts and <style> elements.
// No relay, account or passkey is involved (the page talks to an in-browser mock desktop).
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import { extname, join, normalize, sep } from "node:path";
import { COMPANION_HEADERS, buildCsp } from "../../worker/src/csp.ts";
import { webDir } from "./paths.ts";

const PAGE = "/src/features/sessions/dev/index.html";
const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".json": "application/json",
  ".png": "image/png",
};

export interface PreviewServer {
  /** `http://localhost:PORT`. */
  base: string;
  /** The Content-Security-Policy every response carries. */
  csp: string;
  stop(): Promise<void>;
}

/** Builds the preview once into a temp dir and serves it on a free port. */
export async function startPreviewServer(): Promise<PreviewServer> {
  const out = mkdtempSync(join(tmpdir(), "monoptah-e2e-preview-"));
  const appDir = join(webDir, "app");
  const build = spawnSync(join(appDir, "node_modules", ".bin", "vite"), ["build", "-c", "vite.preview.config.ts"], {
    cwd: appDir,
    encoding: "utf8",
    env: { ...process.env, VITE_MOCK_API: "1", PREVIEW_OUT_DIR: out },
  });
  if (build.status !== 0) throw new Error(`preview build failed (exit ${build.status}): ${(build.stdout + build.stderr).slice(-800)}`);

  let csp = "";
  const server: Server = createServer((request, response) => {
    const path = decodeURIComponent((request.url ?? "/").split("?")[0] ?? "/");
    // The page itself and its assets only; anything else is a plain 404, not the HTML. The app registers /sw.js: an empty worker.
    if (path === "/sw.js") {
      response.writeHead(200, { "Content-Security-Policy": csp, ...COMPANION_HEADERS, "Content-Type": TYPES[".js"] ?? "", "Cache-Control": "no-store" }).end("");
      return;
    }
    const file = normalize(join(out, path.startsWith("/assets/") ? path : path === "/" || path === PAGE ? PAGE : "missing"));
    const headers: Record<string, string> = { "Content-Security-Policy": csp, ...COMPANION_HEADERS, "Cache-Control": "no-store" };
    if (!file.startsWith(out + sep) || !existsSync(file) || !statSync(file).isFile()) {
      response.writeHead(404, headers).end("not found");
      return;
    }
    response.writeHead(200, { ...headers, "Content-Type": TYPES[extname(file)] ?? "application/octet-stream" }).end(readFileSync(file));
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "localhost", () => resolve());
  });
  const address = server.address();
  const port = typeof address === "object" && address !== null ? address.port : 0;
  csp = buildCsp(`localhost:${port}`);
  return {
    base: `http://localhost:${port}`,
    csp,
    stop: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => {
          rmSync(out, { recursive: true, force: true });
          resolve();
        });
      }),
  };
}
