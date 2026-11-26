//! Iterate prompt assembly (issue#34: stable-prefix + region externalization +
//! changelog).
//!
//! Ordering contract (paired with fusion-mlx `enable_prefix_cache`):
//! 1. sys stays constant — the stable head the prefix cache can hit (verify hit
//!    rate via mlx admin `prefix_cache` aggregate metrics);
//! 2. within user: fixed template head -> document TOC (structure summary) ->
//!    changelog -> region node JSON -> this instruction. Volatile content always
//!    goes last so stable content is not polluted by per-request content.
//!
//! Large-document iteration uses region selection: send only region node JSON +
//! full TOC (O(region) instead of O(doc)); the model contract is still to output
//! the full page JSON, with unlisted nodes kept unchanged by the server.

use crate::state::Session;

/// Iterate system prompt (constant; do not append per-request content — stable
/// head for the prefix cache).
pub const ITERATE_SYS: &str = "You are the fusion-design UI iterator. Input is the current \
page JSON plus a modification instruction; output the modified full page JSON \
({\"page\":{...}}, same schema). Keep width/height and all unmentioned nodes \
unchanged; only apply what the instruction requires. Output JSON only, no \
extra text.";

/// Document TOC: one line per node `id kind name`, aggregated across pages.
/// Returns an empty string on parse failure (tolerated).
pub fn toc_of(doc_json: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(doc_json) {
        if let Some(pages) = v.get("pages").and_then(|p| p.as_array()) {
            for p in pages {
                let page = p.get("name").and_then(|n| n.as_str()).unwrap_or("page");
                if let Some(nodes) = p.get("nodes").and_then(|n| n.as_array()) {
                    for n in nodes {
                        let id = n.get("id").and_then(|x| x.as_str()).unwrap_or("?");
                        let kind = n.get("kind").and_then(|x| x.as_str()).unwrap_or("?");
                        let name = n.get("name").and_then(|x| x.as_str()).unwrap_or("-");
                        lines.push(format!("- {page}/{id} {kind} {name}"));
                    }
                }
            }
        }
    }
    lines.join("\n")
}

