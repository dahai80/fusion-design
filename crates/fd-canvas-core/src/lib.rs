//! Fusion-Design 画布内核 — 自研轻量矢量画布模型。
//!
//! 设计决策：OpenPencil 的 4 个核心 crate 强依赖私有底座 `jian-ops-schema`
//! （`vendor/jian/` 上游未拉取，本地空目录）。fusion-design MVP 不自研
//! jian 替身（工程量大且偏离目标），改为定义本原生轻量画布模型，
//! 与已有 `fd-export`/`fd-design-system`/`fd-ecosystem` 直接对接。
//!
//! 模型覆盖 MVP 必需：PenDocument/PenNode/page/node/style/sizing/variable，
//! 形状参照 OpenPencil `jian_ops_schema` 的公开引用，但裁剪至 fusion-design 用途。
//!
//! V0.2 扩展：LayoutMode(Flex/Grid) + Taffy 布局引擎 + ComponentSlot + Token 引用。
// Callers: fd-host-web (DOM render), fd-codegen (code gen), fd-export (export)
// Affected API: NodeStyle (new fields), PenNode (rotation/z_index), PenDocument::compute_layout()
// Data schemas: LayoutMode/FlexParams/GridParams/TrackSizing/ComponentSlot/ComputedLayout
// User instruction: "现在开始实施"

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
    #[serde(default)]
    pub rotation: f32,
    #[serde(default)]
    pub z_index: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Rect,
    Circle,
    Text,
    Image,
    Group,
}

// ── 布局 ──

/// 节点布局模式。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum LayoutMode {
    /// 自由定位（绝对坐标）。
    #[default]
    Free,
    /// Flexbox 布局。
    Flex(FlexParams),
    /// CSS Grid 布局。
    Grid(GridParams),
}

/// Flexbox 布局参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlexParams {
    pub direction: FlexDirection,
    pub align_items: AlignItems,
    pub justify_content: JustifyContent,
    pub wrap: FlexWrap,
    pub gap: f32,
    pub padding: SideOffsets,
}

impl Default for FlexParams {
    fn default() -> Self {
        Self {
            direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
            justify_content: JustifyContent::Start,
            wrap: FlexWrap::NoWrap,
            gap: 0.0,
            padding: SideOffsets::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlexDirection {
    #[default]
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlignItems {
    #[default]
    Stretch,
    Start,
    End,
    Center,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum JustifyContent {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
}

/// 四边偏移量（padding/margin）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct SideOffsets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl SideOffsets {
    pub fn uniform(v: f32) -> Self {
        Self { top: v, right: v, bottom: v, left: v }
    }
}

/// CSS Grid 布局参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridParams {
    pub columns: Vec<TrackSizing>,
    pub rows: Vec<TrackSizing>,
    pub gap: (f32, f32),
    pub areas: Vec<GridArea>,
}

impl Default for GridParams {
    fn default() -> Self {
        Self {
            columns: vec![TrackSizing::Auto],
            rows: vec![TrackSizing::Auto],
            gap: (0.0, 0.0),
            areas: vec![],
        }
    }
}

/// Grid 轨道尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TrackSizing {
    Fixed(f32),
    Auto,
    Flex(f32),
    Percent(f32),
}

/// Grid 区域命名。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridArea {
    pub name: String,
    pub row_start: u32,
    pub row_end: u32,
    pub col_start: u32,
    pub col_end: u32,
}

// ── 组件实例 ──

/// 组件插槽：引用设计系统中的组件并支持覆写。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentSlot {
    pub component_id: String,
    pub variant: String,
    #[serde(default)]
    pub overrides: HashMap<String, serde_json::Value>,
}

/// 组件定义：可被 ComponentSlot 引用的模板节点树。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentDefinition {
    pub id: String,
    pub name: String,
    pub variants: HashMap<String, PenNode>,
}

/// 组件注册中心：存储组件定义，支持实例化。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComponentRegistry {
    pub components: HashMap<String, ComponentDefinition>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, def: ComponentDefinition) {
        self.components.insert(def.id.clone(), def);
    }

    pub fn get(&self, id: &str) -> Option<&ComponentDefinition> {
        self.components.get(id)
    }

    /// 实例化组件：从 ComponentSlot 创建节点树副本，并应用覆写。
    pub fn instantiate(&self, slot: &ComponentSlot) -> Option<PenNode> {
        let def = self.components.get(&slot.component_id)?;
        let template = def.variants.get(&slot.variant)?;
        let mut instance = template.clone();
        apply_overrides(&mut instance, &slot.overrides);
        Some(instance)
    }
}

