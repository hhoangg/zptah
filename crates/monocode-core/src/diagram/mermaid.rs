//! A parser for the common subset of Mermaid flowcharts, producing the shared
//! `FlowGraph`. Anything outside the subset (subgraphs, styling, class
//! shorthand, `&` groups, exotic shapes or arrows, other diagram types) is an
//! error, and the caller falls back to the general Mermaid renderer.
use super::{Direction, FlowEdge, FlowGraph, FlowNode, NodeKind};
use std::collections::HashMap;

pub fn parse_flowchart(source: &str) -> Result<FlowGraph, String> {
    let mut lines = source
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("%%"));
    let first = lines.next().ok_or("Empty diagram")?;
    // `graph TD; A-->B` carries statements after the header on one line.
    let mut head = split_statements(first).into_iter();
    let header = head.next().ok_or("Empty diagram")?;
    let mut words = header.split_whitespace();
    match words.next() {
        Some("flowchart" | "graph") => {}
        _ => return Err("Not a flowchart".into()),
    }
    let direction = match words.next() {
        None => Direction::TB,
        Some(word) => Direction::parse(word).ok_or("Unsupported flowchart direction")?,
    };
    if words.next().is_some() {
        return Err("Unsupported flowchart header".into());
    }
    let mut graph = FlowGraph {
        direction,
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    let mut index: HashMap<String, usize> = HashMap::new();
    for statement in head {
        parse_statement(&statement, &mut graph, &mut index)?;
    }
    for line in lines {
        for statement in split_statements(line) {
            parse_statement(&statement, &mut graph, &mut index)?;
        }
    }
    graph.validate()?;
    Ok(graph)
}

/// Split on `;` that is outside quotes and brackets.
fn split_statements(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut depth) = (false, 0i32);
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            '[' | '(' | '{' if !quoted => depth += 1,
            ']' | ')' | '}' if !quoted => depth -= 1,
            ';' if !quoted && depth <= 0 => {
                out.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    out.push(current);
    out.into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

struct Cursor {
    chars: Vec<char>,
    at: usize,
}

impl Cursor {
    fn rest_starts(&self, text: &str) -> bool {
        text.chars()
            .enumerate()
            .all(|(i, c)| self.chars.get(self.at + i) == Some(&c))
    }
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }
    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.at += 1;
        }
    }
}

const KEYWORDS: [&str; 8] = [
    "subgraph",
    "end",
    "classDef",
    "class",
    "style",
    "linkStyle",
    "click",
    "direction",
];

fn parse_statement(
    statement: &str,
    graph: &mut FlowGraph,
    index: &mut HashMap<String, usize>,
) -> Result<(), String> {
    let first = statement.split_whitespace().next().unwrap_or("");
    if KEYWORDS.contains(&first) {
        return Err(format!("Unsupported flowchart statement '{first}'"));
    }
    let mut cur = Cursor {
        chars: statement.chars().collect(),
        at: 0,
    };
    let mut previous = parse_node(&mut cur, graph, index)?;
    loop {
        cur.skip_space();
        if cur.peek().is_none() {
            return Ok(());
        }
        let (dashed, arrow, label) = parse_link(&mut cur)?;
        cur.skip_space();
        let next = parse_node(&mut cur, graph, index)?;
        graph.edges.push(FlowEdge {
            from: graph.nodes[previous].id.clone(),
            to: graph.nodes[next].id.clone(),
            label,
            dashed,
            arrow,
        });
        previous = next;
    }
}

fn parse_node(
    cur: &mut Cursor,
    graph: &mut FlowGraph,
    index: &mut HashMap<String, usize>,
) -> Result<usize, String> {
    let start = cur.at;
    while cur.peek().is_some_and(|c| c.is_alphanumeric() || c == '_') {
        cur.at += 1;
    }
    if cur.at == start {
        return Err("Unsupported flowchart syntax".into());
    }
    let id: String = cur.chars[start..cur.at].iter().collect();
    let shape = parse_shape(cur)?;
    if cur.rest_starts(":::") || cur.peek() == Some('&') {
        return Err("Unsupported flowchart syntax".into());
    }
    let slot = match index.get(&id) {
        Some(&slot) => slot,
        None => {
            graph.nodes.push(FlowNode {
                id: id.clone(),
                label: id.clone(),
                kind: NodeKind::Box,
                color: None,
            });
            index.insert(id, graph.nodes.len() - 1);
            graph.nodes.len() - 1
        }
    };
    if let Some((kind, label)) = shape {
        graph.nodes[slot].kind = kind;
        graph.nodes[slot].label = label;
    }
    Ok(slot)
}

