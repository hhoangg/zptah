//! Golden layouts shared with the web app. The TypeScript port of the layout
//! and the Mermaid parser (`web/app/src/features/diagram`) is tested against
//! the files in `fixtures/` there: `*.graph.json` and `*.mmd` are inputs,
//! `*.layout.json` and `*.parsed.json` are what this code produces for them.
//! This test keeps the goldens honest: it fails when the Rust output drifts
//! from them (within 0.02 units), so the web port cannot silently diverge.
//!
//! To regenerate after an intended change to the layout or the parser, run
//! `ZPTAH_UPDATE_DIAGRAM_GOLDENS=1 cargo ntest -p monocode-core diagram::golden`
//! from `crates/monocode-gpui`, then run the web tests
//! (`cd web/app && pnpm vitest run src/features/diagram`) and port the change.
use super::layout::layout;
use super::mermaid::parse_flowchart;
use super::{FlowGraph, Rect};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

const TOLERANCE: f64 = 0.02;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/app/src/features/diagram/fixtures")
}

fn dump(graph: &FlowGraph) -> Value {
    let rect = |r: &Rect| json!([r.x, r.y, r.w, r.h]);
    let l = layout(graph).unwrap();
    json!({
        "width": l.width,
        "height": l.height,
        "nodes": l.nodes.iter().map(|n| json!({"index": n.index, "rect": rect(&n.rect), "lines": n.lines})).collect::<Vec<_>>(),
        "edges": l.edges.iter().map(|e| json!({
            "index": e.index,
            "points": e.points,
            "label": e.label.as_ref().map(|x| json!({"rect": rect(&x.rect), "text": x.text})),
        })).collect::<Vec<_>>(),
    })
}

/// Equal structure, numbers within `TOLERANCE`.
fn same(path: &str, a: &Value, b: &Value) -> Result<(), String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            if (x - y).abs() <= TOLERANCE {
                Ok(())
            } else {
                Err(format!("{path}: {x} vs {y}"))
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .try_for_each(|(i, (x, y))| same(&format!("{path}[{i}]"), x, y)),
        (Value::Object(x), Value::Object(y)) if x.len() == y.len() => {
            x.iter().try_for_each(|(k, x)| {
                let y = y.get(k).ok_or(format!("{path}.{k} missing"))?;
                same(&format!("{path}.{k}"), x, y)
            })
        }
        _ if a == b => Ok(()),
        _ => Err(format!("{path}: {a} vs {b}")),
    }
}

fn check(file: PathBuf, actual: Value, update: bool, failures: &mut Vec<String>) {
    if update {
        fs::write(&file, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
        return;
    }
    let text = fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let golden: Value = serde_json::from_str(&text).unwrap();
    if let Err(difference) = same("", &actual, &golden) {
        failures.push(format!("{}: {difference}", file.display()));
    }
}

#[test]
fn rust_output_matches_the_goldens_the_web_port_is_tested_against() {
    let update = std::env::var_os("ZPTAH_UPDATE_DIAGRAM_GOLDENS").is_some();
    let mut failures = Vec::new();
    let mut checked = 0;
    let mut entries: Vec<_> = fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if let Some(stem) = name.strip_suffix(".graph.json") {
            let graph: FlowGraph =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            check(
                dir().join(format!("{stem}.layout.json")),
                dump(&graph),
                update,
                &mut failures,
            );
            checked += 1;
        } else if let Some(stem) = name.strip_suffix(".mmd") {
            let graph = parse_flowchart(&fs::read_to_string(&path).unwrap()).unwrap();
            let actual = json!({"graph": graph, "layout": dump(&graph)});
            check(
                dir().join(format!("{stem}.parsed.json")),
                actual,
                update,
                &mut failures,
            );
            checked += 1;
        }
    }
    assert!(checked >= 7, "fixtures are missing");
    assert!(
        failures.is_empty(),
        "goldens differ (regenerate with ZPTAH_UPDATE_DIAGRAM_GOLDENS=1):\n{}",
        failures.join("\n")
    );
}