fn apply_overrides(node: &mut PenNode, overrides: &HashMap<String, serde_json::Value>) {
    // 按路径覆写，如 "fill" → node.style.fill, "text" → node.text
    for (key, val) in overrides {
        match key.as_str() {
            "fill" => {
                if let Some(s) = val.as_str() {
                    node.style.fill = Some(s.to_string());
                }
            }
            "stroke" => {
                if let Some(s) = val.as_str() {
                    node.style.stroke = Some(s.to_string());
                }
            }
            "radius" => {
                if let Some(f) = val.as_f64() {
                    node.style.radius = Some(f as f32);
                }
            }
            "text" => {
                if let Some(s) = val.as_str() {
                    node.text = Some(s.to_string());
                }
            }
            "w" => {
                if let Some(f) = val.as_f64() {
                    node.w = f as f32;
                }
            }
            "h" => {
                if let Some(f) = val.as_f64() {
                    node.h = f as f32;
                }
            }
            _ => {
                tracing::debug!(key = %key, "apply_overrides: 未知覆写键，忽略");
            }
        }
    }
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
    #[serde(default)]
    pub layout: LayoutMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_slot: Option<ComponentSlot>,
    #[serde(default)]
    pub design_token_refs: HashMap<String, String>,
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

    /// 使用 Taffy 计算所有节点的布局坐标。
    ///
    /// 对于 Free 布局的节点，直接使用其 x/y/w/h。
    /// 对于 Flex/Grid 布局的 Group 节点，使用 Taffy 计算子节点绝对坐标。
    pub fn compute_layout(&self) -> Vec<ComputedLayout> {
        let mut results = Vec::new();
        for page in &self.pages {
            compute_page_layout(page, &mut results);
        }
        tracing::debug!(count = results.len(), "布局计算完成");
        results
    }
}

// ── 布局计算 ──

/// 节点布局计算后的绝对坐标。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputedLayout {
    pub node_id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

fn compute_page_layout(page: &Page, results: &mut Vec<ComputedLayout>) {
    for node in &page.nodes {
        compute_node_layout(node, 0.0, 0.0, results);
    }
}

fn compute_node_layout(node: &PenNode, parent_x: f32, parent_y: f32, results: &mut Vec<ComputedLayout>) {
    let abs_x = parent_x + node.x;
    let abs_y = parent_y + node.y;

    results.push(ComputedLayout {
        node_id: node.id.clone(),
        x: abs_x,
        y: abs_y,
        w: node.w,
        h: node.h,
    });

    match &node.style.layout {
        LayoutMode::Free => {
            for child in &node.children {
                compute_node_layout(child, abs_x, abs_y, results);
            }
        }
        LayoutMode::Flex(params) => {
            let child_layouts = compute_flex_layout(node, params, abs_x, abs_y);
            results.extend(child_layouts);
        }
        LayoutMode::Grid(params) => {
            let child_layouts = compute_grid_layout(node, params, abs_x, abs_y);
            results.extend(child_layouts);
        }
    }
}

