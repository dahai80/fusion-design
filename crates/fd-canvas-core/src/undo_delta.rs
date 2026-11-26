//! FUNC-7: delta undo data structures. Replaces the snapshot-style UndoRedoStack that stored a
//! whole PenDocument; instead stores node-level UndoDelta (add/delete/modify), saving memory.
//! Moves are handled as delete+add (MVP, reorder semantics dropped).
//!
//! This is a different concept from the existing PenDocumentDiff in lib.rs (field-level JSON
//! display diff used by the fd-cli diff subcommand), hence the distinct names UndoDelta/NodeChange
//! instead of reusing those.

use crate::{PenDocument, PenNode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoDelta {
    pub page_id: String,
    pub added: Vec<PenNode>,
    pub deleted: Vec<PenNode>,
    pub modified: Vec<NodeChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeChange {
    pub node_id: String,
    pub old: PenNode,
    pub new: PenNode,
}

impl UndoDelta {
    /// Computes the node-level delta from old to new. Single-page MVP (compares first-page nodes).
    pub fn compute(old: &PenDocument, new: &PenDocument) -> Self {
        let old_nodes = active_nodes(old);
        let new_nodes = active_nodes(new);
        let mut added = Vec::new();
        let mut deleted = Vec::new();
        let mut modified = Vec::new();
        let old_map: std::collections::HashMap<&str, &PenNode> =
            old_nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let new_map: std::collections::HashMap<&str, &PenNode> =
            new_nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        for n in new_nodes {
            match old_map.get(n.id.as_str()) {
                None => added.push(n.clone()),
                Some(old_n) if **old_n != *n => modified.push(NodeChange {
                    node_id: n.id.clone(),
                    old: (*old_n).clone(),
                    new: n.clone(),
                }),
                _ => {}
            }
        }
        for n in old_nodes {
            if !new_map.contains_key(n.id.as_str()) {
                deleted.push(n.clone());
            }
        }
        let page_id = new.pages.first().map(|p| p.id.clone()).unwrap_or_default();
        UndoDelta {
            page_id,
            added,
            deleted,
            modified,
        }
    }

    /// Applies the delta forward to doc (used by push/redo). Add → insert node, delete → remove
    /// node, modify → replace node.
    pub fn apply(&self, doc: &mut PenDocument) {
        let page = match doc.pages.iter_mut().find(|p| p.id == self.page_id) {
            Some(p) => p,
            None => {
                tracing::warn!(page_id = %self.page_id, "UndoDelta apply target page not found, skipping");
                return;
            }
        };
        for n in &self.deleted {
            remove_node_by_id(&mut page.nodes, &n.id);
        }
        for ch in &self.modified {
            replace_node_by_id(&mut page.nodes, &ch.node_id, ch.new.clone());
        }
        for n in &self.added {
            page.nodes.push(n.clone());
        }
    }

    /// Applies the delta in reverse (used by undo). Add → remove, delete → insert, modify →
    /// restore old.
    pub fn apply_reverse(&self, doc: &mut PenDocument) {
        let page = match doc.pages.iter_mut().find(|p| p.id == self.page_id) {
            Some(p) => p,
            None => {
                tracing::warn!(page_id = %self.page_id, "UndoDelta apply_reverse target page not found, skipping");
                return;
            }
        };
        // Reverse-order undo: remove added first, restore modified.old next, re-add deleted last.
        for n in &self.added {
            remove_node_by_id(&mut page.nodes, &n.id);
        }
        for ch in &self.modified {
            replace_node_by_id(&mut page.nodes, &ch.node_id, ch.old.clone());
        }
        for n in &self.deleted {
            page.nodes.push(n.clone());
        }
    }
}

fn active_nodes(doc: &PenDocument) -> &[PenNode] {
    doc.pages.first().map(|p| p.nodes.as_slice()).unwrap_or(&[])
}

#[allow(clippy::ptr_arg)]
fn remove_node_by_id(nodes: &mut Vec<PenNode>, id: &str) {
    nodes.retain(|n| n.id != id);
    for n in nodes.iter_mut() {
        if !n.children.is_empty() {
            remove_node_by_id(&mut n.children, id);
        }
    }
}

#[allow(clippy::ptr_arg)]
fn replace_node_by_id(nodes: &mut Vec<PenNode>, id: &str, replacement: PenNode) {
    for n in nodes.iter_mut() {
        if n.id == id {
            *n = replacement;
            return;
        }
        if !n.children.is_empty() {
            replace_node_by_id(&mut n.children, id, replacement.clone());
        }
    }
}
