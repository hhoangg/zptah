import path from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { staticLibraryCss } from "./vite-plugins/staticLibraryCss.ts";
import { xtermStaticStyles } from "./vite-plugins/xtermStaticStyles.ts";

// A production build of the dev-only preview page (src/features/sessions/dev/, an in-browser mock desktop) for the e2e
// specs that must run under the real Content-Security-Policy and Trusted Types: `vite dev` cannot, because it injects inline
// scripts and runtime <style> elements. It has the same S-08 patches as vite.config.ts but no Worker, router or PWA plugin.
// Not part of `vite build`; web/e2e/lib/preview-server.ts runs it into a temp dir (PREVIEW_OUT_DIR) and serves it with the
// Worker's own CSP header.
export default defineConfig({
  plugins: [react(), tailwindcss(), staticLibraryCss(), xtermStaticStyles()],
  resolve: {
    alias: [
      { find: "@", replacement: path.resolve(import.meta.dirname, "src") },
      { find: /^react-style-singleton$/, replacement: path.resolve(import.meta.dirname, "src/patches/style-singleton.ts") },
    ],
  },
  build: {
    outDir: process.env["PREVIEW_OUT_DIR"] ?? "dist-preview",
    emptyOutDir: true,
    rollupOptions: { input: path.resolve(import.meta.dirname, "src/features/sessions/dev/index.html") },
  },
});
