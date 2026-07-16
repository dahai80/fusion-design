//! Fusion-Design 画布内核 — 自研轻量矢量画布模型。
//!
//! 设计决策：OpenPencil 的 4 个核心 crate 强依赖私有底座 `jian-ops-schema`
//! （`vendor/jian/` 上游未拉取，本地空目录）。fusion-design MVP 不自研
//! jian 替身（工程量大且偏离目标），改为定义本原生轻量画布模型，
//! 与已有 `fd-export`/`fd-design-system`/`fd-ecosystem` 直接对接。
//!
//! 模型覆盖 MVP 必需：PenDocument/PenNode/page/node/style/sizing/variable，
//! 形状参照 OpenPencil `jian_ops_schema` 的公开引用，但裁剪至 fusion-design 用途。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── 文档/页面/节点 ──

/// 画布文档（顶层工程文件 `.fusiondesign` 的结构体）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PenDocument {
    pub pages: Vec<Page>,
    pub variables: Option<HashMap<String, VariableDef>>,
    /// 当前激活的设计规范 ID（对接 fd-design-system）。
    pub active_design_system: Option<String>,
}

/// 页面（一画板）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: String,
    pub name: String,
    pub width: f32,
    pub height: f32,
    pub nodes: Vec<PenNode>,
}

/// �矢量节点（统一形状，覆盖 rect/circle/text/image/group）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PenNode {
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub style: NodeStyle,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub children: Vec<PenNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Rect,
    Circle,
    Text,
    Image,
    Group,
}

// ── 样式 ──

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
}

// ── 变量（设计 Token 引用） ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableDef {
    pub value: VariableValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VariableValue {
    Scalar(VariableScalar),
    /// 主题化值（mode=dark/light 等多套值）。
    Themed(Vec<ThemedEntry>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariableScalar {
    pub kind: ScalarKind,
    pub raw: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScalarKind {
    Color,
    Number,
    String,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemedEntry {
    pub value: VariableScalar,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<HashMap<String, String>>,
}

// ── 文档操作 API ──

impl PenDocument {
    /// 创建空文档。
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加一个页面。
    pub fn add_page(&mut self, page: Page) {
        self.pages.push(page);
    }

    /// 按 ID 查找页面。
    pub fn page(&self, id: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.id == id)
    }

    /// 按 ID 查找页面（可变）。
    pub fn page_mut(&mut self, id: &str) -> Option<&mut Page> {
        self.pages.iter_mut().find(|p| p.id == id)
    }

    /// 按 ID 查找节点（递归，全部页面）。
    pub fn find_node(&self, node_id: &str) -> Option<&PenNode> {
        self.pages
            .iter()
            .flat_map(|p| p.nodes.iter())
            .find_map(|n| n.find(node_id))
    }

    /// 按 ID 查找节点（可变，递归）。
    pub fn find_node_mut(&mut self, node_id: &str) -> Option<&mut PenNode> {
        self.pages
            .iter_mut()
            .flat_map(|p| p.nodes.iter_mut())
            .find_map(|n| n.find_mut(node_id))
    }

    /// 删除节点（返回是否删除成功）。
    pub fn remove_node(&mut self, node_id: &str) -> bool {
        for page in &mut self.pages {
            if remove_node_from_list(&mut page.nodes, node_id) {
                return true;
            }
        }
        false
    }

    /// 序列化为 `.fusiondesign` JSON。
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// 从 JSON 反序列化。
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}

impl Page {
    pub fn new(id: impl Into<String>, name: impl Into<String>, w: f32, h: f32) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            width: w,
            height: h,
            nodes: vec![],
        }
    }

    /// 添加节点。
    pub fn add(&mut self, node: PenNode) {
        self.nodes.push(node);
    }
}

impl PenNode {
    /// 创建 rect 节点。
    pub fn rect(id: impl Into<String>, x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            id: id.into(),
            kind: NodeKind::Rect,
            name: "Rect".into(),
            x, y, w, h,
            style: NodeStyle::default(),
            text: None,
            children: vec![],
        }
    }

    /// 创建 text 节点。
    pub fn text(id: impl Into<String>, x: f32, y: f32, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: NodeKind::Text,
            name: "Text".into(),
            x, y, w: 0.0, h: 0.0,
            style: NodeStyle::default(),
            text: Some(content.into()),
            children: vec![],
        }
    }

    /// 创建 group 节点。
    pub fn group(id: impl Into<String>, x: f32, y: f32, children: Vec<PenNode>) -> Self {
        Self {
            id: id.into(),
            kind: NodeKind::Group,
            name: "Group".into(),
            x, y, w: 0.0, h: 0.0,
            style: NodeStyle::default(),
            text: None,
            children,
        }
    }

    /// 递归查找自身或子孙节点。
    pub fn find(&self, id: &str) -> Option<&PenNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    /// 递归查找（可变）。
    pub fn find_mut(&mut self, id: &str) -> Option<&mut PenNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }

    /// 全部子孙节点 ID（含自身）。
    pub fn descendant_ids(&self) -> Vec<String> {
        let mut ids = vec![self.id.clone()];
        for c in &self.children {
            ids.extend(c.descendant_ids());
        }
        ids
    }
}