fn compute_flex_layout(
    parent: &PenNode,
    params: &FlexParams,
    abs_x: f32,
    abs_y: f32,
) -> Vec<ComputedLayout> {
    let mut taffy_tree: taffy::TaffyTree<()> = taffy::TaffyTree::new();
    let mut node_map: Vec<(taffy::NodeId, String)> = Vec::new();

    let direction = match params.direction {
        FlexDirection::Row => taffy::FlexDirection::Row,
        FlexDirection::RowReverse => taffy::FlexDirection::RowReverse,
        FlexDirection::Column => taffy::FlexDirection::Column,
        FlexDirection::ColumnReverse => taffy::FlexDirection::ColumnReverse,
    };

    let align_items = match params.align_items {
        AlignItems::Stretch => taffy::AlignItems::Stretch,
        AlignItems::Start => taffy::AlignItems::Start,
        AlignItems::End => taffy::AlignItems::End,
        AlignItems::Center => taffy::AlignItems::Center,
    };

    let justify_content = match params.justify_content {
        JustifyContent::Start => taffy::JustifyContent::Start,
        JustifyContent::Center => taffy::JustifyContent::Center,
        JustifyContent::End => taffy::JustifyContent::End,
        JustifyContent::SpaceBetween => taffy::JustifyContent::SpaceBetween,
        JustifyContent::SpaceAround => taffy::JustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly => taffy::JustifyContent::SpaceEvenly,
    };

    let flex_wrap = match params.wrap {
        FlexWrap::NoWrap => taffy::FlexWrap::NoWrap,
        FlexWrap::Wrap => taffy::FlexWrap::Wrap,
    };

    let parent_style = taffy::Style {
        display: taffy::Display::Flex,
        flex_direction: direction,
        align_items: Some(align_items),
        justify_content: Some(justify_content),
        flex_wrap,
        gap: taffy::Size::length(params.gap),
        padding: taffy::Rect {
            top: taffy::LengthPercentage::Length(params.padding.top),
            right: taffy::LengthPercentage::Length(params.padding.right),
            bottom: taffy::LengthPercentage::Length(params.padding.bottom),
            left: taffy::LengthPercentage::Length(params.padding.left),
        },
        size: taffy::Size {
            width: taffy::Dimension::Length(parent.w),
            height: taffy::Dimension::Length(parent.h),
        },
        ..Default::default()
    };

    let mut child_ids: Vec<taffy::NodeId> = Vec::new();
    for child in &parent.children {
        let child_style = taffy::Style {
            size: taffy::Size {
                width: if child.w > 0.0 { taffy::Dimension::Length(child.w) } else { taffy::Dimension::Auto },
                height: if child.h > 0.0 { taffy::Dimension::Length(child.h) } else { taffy::Dimension::Auto },
            },
            ..Default::default()
        };
        let id = taffy_tree.new_leaf(child_style).unwrap();
        child_ids.push(id);
        node_map.push((id, child.id.clone()));
    }

    let root_id = taffy_tree.new_with_children(parent_style, &child_ids).unwrap();

    let available = taffy::Size {
        width: taffy::AvailableSpace::Definite(parent.w),
        height: taffy::AvailableSpace::Definite(parent.h),
    };
    if let Err(e) = taffy_tree.compute_layout(root_id, available) {
        tracing::warn!(error = %e, "Taffy Flex 布局计算失败，回退自由布局");
        let mut results = Vec::new();
        for child in &parent.children {
            compute_node_layout(child, abs_x, abs_y, &mut results);
        }
        return results;
    }

    let mut results = Vec::new();
    for (taffy_id, node_id) in &node_map {
        let layout = taffy_tree.layout(*taffy_id).unwrap();
        results.push(ComputedLayout {
            node_id: node_id.clone(),
            x: abs_x + layout.location.x,
            y: abs_y + layout.location.y,
            w: layout.size.width,
            h: layout.size.height,
        });
    }
    results
}