/// Region selection: filters each page's nodes by node id, keeping the page
/// shell (width/height/name). Returns the filtered page JSON; None when nothing
/// matches (caller returns 400, fail visibly).
pub fn select_region(doc_json: &str, node_ids: &[String]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(doc_json).ok()?;
    let mut out = Vec::new();
    if let Some(pages) = v.get("pages").and_then(|p| p.as_array()) {
        for p in pages {
            let nodes = p.get("nodes").and_then(|n| n.as_array())?;
            let kept: Vec<&serde_json::Value> = nodes
                .iter()
                .filter(|n| {
                    n.get("id")
                        .and_then(|x| x.as_str())
                        .map(|id| node_ids.iter().any(|r| r == id))
                        .unwrap_or(false)
                })
                .collect();
            if kept.is_empty() {
                continue;
            }
            let mut page = p.clone();
            page["nodes"] = serde_json::Value::Array(kept.into_iter().cloned().collect());
            out.push(page);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(serde_json::json!({ "pages": out }).to_string())
    }
}

/// Changelog: takes version labels (generation/iteration instructions,
/// truncated), the most recent 8 one per line; earlier ones folded into a single
/// summary line. The first version (initial/restore) counts as one background
/// line.
pub fn changelog_of(sess: &Session) -> String {
    let mut lines: Vec<String> = Vec::new();
    let total = sess.versions.len();
    let skip = total.saturating_sub(8);
    if skip > 1 {
        lines.push(format!("- ({} earlier versions before these)", skip));
    }
    for v in sess.versions.iter().skip(skip) {
        lines.push(format!("- v{}: {}", v.version, v.label));
    }
    lines.join("\n")
}

/// Iterate user prompt. Fixed ordering: TOC -> changelog -> base JSON ->
/// instruction (volatile last).
/// When `region` is non-empty the base is the region JSON (O(region)), and a
/// note before the instruction declares that unlisted nodes stay unchanged.
pub fn build_iterate_user(
    doc_json: &str,
    region: Option<&[String]>,
    changelog: &str,
    instruction: &str,
) -> String {
    let toc = toc_of(doc_json);
    let (base_note, base) = match region {
        Some(ids) if !ids.is_empty() => match select_region(doc_json, ids) {
            Some(json) => (
                "Below are only the region nodes to modify this time. Output \
                 the modified result for these nodes only: \
                 {\"page\":{\"nodes\":[...]}} (keep their ids, do not output \
                 other nodes; the server will merge the result back into the \
                 full page, and unlisted nodes are guaranteed unchanged by the \
                 server)"
                    .to_string(),
                json,
            ),
            None => ("Current page:".to_string(), doc_json.to_string()),
        },
        _ => ("Current page:".to_string(), doc_json.to_string()),
    };
    format!(
        "Document structure (TOC):\n{toc}\n\nRecent changes:\n{cl}\n\n{note}\n{base}\n\nModification instruction: {ins}",
        toc = if toc.is_empty() {
            "(empty document)"
        } else {
            &toc
        },
        cl = if changelog.is_empty() {
            "(none)"
        } else {
            changelog
        },
        note = base_note,
        base = base,
        ins = instruction,
    )
}

/// Merges region results back into the full page: nodes in base hit by region
/// are replaced in order by model output nodes (extra output nodes appended to
/// the first page's tail), unhit nodes kept as-is. Deterministic assembly
/// (Rule 5: decisions in code, not the model), the model cannot echo back nodes
/// it never saw.
pub fn merge_region_nodes(
    base_json: &str,
    model_nodes: &[serde_json::Value],
    region_ids: &[String],
) -> Option<String> {
    let mut base: serde_json::Value =
        serde_json::from_str(base_json).unwrap_or_else(|_| serde_json::json!({ "pages": [] }));
    // Positional matching: the model is asked to output only region nodes (in
    // base order), positions correspond one-to-one; extra nodes (no matching
    // base slot) are appended to the first page's tail.
    let mut positional: Vec<serde_json::Value> = model_nodes.to_vec();
    if model_nodes.is_empty() {
        return None;
    }
    let mut replaced = 0usize;
    if let Some(pages) = base.get_mut("pages").and_then(|p| p.as_array_mut()) {
        for page in pages.iter_mut() {
            if let Some(nodes) = page.get_mut("nodes").and_then(|n| n.as_array_mut()) {
                let mut out: Vec<serde_json::Value> = Vec::with_capacity(nodes.len());
                for n in nodes.drain(..) {
                    let id = n.get("id").and_then(|x| x.as_str()).unwrap_or("");
                    if region_ids.iter().any(|r| r == id) {
                        // When the model explicitly deletes the node there is no
                        // replacement candidate (repl None, original node dropped).
                        if let Some(v) = (!positional.is_empty()).then(|| positional.remove(0)) {
                            replaced += 1;
                            out.push(v);
                        }
                    } else {
                        out.push(n);
                    }
                }
                *nodes = out;
            }
        }
        if !positional.is_empty() {
            if let Some(first) = pages.first_mut() {
                if let Some(nodes) = first.get_mut("nodes").and_then(|n| n.as_array_mut()) {
                    nodes.append(&mut positional);
                }
            }
        }
    }
    tracing::info!(
        requested = region_ids.len(),
        replaced,
        "merge_region_nodes: region merge complete"
    );
    Some(base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc() -> String {
        serde_json::json!({
            "pages": [{
                "id": "p1", "name": "home", "width": 1440.0, "height": 900.0,
                "nodes": [
                    {"id": "n1", "kind": "rect", "name": "bg", "x": 0.0, "y": 0.0, "w": 100.0, "h": 50.0},
                    {"id": "n2", "kind": "text", "name": "title", "x": 10.0, "y": 10.0, "w": 80.0, "h": 20.0},
                    {"id": "n3", "kind": "circle", "name": "avatar", "x": 20.0, "y": 20.0, "w": 30.0, "h": 30.0}
                ]
            }]
        })
        .to_string()
    }

    #[test]
    fn toc_lists_nodes_with_page_prefix() {
        let toc = toc_of(&sample_doc());
        assert!(toc.contains("home/n1 rect bg"), "{toc}");
        assert!(toc.contains("home/n3 circle avatar"), "{toc}");
        assert_eq!(toc.lines().count(), 3);
        assert_eq!(toc_of("not json"), "");
    }

    #[test]
    fn region_selects_only_matching_nodes() {
        let r = select_region(&sample_doc(), &["n2".to_string()]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        let nodes = v["pages"][0]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1, "{r}");
        assert_eq!(nodes[0]["id"], "n2");
        // Page shell kept (width/height), so the size contract survives when the
        // model outputs a whole page.
        assert_eq!(v["pages"][0]["width"], 1440.0);
        assert_eq!(select_region(&sample_doc(), &["nope".to_string()]), None);
    }

    #[test]
    fn changelog_folds_old_versions() {
        let mut sess = crate::state::Session {
            id: "s-t".into(),
            created_at: "t".into(),
            versions: vec![],
            current: 1,
            last_used: 0,
        };
        for i in 1..=12 {
            sess.versions.push(crate::state::VersionEntry {
                version: i,
                label: format!("iter: instruction{i}"),
                created_at: "t".into(),
                node_count: 1,
                document: "{}".into(),
            });
        }
        let cl = changelog_of(&sess);
        assert!(cl.contains("4 earlier versions before these"), "{cl}");
        assert!(cl.contains("v12: iter: instruction12"), "{cl}");
        assert!(!cl.contains("instruction4"), "{cl}");
    }

    #[test]
    fn merge_region_replaces_only_selected() {
        let base = sample_doc();
        let model_nodes = vec![serde_json::json!({
            "id": "n2", "kind": "text", "name": "title", "x": 10.0, "y": 10.0,
            "w": 120.0, "h": 30.0, "text": "BIG"
        })];
        let merged = merge_region_nodes(&base, &model_nodes, &["n2".to_string()]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let nodes = v["pages"][0]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3, "other nodes kept: {merged}");
        assert_eq!(nodes[1]["text"], "BIG", "n2 replaced by model result");
        assert_eq!(nodes[0]["id"], "n1", "unselected node kept as-is");
        assert_eq!(nodes[2]["id"], "n3", "unselected node kept as-is");
        // Model-added nodes appended to the first page's tail.
        let extra = vec![
            serde_json::json!({"id": "n2", "kind": "text", "name": "t2", "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0}),
            serde_json::json!({"id": "n9", "kind": "rect", "name": "new", "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0}),
        ];
        let merged2 = merge_region_nodes(&base, &extra, &["n2".to_string()]).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&merged2).unwrap();
        let nodes2 = v2["pages"][0]["nodes"].as_array().unwrap();
        assert_eq!(nodes2.len(), 4, "n2 replaced + n9 appended: {merged2}");
        assert_eq!(nodes2[3]["id"], "n9");
    }

    #[test]
    fn merge_region_without_replacements_fails() {
        // Model returns zero nodes -> None (caller 502; never silently drop nodes).
        assert!(merge_region_nodes(&sample_doc(), &[], &["n2".to_string()]).is_none());
    }

    #[test]
    fn iterate_user_orders_stable_to_volatile() {
        let doc = sample_doc();
        let user = build_iterate_user(
            &doc,
            Some(&["n2".to_string()]),
            "- v2: add dark mode",
            "make the title bigger",
        );
        let pos_toc = user.find("Document structure").unwrap();
        let pos_cl = user.find("Recent changes").unwrap();
        let pos_region = user.find("region nodes").unwrap();
        let pos_ins = user.find("Modification instruction").unwrap();
        assert!(
            pos_toc < pos_cl && pos_cl < pos_region && pos_region < pos_ins,
            "{user}"
        );
        // Region mode: base JSON contains only the n2 node object (TOC mentions
        // don't count, O(region)).
        assert_eq!(
            user.matches("\"kind\"").count(),
            1,
            "region JSON must not carry unselected node objects: {user}"
        );
        assert!(user.contains("merge the result back into the"), "{user}");
        // No region: falls back to the full document.
        let full = build_iterate_user(&doc, None, "", "change background color");
        assert!(full.contains("Current page"), "{full}");
        assert!(full.contains("\"avatar\""), "{full}");
    }
}