fn remove_node_from_list(nodes: &mut Vec<PenNode>, id: &str) -> bool {
    let before = nodes.len();
    nodes.retain(|n| n.id != id);
    if nodes.len() != before {
        return true;
    }
    nodes.iter_mut().any(|n| remove_node_from_list(&mut n.children, id))
}

// ── 错误 ──

#[derive(Debug, thiserror::Error)]
pub enum CanvasError {
    #[error("节点 {0} 未找到")]
    NodeNotFound(String),
    #[error("页面 {0} 未找到")]
    PageNotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc() -> PenDocument {
        let mut doc = PenDocument::new();
        let mut page = Page::new("p1", "Home", 100.0, 100.0);
        page.add(PenNode::rect("n1", 0.0, 0.0, 50.0, 50.0));
        page.add(PenNode::text("n2", 10.0, 20.0, "hello"));
        doc.add_page(page);
        doc
    }

    #[test]
    fn doc_add_and_find_page() {
        let doc = sample_doc();
        assert!(doc.page("p1").is_some());
        assert!(doc.page("nope").is_none());
    }

    #[test]
    fn doc_find_node_recursive() {
        let doc = sample_doc();
        assert!(doc.find_node("n1").is_some());
        assert!(doc.find_node("n2").is_some());
        assert!(doc.find_node("nope").is_none());
    }

    #[test]
    fn doc_find_node_mut() {
        let mut doc = sample_doc();
        doc.find_node_mut("n1").unwrap().x = 99.0;
        assert_eq!(doc.find_node("n1").unwrap().x, 99.0);
    }

    #[test]
    fn doc_remove_node_top_level() {
        let mut doc = sample_doc();
        assert!(doc.remove_node("n1"));
        assert!(doc.find_node("n1").is_none());
    }

    #[test]
    fn doc_remove_node_nested() {
        let mut doc = PenDocument::new();
        let mut page = Page::new("p", "P", 100.0, 100.0);
        let mut g = PenNode::group("g", 0.0, 0.0, vec![]);
        g.children.push(PenNode::rect("inner", 0.0, 0.0, 10.0, 10.0));
        page.add(g);
        doc.add_page(page);
        assert!(doc.remove_node("inner"));
        assert!(doc.find_node("inner").is_none());
        // parent g still present
        assert!(doc.find_node("g").is_some());
    }

    #[test]
    fn doc_remove_nonexistent_returns_false() {
        let mut doc = sample_doc();
        assert!(!doc.remove_node("nope"));
    }

    #[test]
    fn node_descendant_ids_includes_self() {
        let g = PenNode::group(
            "g",
            0.0,
            0.0,
            vec![PenNode::rect("c1", 0.0, 0.0, 1.0, 1.0)],
        );
        assert_eq!(g.descendant_ids(), vec!["g", "c1"]);
    }

    #[test]
    fn node_find_in_children() {
        let g = PenNode::group(
            "g",
            0.0,
            0.0,
            vec![PenNode::rect("c1", 0.0, 0.0, 1.0, 1.0)],
        );
        assert!(g.find("c1").is_some());
        assert!(g.find("c2").is_none());
    }

    #[test]
    fn doc_json_roundtrip() {
        let doc = sample_doc();
        let json = doc.to_json().unwrap();
        let doc2 = PenDocument::from_json(&json).unwrap();
        assert_eq!(doc2.pages.len(), 1);
        assert_eq!(doc2.pages[0].id, "p1");
    }

    #[test]
    fn doc_from_json_invalid() {
        assert!(PenDocument::from_json("not json").is_err());
    }

    #[test]
    fn node_style_serde_skip_none() {
        let n = PenNode::rect("r", 0.0, 0.0, 10.0, 10.0);
        let s = serde_json::to_string(&n).unwrap();
        // 默认 NodeStyle 全 None，应被 skip
        assert!(!s.contains("\"fill\""));
    }

    #[test]
    fn variable_scalar_serde() {
        let v = VariableValue::Scalar(VariableScalar {
            kind: ScalarKind::Color,
            raw: "#FFF".into(),
        });
        let s = serde_json::to_string(&v).unwrap();
        let v2: VariableValue = serde_json::from_str(&s).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn page_new_constructs() {
        let p = Page::new("id", "Name", 100.0, 200.0);
        assert_eq!(p.id, "id");
        assert_eq!(p.width, 100.0);
        assert!(p.nodes.is_empty());
    }

    #[test]
    fn node_text_constructor() {
        let n = PenNode::text("t", 0.0, 0.0, "hi");
        assert_eq!(n.kind, NodeKind::Text);
        assert_eq!(n.text.as_deref(), Some("hi"));
    }
}
