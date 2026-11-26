// Test helper: the shared fixtures in ./fixtures. The graphs are FlowGraph JSON in the desktop's serde format and the
// `.layout.json` goldens are produced by the Rust layout (crates/monocode-core/src/diagram) for the same inputs. The `.mmd` and
// `.parsed.json` fixtures are for the Rust Mermaid parser only: the web app has no flowchart parser (it draws Mermaid with the
// `mermaid` library), so nothing here reads them. The Rust test
// crates/monocode-core/src/diagram/golden.rs fails when the goldens drift and rewrites them with ZPTAH_UPDATE_DIAGRAM_GOLDENS=1.
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import type { FlowGraph } from "./model";

const dir = join(import.meta.dirname, "fixtures");

export type GoldenLayout = {
  width: number;
  height: number;
  nodes: { index: number; rect: [number, number, number, number]; lines: string[] }[];
  edges: { index: number; points: [number, number][]; label: { rect: [number, number, number, number]; text: string } | null }[];
};

export const fixtureNames = (suffix: string): string[] => readdirSync(dir).filter((f) => f.endsWith(suffix)).map((f) => f.slice(0, -suffix.length)).sort();
export const readGraph = (name: string): FlowGraph => JSON.parse(readFileSync(join(dir, `${name}.graph.json`), "utf8")) as FlowGraph;
export const readGolden = (name: string): GoldenLayout | null => (existsSync(join(dir, `${name}.layout.json`)) ? (JSON.parse(readFileSync(join(dir, `${name}.layout.json`), "utf8")) as GoldenLayout) : null);