fn compute_grid_layout(
    parent: &PenNode,
    params: &GridParams,
    abs_x: f32,
    abs_y: f32,
) -> Vec<ComputedLayout> {
    let mut taffy_tree: taffy::TaffyTree<()> = taffy::TaffyTree::new();
    let mut node_map: Vec<(taffy::NodeId, String)> = Vec::new();

    let columns: Vec<taffy::TrackSizingFunction> = params.columns.iter().map(|t| match t {
        TrackSizing::Fixed(v) => taffy::style_helpers::length(*v),
        TrackSizing::Auto => taffy::style_helpers::auto(),
        TrackSizing::Flex(v) => taffy::style_helpers::flex(*v),
        TrackSizing::Percent(v) => taffy::style_helpers::percent(*v),
    }).collect();

    let rows: Vec<taffy::TrackSizingFunction> = params.rows.iter().map(|t| match t {
        TrackSizing::Fixed(v) => taffy::style_helpers::length(*v),
        TrackSizing::Auto => taffy::style_helpers::auto(),
        TrackSizing::Flex(v) => taffy::style_helpers::flex(*v),
        TrackSizing::Percent(v) => taffy::style_helpers::percent(*v),
    }).collect();

    let parent_style = taffy::Style {
        display: taffy::Display::Grid,
        grid_template_columns: columns,
        grid_template_rows: rows,
        gap: taffy::Size {
            width: taffy::LengthPercentage::Length(params.gap.1),
            height: taffy::LengthPercentage::Length(params.gap.0),
        },
        size: taffy::Size {
            width: taffy::Dimension::Length(parent.w),
            height: taffy::Dimension::Length(parent.h),
        },
        ..Default::default()
    };

    let mut child_ids: Vec<taffy::NodeId> = Vec::new();
    for (i, child) in parent.children.iter().enumerate() {
        let mut child_style = taffy::Style {
            size: taffy::Size {
                width: if child.w > 0.0 { taffy::Dimension::Length(child.w) } else { taffy::Dimension::Auto },
                height: if child.h > 0.0 { taffy::Dimension::Length(child.h) } else { taffy::Dimension::Auto },
            },
            ..Default::default()
        };

        if let Some(area) = params.areas.get(i) {
            child_style.grid_row = taffy::Line {
                start: taffy::style_helpers::line(area.row_start as i16),
                end: taffy::style_helpers::line(area.row_end as i16),
            };
            child_style.grid_column = taffy::Line {
                start: taffy::style_helpers::line(area.col_start as i16),
                end: taffy::style_helpers::line(area.col_end as i16),
            };
        }

        let id = taffy_tree.new_leaf(child_style).unwrap();
        child_ids.push(id);
        node_map.push((id, child.id.clone()));
    }

    let root_id = taffy_tree.new_with_children(parent_style, &child_ids).unwrap();

    let available = taffy::Size {
        width: taffy::AvailableSpace::Definite(parent.w),
        height: taffy::AvailableSpace::Definite(parent.h),
    };
    if let Err(e) = taffy_tree.compute_layout(root_id, available) {
        tracing::warn!(error = %e, "Taffy Grid 布局计算失败，回退自由布局");
        let mut results = Vec::new();
        for child in &parent.children {
            compute_node_layout(child, abs_x, abs_y, &mut results);
        }
        return results;
    }

    let mut results = Vec::new();
    for (taffy_id, node_id) in &node_map {
        let layout = taffy_tree.layout(*taffy_id).unwrap();
        results.push(ComputedLayout {
            node_id: node_id.clone(),
            x: abs_x + layout.location.x,
            y: abs_y + layout.location.y,
            w: layout.size.width,
            h: layout.size.height,
        });
    }
    results
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
            rotation: 0.0,
            z_index: 0,
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
            rotation: 0.0,
            z_index: 0,
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
            rotation: 0.0,
            z_index: 0,
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
        assert!(doc.find_node("g").is_some());
    }

    #[test]
    fn doc_remove_nonexistent_returns_false() {
        let mut doc = sample_doc();
        assert!(!doc.remove_node("nope"));
    }

    #[test]
    fn node_descendant_ids_includes_self() {
        let g = PenNode::group("g", 0.0, 0.0, vec![PenNode::rect("c1", 0.0, 0.0, 1.0, 1.0)]);
        assert_eq!(g.descendant_ids(), vec!["g", "c1"]);
    }

    #[test]
    fn node_find_in_children() {
        let g = PenNode::group("g", 0.0, 0.0, vec![PenNode::rect("c1", 0.0, 0.0, 1.0, 1.0)]);
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
        assert_eq!(n.rotation, 0.0);
        assert_eq!(n.z_index, 0);
    }

    // ── V0.2 布局测试 ──

    #[test]
    fn layout_mode_free_default() {
        let style = NodeStyle::default();
        assert!(matches!(style.layout, LayoutMode::Free));
    }

    #[test]
    fn layout_mode_flex_serde_roundtrip() {
        let flex = LayoutMode::Flex(FlexParams {
            direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            wrap: FlexWrap::Wrap,
            gap: 8.0,
            padding: SideOffsets::uniform(16.0),
        });
        let s = serde_json::to_string(&flex).unwrap();
        let f2: LayoutMode = serde_json::from_str(&s).unwrap();
        assert_eq!(flex, f2);
    }

    #[test]
    fn layout_mode_grid_serde_roundtrip() {
        let grid = LayoutMode::Grid(GridParams {
            columns: vec![TrackSizing::Flex(1.0), TrackSizing::Flex(1.0)],
            rows: vec![TrackSizing::Auto],
            gap: (8.0, 16.0),
            areas: vec![GridArea {
                name: "main".into(),
                row_start: 1, row_end: 2,
                col_start: 1, col_end: 3,
            }],
        });
        let s = serde_json::to_string(&grid).unwrap();
        let g2: LayoutMode = serde_json::from_str(&s).unwrap();
        assert_eq!(grid, g2);
    }

    #[test]
    fn component_slot_serde_roundtrip() {
        let slot = ComponentSlot {
            component_id: "button".into(),
            variant: "primary".into(),
            overrides: {
                let mut m = HashMap::new();
                m.insert("fill".into(), serde_json::json!("#FF0000"));
                m
            },
        };
        let s = serde_json::to_string(&slot).unwrap();
        let s2: ComponentSlot = serde_json::from_str(&s).unwrap();
        assert_eq!(slot, s2);
    }

    #[test]
    fn node_style_layout_field_serde() {
        let mut style = NodeStyle::default();
        style.layout = LayoutMode::Flex(FlexParams {
            direction: FlexDirection::Row,
            gap: 12.0,
            ..Default::default()
        });
        style.design_token_refs.insert("fill".into(), "color.accent".into());
        let s = serde_json::to_string(&style).unwrap();
        let s2: NodeStyle = serde_json::from_str(&s).unwrap();
        assert_eq!(style, s2);
    }

    #[test]
    fn pen_node_rotation_z_index_serde() {
        let mut n = PenNode::rect("r", 0.0, 0.0, 10.0, 10.0);
        n.rotation = 45.0;
        n.z_index = 5;
        let s = serde_json::to_string(&n).unwrap();
        let n2: PenNode = serde_json::from_str(&s).unwrap();
        assert_eq!(n2.rotation, 45.0);
        assert_eq!(n2.z_index, 5);
    }

    #[test]
    fn compute_layout_free_nodes() {
        let doc = sample_doc();
        let layouts = doc.compute_layout();
        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0].node_id, "n1");
        assert_eq!(layouts[0].x, 0.0);
        assert_eq!(layouts[1].node_id, "n2");
    }

    #[test]
    fn compute_layout_flex_children() {
        let mut doc = PenDocument::new();
        let mut page = Page::new("p1", "Flex", 400.0, 200.0);
        let mut container = PenNode::group("container", 0.0, 0.0, vec![
            PenNode::rect("a", 0.0, 0.0, 100.0, 50.0),
            PenNode::rect("b", 0.0, 0.0, 100.0, 50.0),
        ]);
        container.w = 400.0;
        container.h = 200.0;
        container.style.layout = LayoutMode::Flex(FlexParams {
            direction: FlexDirection::Row,
            gap: 10.0,
            ..Default::default()
        });
        page.add(container);
        doc.add_page(page);

        let layouts = doc.compute_layout();
        assert_eq!(layouts.len(), 3);

        let a = layouts.iter().find(|l| l.node_id == "a").unwrap();
        let b = layouts.iter().find(|l| l.node_id == "b").unwrap();
        assert_eq!(a.x, 0.0);
        assert_eq!(b.x, 110.0);
    }

    #[test]
    fn compute_layout_grid_children() {
        let mut doc = PenDocument::new();
        let mut page = Page::new("p1", "Grid", 400.0, 200.0);
        let mut container = PenNode::group("container", 0.0, 0.0, vec![
            PenNode::rect("a", 0.0, 0.0, 0.0, 50.0),
            PenNode::rect("b", 0.0, 0.0, 0.0, 50.0),
        ]);
        container.w = 400.0;
        container.h = 200.0;
        container.style.layout = LayoutMode::Grid(GridParams {
            columns: vec![TrackSizing::Flex(1.0), TrackSizing::Flex(1.0)],
            rows: vec![TrackSizing::Auto],
            gap: (10.0, 10.0),
            areas: vec![],
        });
        page.add(container);
        doc.add_page(page);

        let layouts = doc.compute_layout();
        let a = layouts.iter().find(|l| l.node_id == "a").unwrap();
        let b = layouts.iter().find(|l| l.node_id == "b").unwrap();
        assert!(a.w > 0.0);
        assert!(b.w > 0.0);
        assert!(b.x > a.x);
    }

    #[test]
    fn side_offsets_uniform() {
        let s = SideOffsets::uniform(8.0);
        assert_eq!(s.top, 8.0);
        assert_eq!(s.right, 8.0);
        assert_eq!(s.bottom, 8.0);
        assert_eq!(s.left, 8.0);
    }

    #[test]
    fn track_sizing_serde() {
        let sizes = vec![
            TrackSizing::Fixed(200.0),
            TrackSizing::Auto,
            TrackSizing::Flex(1.0),
            TrackSizing::Percent(0.5),
        ];
        let s = serde_json::to_string(&sizes).unwrap();
        let s2: Vec<TrackSizing> = serde_json::from_str(&s).unwrap();
        assert_eq!(sizes, s2);
    }

    #[test]
    fn backward_compat_old_json_still_loads() {
        let old_json = r#"{"pages":[{"id":"p1","name":"Home","width":100,"height":100,"nodes":[{"id":"n1","kind":"Rect","name":"Rect","x":0,"y":0,"w":50,"h":50,"style":{}}]}]}"#;
        let doc = PenDocument::from_json(old_json).unwrap();
        assert_eq!(doc.pages[0].nodes[0].rotation, 0.0);
        assert_eq!(doc.pages[0].nodes[0].z_index, 0);
        assert!(matches!(doc.pages[0].nodes[0].style.layout, LayoutMode::Free));
    }

    // ── ComponentSlot 实例化测试 ──

    #[test]
    fn component_registry_instantiate_with_overrides() {
        let mut reg = ComponentRegistry::new();
        let mut variants = HashMap::new();
        variants.insert("primary".into(), PenNode::rect("btn", 0.0, 0.0, 120.0, 40.0));
        reg.register(ComponentDefinition {
            id: "button".into(),
            name: "Button".into(),
            variants,
        });

        let slot = ComponentSlot {
            component_id: "button".into(),
            variant: "primary".into(),
            overrides: {
                let mut m = HashMap::new();
                m.insert("fill".into(), serde_json::json!("#FF0000"));
                m.insert("w".into(), serde_json::json!(200.0));
                m
            },
        };

        let instance = reg.instantiate(&slot).unwrap();
        assert_eq!(instance.style.fill.as_deref(), Some("#FF0000"));
        assert_eq!(instance.w, 200.0);
        assert_eq!(instance.h, 40.0); // 不变
    }

    #[test]
    fn component_registry_instantiate_unknown_component() {
        let reg = ComponentRegistry::new();
        let slot = ComponentSlot {
            component_id: "nonexist".into(),
            variant: "default".into(),
            overrides: HashMap::new(),
        };
        assert!(reg.instantiate(&slot).is_none());
    }

    #[test]
    fn component_registry_instantiate_unknown_variant() {
        let mut reg = ComponentRegistry::new();
        let mut variants = HashMap::new();
        variants.insert("primary".into(), PenNode::rect("btn", 0.0, 0.0, 120.0, 40.0));
        reg.register(ComponentDefinition {
            id: "button".into(),
            name: "Button".into(),
            variants,
        });

        let slot = ComponentSlot {
            component_id: "button".into(),
            variant: "ghost".into(),
            overrides: HashMap::new(),
        };
        assert!(reg.instantiate(&slot).is_none());
    }

    #[test]
    fn component_overrides_do_not_affect_template() {
        let mut reg = ComponentRegistry::new();
        let mut variants = HashMap::new();
        variants.insert("default".into(), PenNode::rect("btn", 0.0, 0.0, 100.0, 40.0));
        reg.register(ComponentDefinition {
            id: "button".into(),
            name: "Button".into(),
            variants,
        });

        let slot = ComponentSlot {
            component_id: "button".into(),
            variant: "default".into(),
            overrides: {
                let mut m = HashMap::new();
                m.insert("fill".into(), serde_json::json!("#00FF00"));
                m
            },
        };

        let _instance = reg.instantiate(&slot).unwrap();
        // 原模板未被修改
        let tmpl = reg.get("button").unwrap().variants.get("default").unwrap();
        assert!(tmpl.style.fill.is_none());
    }
}
