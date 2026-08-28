//! FUNC-7：delta undo 数据结构。替代快照式 UndoRedoStack 存整 PenDocument，
//! 改存节点级 UndoDelta（增/删/改），节省内存。move 按 delete+add 处理（MVP，丢 reorder 语义）。
//!
//! 与 lib.rs 既有的 PenDocumentDiff（字段级 JSON 展示 diff，fd-cli diff 子命令用）是不同概念，
//! 故独立命名 UndoDelta/NodeChange，不复用其名。

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
    /// 计算 old → new 的节点级 delta。单页 MVP（取首页 nodes 对比）。
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

    /// 正向应用 delta 到 doc（push/redo 用）。增→加节点，删→移节点，改→替换节点。
    pub fn apply(&self, doc: &mut PenDocument) {
        let page = match doc.pages.iter_mut().find(|p| p.id == self.page_id) {
            Some(p) => p,
            None => {
                tracing::warn!(page_id = %self.page_id, "UndoDelta apply 目标页不存在，跳过");
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

    /// 反向应用 delta（undo 用）。增→移，删→加，改→换回 old。
    pub fn apply_reverse(&self, doc: &mut PenDocument) {
        let page = match doc.pages.iter_mut().find(|p| p.id == self.page_id) {
            Some(p) => p,
            None => {
                tracing::warn!(page_id = %self.page_id, "UndoDelta apply_reverse 目标页不存在，跳过");
                return;
            }
        };
        // 逆序撤销：先移 added、再换回 modified.old、最后补回 deleted。
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
