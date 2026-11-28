// The Workflows entry of the workspace drawer: there with the `workflows` capability, absent without it.
import { render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({ workflows: true }));
vi.mock("@/version", () => ({ useCapability: (name: string) => name === "workflows" && h.workflows }));
vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, params, children, className }: { to: string; params: { desktopId: string }; children: ReactNode; className?: string }) => (
    <a href={to.replace("$desktopId", params.desktopId)} className={className}>
      {children}
    </a>
  ),
}));

import { WorkflowsNavItem } from "./WorkflowsNavItem";

beforeEach(() => {
  h.workflows = true;
});

describe("the drawer Workflows entry", () => {
  it("links to the list when the desktop advertises workflows", () => {
    render(<WorkflowsNavItem desktopId="d1" />);
    expect(screen.getByRole("link", { name: "Workflows" })).toHaveAttribute("href", "/d/d1/workflows");
  });

  it("is not there on a desktop without the capability", () => {
    h.workflows = false;
    render(<WorkflowsNavItem desktopId="d1" />);
    expect(screen.queryByRole("link", { name: "Workflows" })).toBeNull();
  });
});
