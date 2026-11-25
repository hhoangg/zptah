import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ResponsiveOverlay } from "./ResponsiveOverlay";

const original = window.matchMedia;
function viewport(wide: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: wide && query.includes("min-width: 640px"),
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as never;
}

beforeEach(() => viewport(false));
afterEach(() => {
  window.matchMedia = original;
});

describe("ResponsiveOverlay mobile sheet layout", () => {
  it("never shrinks the header or footer, so a long list cannot overlap the description", () => {
    render(
      <ResponsiveOverlay open onOpenChange={() => undefined} title="Projects" description="A long description" footer={<button>Save</button>}>
        body
      </ResponsiveOverlay>,
    );
    expect(document.querySelector('[data-slot="drawer-header"]')!.className).toContain("shrink-0");
    expect(screen.getByRole("button", { name: "Save" }).parentElement!.className).toContain("shrink-0");
  });
});

describe("ResponsiveOverlay fullHeight", () => {
  it("makes the mobile sheet fill the viewport", () => {
    render(
      <ResponsiveOverlay open onOpenChange={() => undefined} title="Search" fullHeight>
        body
      </ResponsiveOverlay>,
    );
    const sheet = document.querySelector('[data-slot="drawer-content"]')!;
    expect(sheet.className).toContain("h-dvh");
    expect(sheet.className).toContain("max-h-dvh");
    expect(sheet.className).not.toContain("max-h-[92dvh]");
    expect(screen.getByRole("dialog", { name: "Search" })).toBeInTheDocument();
  });

  it("keeps the 92dvh sheet by default", () => {
    render(
      <ResponsiveOverlay open onOpenChange={() => undefined} title="Search">
        body
      </ResponsiveOverlay>,
    );
    const sheet = document.querySelector('[data-slot="drawer-content"]')!;
    expect(sheet.className).toContain("max-h-[92dvh]");
    expect(sheet.className).not.toContain("h-dvh");
  });

  it("is a dialog, not a sheet, from 640px", () => {
    viewport(true);
    render(
      <ResponsiveOverlay open onOpenChange={() => undefined} title="Search" fullHeight>
        body
      </ResponsiveOverlay>,
    );
    expect(document.querySelector('[data-slot="drawer-content"]')).toBeNull();
  });
});