fn parse_shape(cur: &mut Cursor) -> Result<Option<(NodeKind, String)>, String> {
    let table: [(&str, &str, NodeKind); 4] = [
        ("([", "])", NodeKind::Start),
        ("((", "))", NodeKind::Circle),
        ("[", "]", NodeKind::Box),
        ("(", ")", NodeKind::Round),
    ];
    if cur.peek() == Some('{') {
        if cur.rest_starts("{{") {
            return Err("Unsupported flowchart shape".into());
        }
        let text = read_until(cur, 1, "}")?;
        return Ok(Some((NodeKind::Decision, clean_label(&text))));
    }
    for (open, close, kind) in table {
        if cur.rest_starts(open) {
            let text = read_until(cur, open.chars().count(), close)?;
            return Ok(Some((kind, clean_label(&text))));
        }
    }
    if cur.peek() == Some('>') {
        return Err("Unsupported flowchart shape".into());
    }
    Ok(None)
}

/// Consume `skip` opening characters, then text up to `close` (outside quotes).
fn read_until(cur: &mut Cursor, skip: usize, close: &str) -> Result<String, String> {
    cur.at += skip;
    let start = cur.at;
    let mut quoted = false;
    while cur.at < cur.chars.len() {
        if cur.chars[cur.at] == '"' {
            quoted = !quoted;
        } else if !quoted && cur.rest_starts(close) {
            let text: String = cur.chars[start..cur.at].iter().collect();
            cur.at += close.chars().count();
            return Ok(text);
        }
        cur.at += 1;
    }
    Err("Unterminated flowchart node text".into())
}

fn clean_label(text: &str) -> String {
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text);
    let mut out = text.to_owned();
    for br in ["<br/>", "<br />", "<br>"] {
        out = out.replace(br, "\n");
    }
    out.replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// Returns (dashed, arrow, label).
fn parse_link(cur: &mut Cursor) -> Result<(bool, bool, Option<String>), String> {
    let unsupported = || Err("Unsupported flowchart link".to_owned());
    let c = cur.peek().ok_or("Unsupported flowchart link")?;
    let (dashed, arrow, mut label) = match c {
        '-' if cur.rest_starts("-.") => {
            // Dotted: -.-> , -.- , or -. text .->
            cur.at += 2;
            if cur.peek() == Some(' ') {
                let text = take_until_any(cur, &[".->", ".-"])?;
                (true, text.1, Some(text.0))
            } else {
                while cur.peek() == Some('.') {
                    cur.at += 1;
                }
                if cur.peek() != Some('-') {
                    return unsupported();
                }
                cur.at += 1;
                let arrow = take_arrow_head(cur);
                (true, arrow, None)
            }
        }
        '-' | '=' => {
            let bar = c;
            let mut count = 0;
            while cur.peek() == Some(bar) {
                cur.at += 1;
                count += 1;
            }
            if count < 2 {
                return unsupported();
            }
            if cur.peek() == Some('>') {
                cur.at += 1;
                (false, true, None)
            } else if count >= 3 {
                (false, false, None)
            } else if cur.peek() == Some(' ') {
                // `-- text -->`, `-- text ---`, `== text ==>`
                let closers: [&str; 2] = if bar == '-' {
                    ["-->", "---"]
                } else {
                    ["==>", "==="]
                };
                let (text, arrow) = take_until_any(cur, &closers)?;
                (false, arrow, Some(text))
            } else {
                return unsupported();
            }
        }
        _ => return unsupported(),
    };
    cur.skip_space();
    if cur.peek() == Some('|') {
        let text = read_until(cur, 1, "|")?;
        label = Some(clean_label(&text));
    }
    Ok((dashed, arrow, label.filter(|l| !l.trim().is_empty())))
}

fn take_arrow_head(cur: &mut Cursor) -> bool {
    if cur.peek() == Some('>') {
        cur.at += 1;
        true
    } else {
        false
    }
}

/// Read link text up to the first closer; a closer ending in `>` is an arrow.
fn take_until_any(cur: &mut Cursor, closers: &[&str]) -> Result<(String, bool), String> {
    let start = cur.at;
    while cur.at < cur.chars.len() {
        for closer in closers {
            if cur.rest_starts(closer) {
                let text: String = cur.chars[start..cur.at].iter().collect();
                cur.at += closer.chars().count();
                return Ok((clean_label(&text), closer.ends_with('>')));
            }
        }
        cur.at += 1;
    }
    Err("Unterminated flowchart link text".into())
}
