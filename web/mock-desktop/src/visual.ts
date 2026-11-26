// A fake interactive visual for the `visual.dispatch` fixture: one tree with every control type. Like the desktop's
// sandbox it numbers its handlers `<generation>:<index>`, so a handler of an older render is stale and answers the
// first render again (`reset`). The state is shared by every device of a world: the fixture only needs to show that
// a reply carries the next tree.
const NAMES = ["Bar", "Line", "Area"] as const;
const KINDS = ["bar", "line", "area"] as const;
const RANGES = ["Day", "Week", "Month"];
const VALUES = [3, 5, 4, 8, 9];

export interface VisualEventState {
  tab: number;
  clicks: number;
  on: boolean;
  range: number;
  level: number;
  note: string;
}

const INITIAL: VisualEventState = { tab: 0, clicks: 0, on: false, range: 0, level: 30, note: "" };

export class MockVisual {
  #state: VisualEventState = { ...INITIAL };
  #generation = 1;
  /** Every accepted event, for a test. */
  readonly events: { handler: string; value: unknown }[] = [];

  get state(): Readonly<VisualEventState> {
    return this.#state;
  }

  tree(): unknown {
    const h = (index: number) => `${this.#generation}:${index}`;
    const s = this.#state;
    return {
      kind: "column",
      gap: 8,
      children: [
        { kind: "tabs", labels: [...NAMES], selected: s.tab, handler: h(0) },
        { kind: "chart", chart_type: KINDS[s.tab], series: [{ label: "Deploys", values: VALUES }], labels: ["Mon", "Tue", "Wed", "Thu", "Fri"], height: 160 },
        { kind: "text", text: `Clicks ${s.clicks} / ${s.on ? "Notify on" : "Notify off"} / ${RANGES[s.range]} / Level ${s.level} / Note ${s.note || "empty"}`, size: "small", color: "muted" },
        { kind: "button", label: "Add one", handler: h(1), disabled: false },
        { kind: "toggle", label: "Notify", value: s.on, handler: h(2) },
        { kind: "select", label: "Range", options: RANGES, selected: s.range, handler: h(3) },
        { kind: "slider", label: "Level", value: s.level, min: 0, max: 100, step: 1, handler: h(4) },
        { kind: "input", label: "Note", value: s.note, placeholder: "Type", handler: h(5) },
      ],
    };
  }

  /** `reset` is true when the handler was stale: the state starts over and `tree` is the first render. */
  dispatch(handler: string, value: unknown): { tree: unknown; reset: boolean } {
    const match = /^(\d+):(\d)$/.exec(handler);
    if (!match || Number(match[1]) !== this.#generation) {
      this.#state = { ...INITIAL };
      this.#generation = 1;
      return { tree: this.tree(), reset: true };
    }
    const s = this.#state;
    switch (Number(match[2])) {
      case 0:
        if (typeof value === "number" && Number.isInteger(value) && value >= 0 && value < NAMES.length) s.tab = value;
        else throw new RangeError("Invalid value for this visual control");
        break;
      case 1:
        if (value !== null) throw new RangeError("Invalid value for this visual control");
        s.clicks += 1;
        break;
      case 2:
        if (typeof value !== "boolean") throw new RangeError("Invalid value for this visual control");
        s.on = value;
        break;
      case 3:
        if (typeof value === "number" && Number.isInteger(value) && value >= 0 && value < RANGES.length) s.range = value;
        else throw new RangeError("Invalid value for this visual control");
        break;
      case 4:
        if (typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 100) s.level = value;
        else throw new RangeError("Invalid value for this visual control");
        break;
      default:
        if (typeof value !== "string" || value.length > 4096) throw new RangeError("Invalid value for this visual control");
        s.note = value;
    }
    this.events.push({ handler, value });
    this.#generation += 1;
    return { tree: this.tree(), reset: false };
  }
}
