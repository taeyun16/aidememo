//! `aidememo graph` — emit the entity graph as Mermaid or Graphviz DOT.
//!
//! Default scope: traverse from `--from <ENTITY>` up to `--depth N` (default 2).
//! If `--from` is omitted, dumps the entire graph (capped by `--limit`).

use aidememo_core::{AideMemo, AideMemoError, Config, ListOpts, TraverseDirection, TraverseOpts};
use bpaf::*;
use std::collections::HashSet;
use std::path::Path;

use crate::cmd::Command;

#[derive(Debug, Clone)]
pub struct GraphSub {
    pub format: Option<String>,
    pub from: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<usize>,
}

pub fn graph_command() -> impl Parser<Command> {
    let format = long("format")
        .short('f')
        .help("Output format: mermaid (default) | dot")
        .argument::<String>("FORMAT")
        .optional();
    let from = long("from")
        .help("Start from this entity; omit to dump the whole graph")
        .argument::<String>("ENTITY")
        .optional();
    let depth = long("depth")
        .short('d')
        .help("Traverse depth when --from is given (default 2)")
        .argument::<u32>("DEPTH")
        .optional();
    let limit = long("limit")
        .short('l')
        .help("Max entities when --from is omitted (default 100)")
        .argument::<usize>("LIMIT")
        .optional();

    construct!(GraphSub {
        format,
        from,
        depth,
        limit
    })
    .map(Command::Graph)
    .to_options()
    .command("graph")
    .help("Render the entity graph as Mermaid or DOT")
}

#[derive(Clone, Copy)]
enum Format {
    Mermaid,
    Dot,
}

pub fn run_graph(
    store_path: &Path,
    config: Config,
    sub: GraphSub,
) -> Result<String, AideMemoError> {
    let format = match sub.format.as_deref() {
        Some("dot") | Some("DOT") | Some("graphviz") => Format::Dot,
        _ => Format::Mermaid,
    };

    let wiki = AideMemo::open(store_path, config)?;

    // Collect (entity_name, entity_type) and (source, target, rel_type) edges.
    let mut nodes: Vec<(String, String)> = Vec::new();
    let mut node_keys: HashSet<String> = HashSet::new();
    let mut edges: Vec<(String, String, String)> = Vec::new();
    let mut edge_keys: HashSet<String> = HashSet::new();

    let add_node =
        |name: &str, ty: &str, nodes: &mut Vec<(String, String)>, keys: &mut HashSet<String>| {
            if keys.insert(name.to_string()) {
                nodes.push((name.to_string(), ty.to_string()));
            }
        };

    if let Some(start) = sub.from {
        let depth = sub.depth.unwrap_or(2);
        let result = wiki.traverse(
            &start,
            TraverseOpts {
                depth,
                relation_types: None,
                direction: TraverseDirection::Both,
            },
        )?;
        for e in &result.entities {
            add_node(
                &e.name,
                &e.entity_type.to_string(),
                &mut nodes,
                &mut node_keys,
            );
        }
        for r in &result.relations {
            let from_name = wiki
                .entity_get_by_id(r.source_id)
                .map(|e| e.name)
                .unwrap_or_default();
            let to_name = wiki
                .entity_get_by_id(r.target_id)
                .map(|e| e.name)
                .unwrap_or_default();
            let key = format!("{}->{}->{}", from_name, r.relation_type, to_name);
            if !from_name.is_empty() && !to_name.is_empty() && edge_keys.insert(key) {
                edges.push((from_name, to_name, r.relation_type.to_string()));
            }
        }
    } else {
        // Whole-graph dump capped by --limit.
        let limit = sub.limit.unwrap_or(100);
        let entities = wiki.entity_list(ListOpts {
            entity_type: None,
            min_facts: None,
            limit: Some(limit),
            sort_by: Default::default(),
            offset: 0,
        })?;
        for e in &entities {
            add_node(
                &e.name,
                &e.entity_type.to_string(),
                &mut nodes,
                &mut node_keys,
            );
            // pull both directions of relations for each entity
            let rels = wiki.relations_get(&e.name, TraverseDirection::Forward)?;
            for r in rels {
                let target_name = wiki
                    .entity_get_by_id(r.target_id)
                    .map(|t| t.name)
                    .unwrap_or_default();
                if target_name.is_empty() {
                    continue;
                }
                let key = format!("{}->{}->{}", e.name, r.relation_type, target_name);
                if edge_keys.insert(key) {
                    edges.push((e.name.clone(), target_name, r.relation_type.to_string()));
                }
            }
        }
    }

    Ok(match format {
        Format::Mermaid => render_mermaid(&nodes, &edges),
        Format::Dot => render_dot(&nodes, &edges),
    })
}

fn safe_id(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        out.insert(0, 'n');
    }
    out
}

fn render_mermaid(nodes: &[(String, String)], edges: &[(String, String, String)]) -> String {
    let mut out = String::from("graph LR\n");
    for (name, ty) in nodes {
        let id = safe_id(name);
        out.push_str(&format!("    {id}[\"{name}<br/><i>{ty}</i>\"]\n"));
    }
    for (from, to, rel) in edges {
        out.push_str(&format!(
            "    {} -->|{}| {}\n",
            safe_id(from),
            rel,
            safe_id(to)
        ));
    }
    out
}

fn render_dot(nodes: &[(String, String)], edges: &[(String, String, String)]) -> String {
    let mut out =
        String::from("digraph aidememo {\n  rankdir=LR;\n  node [shape=box, style=rounded];\n");
    for (name, ty) in nodes {
        out.push_str(&format!(
            "  {} [label=\"{}\\n[{}]\"];\n",
            safe_id(name),
            name,
            ty
        ));
    }
    for (from, to, rel) in edges {
        out.push_str(&format!(
            "  {} -> {} [label=\"{}\"];\n",
            safe_id(from),
            safe_id(to),
            rel
        ));
    }
    out.push_str("}\n");
    out
}
