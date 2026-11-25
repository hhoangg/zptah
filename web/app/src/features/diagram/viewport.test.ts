import { describe, expect, it } from "vitest";
import { ASCENT, labelOrigin, luminance, prefersDarkText } from "./model";
import { DEFAULT_VIEWPORT, FIT_MAX, MAX_SCALE, MIN_SCALE, boardHeight, dotSpacing, fitViewport, panBy, percent, pinchFactor, toContent, wheelFactor, zoomAt } from "./viewport";

describe("viewport math (same constants and formulas as the desktop)", () => {
  it("keeps the point under the pointer fixed while zooming, and clamps the scale", () => {
    let view = { scale: 1, x: 30, y: -10 };
    const before = toContent(view, 200, 120);
    view = zoomAt(view, 1.5, 200, 120);
    const after = toContent(view, 200, 120);
    expect(after[0]).toBeCloseTo(before[0], 3);
    expect(after[1]).toBeCloseTo(before[1], 3);
    expect(view.scale).toBeCloseTo(1.5, 6);
    expect(zoomAt(view, 100, 0, 0).scale).toBe(MAX_SCALE);
    expect(zoomAt(view, 0.0001, 0, 0).scale).toBe(MIN_SCALE);
    expect(percent(DEFAULT_VIEWPORT)).toBe(100);
  });

  it("fits and centres, never enlarging past the limit", () => {
    const v = fitViewport([1000, 300], [500, 400]);
    expect(v.scale).toBeCloseTo(0.46, 4);
    expect(v.x).toBeCloseTo(20, 3);
    expect(v.y).toBeCloseTo((400 - 300 * v.scale) / 2, 3);
    const tiny = fitViewport([50, 50], [800, 600]);
    expect(tiny.scale).toBe(FIT_MAX);
    expect(tiny.x).toBeCloseTo((800 - 50 * FIT_MAX) / 2, 3);
    expect(fitViewport([100000, 100000], [400, 300]).scale).toBe(MIN_SCALE);
  });

  it("pans and converts wheel and pinch gestures to factors", () => {
    expect(panBy(DEFAULT_VIEWPORT, 5, -7)).toEqual({ scale: 1, x: 5, y: -7 });
    expect(wheelFactor(-20)).toBeGreaterThan(1.05);
    expect(wheelFactor(-20)).toBeLessThan(1.15);
    expect(wheelFactor(20)).toBeLessThan(1);
    expect(wheelFactor(-1e6)).toBe(2);
    expect(pinchFactor(0.1)).toBeCloseTo(1.1, 6);
    expect(pinchFactor(-5)).toBe(0.5);
  });

  it("sizes the board from the content and spaces the dots", () => {
    expect(boardHeight(50)).toBe(180);
    expect(boardHeight(300)).toBe(340);
    expect(boardHeight(900)).toBe(420);
    expect(boardHeight(900, 250)).toBe(250);
    expect(dotSpacing(1)).toBe(18);
    expect(dotSpacing(0.25)).toBe(9);
  });
});

describe("canvas label anchoring math", () => {
  const line = 12 * 1.4;
  it("keeps (x, y) the top-left corner with the defaults", () => {
    expect(labelOrigin(10, 20, 50, 12, "start", "top")).toEqual([10, 20]);
  });
  it("centres and bottom-aligns", () => {
    expect(labelOrigin(10, 20, 50, 12, "center", "middle")).toEqual([-15, 20 - line / 2]);
    expect(labelOrigin(10, 20, 50, 12, "end", "bottom")).toEqual([-40, 20 - line]);
    expect(labelOrigin(0, 100, 0, 12, "start", "alphabetic")).toEqual([0, 100 - ASCENT * 12]);
    expect(labelOrigin(0, 18, 0, 12, "start", "middle")[1] + line / 2).toBeCloseTo(18, 4);
  });
  it("picks dark ink on light fills", () => {
    expect(prefersDarkText(luminance([1, 1, 1]))).toBe(true);
    expect(prefersDarkText(luminance([0.98, 0.9, 0.3]))).toBe(true);
    expect(prefersDarkText(luminance([0, 0, 0]))).toBe(false);
    expect(prefersDarkText(luminance([0.1, 0.2, 0.5]))).toBe(false);
  });
});
