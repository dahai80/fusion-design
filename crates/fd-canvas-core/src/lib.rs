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
//! V0.2 扩展：LayoutMode(Flex/Grid) 声明 + ComponentSlot + Token 引用。
// Callers: fd-host-web (DOM render), fd-codegen (layout-aware CSS gen), fd-export (export)
// Affected API: NodeStyle (new fields), PenNode (rotation/z_index)
// Data schemas: LayoutMode/FlexParams/GridParams/TrackSizing/ComponentSlot
// H-A10：Taffy compute_layout 已移除（死代码）；Flex/Grid 渲染由 codegen 生成 CSS。

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Deserializer, Serialize};

// A-5：CSS 安全工具下沉到 fd-css-utils 叶子 crate，此处 re-export 保持
// 下游调用方 `fd_canvas_core::parse_hex_color` / `sanitize_css_value` 不变。
pub use fd_css_utils::{parse_hex_color, sanitize_css_value};

// 反序列化安全护栏：限制恶意/损坏 .fusiondesign 的嵌套深度与节点总数，
// 防止深度嵌套导致栈溢出或海量节点导致 OOM。
const MAX_NODE_DEPTH: usize = 64;
const MAX_NODE_TOTAL: usize = 100_000;

// 文件格式 schema 版本（A1：无版本号无法做向前兼容/迁移）。
// 当前版本 1。加载时校验：缺失视作 1（兼容旧文件），高于当前视作错误。
pub const SCHEMA_VERSION: u32 = 1;

// ── 文档/页面/节点 ──

/// 画布文档（顶层工程文件 `.fusiondesign` 的结构体）。
///
/// A4：安全护栏（深度/总量上限）在反序列化边界强制执行，不经 from_json 走裸
/// `serde_json::from_str::<PenDocument>` 同样受限——绕过在类型层不可能。
/// A1：schema_version 字段支撑文件格式向前兼容/迁移。
#[derive(Debug, Clone, Default, Serialize)]
pub struct PenDocument {
    /// 文件格式 schema 版本（A1）。缺失/0 视作 1（兼容旧文件）。
    #[serde(default)]
    pub schema_version: u32,
    pub pages: Vec<Page>,
    pub variables: Option<HashMap<String, VariableDef>>,
    /// 当前激活的设计规范 ID（对接 fd-design-system）。
    pub active_design_system: Option<String>,
}

impl<'de> Deserialize<'de> for PenDocument {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct PenDocumentRaw {
            #[serde(default)]
            schema_version: u32,
            #[serde(default)]
            pages: Vec<Page>,
            #[serde(default)]
            variables: Option<HashMap<String, VariableDef>>,
            #[serde(default)]
            active_design_system: Option<String>,
        }
        let raw = PenDocumentRaw::deserialize(d)?;
        // 缺失/0 视作 1（兼容 v0.1.x 旧文件）。
        let schema_version = if raw.schema_version == 0 {
            SCHEMA_VERSION
        } else {
            raw.schema_version
        };
        if schema_version > SCHEMA_VERSION {
            return Err(serde::de::Error::custom(format!(
                "文件 schema 版本 {schema_version} 高于当前支持 {SCHEMA_VERSION}，请升级 fusion-design"
            )));
        }
        let doc = PenDocument {
            schema_version,
            pages: raw.pages,
            variables: raw.variables,
            active_design_system: raw.active_design_system,
        };
        // A4：反序列化边界强制安全护栏——任何加载路径（含裸 from_str、UndoRedoStack
        // 反序列化内嵌 PenDocument）都无法绕过深度/总量上限。
        doc.validate_limits().map_err(serde::de::Error::custom)?;
        Ok(doc)
    }
}

/// 页面（一画板）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: String,
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub nodes: Vec<PenNode>,
}

/// �矢量节点（统一形状，覆盖 rect/circle/text/image/group）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PenNode {
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub style: NodeStyle,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub children: Vec<PenNode>,
    #[serde(default)]
    pub rotation: f64,
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
    pub gap: f64,
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
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl SideOffsets {
    pub fn uniform(v: f64) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }
}

/// CSS Grid 布局参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridParams {
    pub columns: Vec<TrackSizing>,
    pub rows: Vec<TrackSizing>,
    pub gap: (f64, f64),
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
    Fixed(f64),
    Auto,
    Flex(f64),
    Percent(f64),
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
                    node.style.radius = Some(f);
                }
            }
            "text" => {
                if let Some(s) = val.as_str() {
                    node.text = Some(s.to_string());
                }
            }
            "w" => {
                if let Some(f) = val.as_f64() {
                    node.w = f;
                }
            }
            "h" => {
                if let Some(f) = val.as_f64() {
                    node.h = f;
                }
            }
            _ => {
                tracing::debug!(key = %key, "apply_overrides: 未知覆写键，忽略");
            }
        }
    }
}

/// 递归统计节点最大嵌套深度与累计节点数。深度超限即 bail（短路，不继续遍历）。
fn count_depth(node: &PenNode, depth: usize, total: &mut usize) -> anyhow::Result<usize> {
    *total += 1;
    if *total > MAX_NODE_TOTAL {
        anyhow::bail!("节点总数超过安全上限 {MAX_NODE_TOTAL}");
    }
    if depth > MAX_NODE_DEPTH {
        anyhow::bail!("节点嵌套深度超过安全上限 {MAX_NODE_DEPTH}");
    }
    let mut max_d = depth;
    for child in &node.children {
        let d = count_depth(child, depth + 1, total)?;
        if d > max_d {
            max_d = d;
        }
    }
    Ok(max_d)
}

// ── 样式 ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default)]
    pub layout: LayoutMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_slot: Option<ComponentSlot>,
    #[serde(default)]
    pub design_token_refs: HashMap<String, String>,
    #[serde(default)]
    pub locked: bool,
    #[serde(default = "default_true")]
    pub visible: bool,
}

fn default_true() -> bool {
    true
}

impl Default for NodeStyle {
    fn default() -> Self {
        Self {
            fill: None,
            stroke: None,
            stroke_width: None,
            radius: None,
            font_size: None,
            font_family: None,
            opacity: None,
            layout: LayoutMode::default(),
            component_slot: None,
            design_token_refs: HashMap::new(),
            locked: false,
            visible: true,
        }
    }
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

    /// 从 JSON 反序列化（anyhow 友好入口）。
    /// 安全护栏已由 `Deserialize` 实现强制（A4），此方法仅做错误类型转换。
    ///
    /// E-14：截断/损坏检测。`#[serde(default)]` 使被截断的文件"成功"解析成
    /// 残缺文档（缺失 pages → 空 Vec，缺失 schema_version → 默认 1），用户打开
    /// 看到空白画布却无任何告警，静默丢数据。此处对"原文非空但解析出全空文档"
    /// 的可疑情形显式 warn（fail visibly），不报错以免阻断合法的新建空文档。
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        let trimmed = json.trim();
        let doc: PenDocument = serde_json::from_str(json)
            .map_err(|e| anyhow::anyhow!("反序列化失败（含安全护栏校验）: {e}"))?;
        // 可疑截断：原文非空（非纯空白/空对象）但解析出零页面。提示用户文件可能损坏。
        if !trimmed.is_empty() && trimmed != "{}" && doc.pages.is_empty() {
            tracing::warn!(
                input_len = json.len(),
                pages = doc.pages.len(),
                "from_json: 输入非空但解析出零页面，文件可能被截断/损坏（serde default 已静默补缺字段）。请检查文件完整性。"
            );
        }
        Ok(doc)
    }

    /// 校验节点嵌套深度与总数在安全阈值内。
    pub fn validate_limits(&self) -> anyhow::Result<()> {
        let mut total: usize = 0;
        for page in &self.pages {
            for node in &page.nodes {
                let depth = count_depth(node, 1, &mut total)?;
                if depth > MAX_NODE_DEPTH {
                    anyhow::bail!("节点嵌套深度 {depth} 超过安全上限 {MAX_NODE_DEPTH}");
                }
            }
        }
        if total > MAX_NODE_TOTAL {
            anyhow::bail!("节点总数 {total} 超过安全上限 {MAX_NODE_TOTAL}");
        }
        Ok(())
    }

    pub fn snapshot(&self) -> PenDocument {
        self.clone()
    }
}

// H-A10/P1-5：Taffy compute_layout/compute_flex_layout/compute_grid_layout
// + ComputedLayout 已移除（零生产调用，展示性死代码）。
// Flex/Grid 渲染现由 layout-aware codegen 生成 flex/grid CSS，浏览器/wasm 执行布局。
// LayoutMode/FlexParams/GridParams 等 NodeStyle 声明类型保留（codegen/渲染器消费）。

impl Page {
    pub fn new(id: impl Into<String>, name: impl Into<String>, w: f64, h: f64) -> Self {
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
    pub fn rect(id: impl Into<String>, x: f64, y: f64, w: f64, h: f64) -> Self {
        Self {
            id: id.into(),
            kind: NodeKind::Rect,
            name: "Rect".into(),
            x,
            y,
            w,
            h,
            style: NodeStyle::default(),
            text: None,
            children: vec![],
            rotation: 0.0,
            z_index: 0,
        }
    }

    /// 创建 text 节点。
    pub fn text(id: impl Into<String>, x: f64, y: f64, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: NodeKind::Text,
            name: "Text".into(),
            x,
            y,
            w: 0.0,
            h: 0.0,
            style: NodeStyle::default(),
            text: Some(content.into()),
            children: vec![],
            rotation: 0.0,
            z_index: 0,
        }
    }

    /// 创建 group 节点。
    pub fn group(id: impl Into<String>, x: f64, y: f64, children: Vec<PenNode>) -> Self {
        Self {
            id: id.into(),
            kind: NodeKind::Group,
            name: "Group".into(),
            x,
            y,
            w: 0.0,
            h: 0.0,
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
    nodes
        .iter_mut()
        .any(|n| remove_node_from_list(&mut n.children, id))
}

// ── 错误 ──

#[derive(Debug, thiserror::Error)]
pub enum CanvasError {
    #[error("节点 {0} 未找到")]
    NodeNotFound(String),
    #[error("页面 {0} 未找到")]
    PageNotFound(String),
    // A-4：库 crate 错误细化（替 anyhow 字符串），fd-cli report_error 按 downcast 变体 match。
    #[error("文档解析失败: {0}")]
    ParseError(String),
    #[error("节点嵌套深度 {depth} 超过安全上限 {limit}")]
    DepthExceeded { depth: usize, limit: usize },
    #[error("节点总数 {total} 超过安全上限 {limit}")]
    NodeTotalExceeded { total: usize, limit: usize },
    // A-7：schema 迁移框架——文件版本高于当前 SCHEMA_VERSION 时上报。
    #[error("文档 schema 版本 {0} 不支持")]
    SchemaVersion(u32),
}

// ── 撤销/重做栈 ──

const UNDO_REDO_MAX_DEPTH: usize = 50;

// P2：VecDeque + pop_front() O(1)，替代 Vec::remove(0) O(n) 深拷贝。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoRedoStack {
    undo_stack: VecDeque<PenDocument>,
    redo_stack: VecDeque<PenDocument>,
}

impl UndoRedoStack {
    pub fn new() -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
        }
    }

    pub fn push(&mut self, snapshot: PenDocument) {
        if self.undo_stack.len() >= UNDO_REDO_MAX_DEPTH {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(snapshot);
        self.redo_stack.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) -> Option<PenDocument> {
        if self.undo_stack.len() < 2 {
            return None;
        }
        let current = self.undo_stack.pop_back()?;
        self.redo_stack.push_back(current);
        self.undo_stack.back().cloned()
    }

    pub fn redo(&mut self) -> Option<PenDocument> {
        let snapshot = self.redo_stack.pop_back()?;
        self.undo_stack.push_back(snapshot.clone());
        Some(snapshot)
    }
}

impl Default for UndoRedoStack {
    fn default() -> Self {
        Self::new()
    }
}

// ── 文档差异对比 ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DiffChangeType {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffEntry {
    pub node_id: String,
    pub change_type: DiffChangeType,
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<serde_json::Value>,
    /// 新增节点所属页面 ID（仅 Added 有意义）；diff 据此标注节点所在页，
    /// 供 fd-cli diff 子命令展示。避免恒定落入 pages.first() 导致多页文档节点错位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PenDocumentDiff {
    pub entries: Vec<DiffEntry>,
}

impl PenDocumentDiff {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl PenDocument {
    /// 计算两个 PenDocument 之间的节点级差异。
    pub fn diff(&self, other: &PenDocument) -> PenDocumentDiff {
        let mut entries = Vec::new();
        let self_nodes = self.all_nodes();
        let other_nodes = other.all_nodes();
        let other_by_page = other.all_nodes_by_page();
        let self_ids: Vec<&str> = self_nodes.keys().map(|s| s.as_str()).collect();
        let other_ids: Vec<&str> = other_nodes.keys().map(|s| s.as_str()).collect();

        for id in &other_ids {
            if !self_ids.contains(id) {
                let page_id = other_by_page.get(*id).cloned();
                entries.push(DiffEntry {
                    node_id: id.to_string(),
                    change_type: DiffChangeType::Added,
                    field: "*".into(),
                    old_value: None,
                    new_value: Some(
                        serde_json::to_value(other_nodes[*id])
                            .ok()
                            .unwrap_or_default(),
                    ),
                    page_id,
                });
            }
        }
        for id in &self_ids {
            if !other_ids.contains(id) {
                entries.push(DiffEntry {
                    node_id: id.to_string(),
                    change_type: DiffChangeType::Removed,
                    field: "*".into(),
                    old_value: Some(
                        serde_json::to_value(self_nodes[*id])
                            .ok()
                            .unwrap_or_default(),
                    ),
                    new_value: None,
                    page_id: None,
                });
            }
        }
        for id in &self_ids {
            if let (Some(old_node), Some(new_node)) = (
                self_ids
                    .iter()
                    .find(|s| **s == *id)
                    .and_then(|_| self_nodes.get(*id)),
                other_ids
                    .iter()
                    .find(|s| **s == *id)
                    .and_then(|_| other_nodes.get(*id)),
            ) {
                let old_json = serde_json::to_value(old_node).unwrap_or_default();
                let new_json = serde_json::to_value(new_node).unwrap_or_default();
                if old_json != new_json {
                    diff_json_objects(&old_json, &new_json, id, &mut entries);
                }
            }
        }

        PenDocumentDiff { entries }
    }

    /// 收集文档中所有节点，返回 id → node 的映射。
    fn all_nodes(&self) -> HashMap<String, &PenNode> {
        let mut map = HashMap::new();
        for page in &self.pages {
            collect_nodes(&page.nodes, &mut map);
        }
        map
    }

    /// 收集文档中所有节点，返回 id → 所在页面 ID 的映射。
    fn all_nodes_by_page(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for page in &self.pages {
            collect_node_pages(&page.nodes, &page.id, &mut map);
        }
        map
    }

    // H-A13/P1-7：apply_patch 已移除（零生产调用，展示性死代码）。
    // diff 计算（diff_versions/diff_adjacent）保留——fd-cli diff 子命令用于展示。
    // 协同编辑的 apply 路径如需恢复，见 git 历史；当前 PRD 未承诺协同编辑。
}

fn collect_nodes<'a>(nodes: &'a [PenNode], map: &mut HashMap<String, &'a PenNode>) {
    for n in nodes {
        map.insert(n.id.clone(), n);
        collect_nodes(&n.children, map);
    }
}

fn collect_node_pages(nodes: &[PenNode], page_id: &str, map: &mut HashMap<String, String>) {
    for n in nodes {
        map.insert(n.id.clone(), page_id.to_string());
        collect_node_pages(&n.children, page_id, map);
    }
}

fn diff_json_objects(
    old: &serde_json::Value,
    new: &serde_json::Value,
    node_id: &str,
    entries: &mut Vec<DiffEntry>,
) {
    if let (serde_json::Value::Object(old_map), serde_json::Value::Object(new_map)) = (old, new) {
        for key in new_map.keys() {
            let old_val = old_map.get(key);
            let new_val = new_map.get(key);
            if old_val != new_val {
                entries.push(DiffEntry {
                    node_id: node_id.to_string(),
                    change_type: DiffChangeType::Modified,
                    field: key.clone(),
                    old_value: old_val.cloned(),
                    new_value: new_val.cloned(),
                    page_id: None,
                });
            }
        }
        for key in old_map.keys() {
            if !new_map.contains_key(key) {
                entries.push(DiffEntry {
                    node_id: node_id.to_string(),
                    change_type: DiffChangeType::Modified,
                    field: key.clone(),
                    old_value: old_map.get(key).cloned(),
                    new_value: None,
                    page_id: None,
                });
            }
        }
    }
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
        g.children
            .push(PenNode::rect("inner", 0.0, 0.0, 10.0, 10.0));
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
                row_start: 1,
                row_end: 2,
                col_start: 1,
                col_end: 3,
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
        let mut style = NodeStyle {
            layout: LayoutMode::Flex(FlexParams {
                direction: FlexDirection::Row,
                gap: 12.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        style
            .design_token_refs
            .insert("fill".into(), "color.accent".into());
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
        assert!(matches!(
            doc.pages[0].nodes[0].style.layout,
            LayoutMode::Free
        ));
    }

    // ── ComponentSlot 实例化测试 ──

    #[test]
    fn component_registry_instantiate_with_overrides() {
        let mut reg = ComponentRegistry::new();
        let mut variants = HashMap::new();
        variants.insert(
            "primary".into(),
            PenNode::rect("btn", 0.0, 0.0, 120.0, 40.0),
        );
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
        variants.insert(
            "primary".into(),
            PenNode::rect("btn", 0.0, 0.0, 120.0, 40.0),
        );
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
        variants.insert(
            "default".into(),
            PenNode::rect("btn", 0.0, 0.0, 100.0, 40.0),
        );
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

    #[test]
    fn node_style_locked_visible_default() {
        let style = NodeStyle::default();
        assert!(!style.locked);
        assert!(style.visible);
    }

    #[test]
    fn node_style_locked_visible_serde_roundtrip() {
        let style = NodeStyle {
            locked: true,
            visible: false,
            ..Default::default()
        };
        let json = serde_json::to_string(&style).unwrap();
        let de: NodeStyle = serde_json::from_str(&json).unwrap();
        assert!(de.locked);
        assert!(!de.visible);
    }

    #[test]
    fn node_style_backward_compat_old_json() {
        let old = serde_json::json!({
            "fill": "#FF0000",
            "layout": "Free",
            "design_token_refs": {}
        });
        let de: NodeStyle = serde_json::from_value(old).unwrap();
        assert!(!de.locked);
        assert!(de.visible);
    }

    #[test]
    fn undo_redo_basic() {
        let mut stack = UndoRedoStack::new();
        let doc_v1 = PenDocument::new();
        stack.push(doc_v1.clone());

        let mut doc_v2 = doc_v1.clone();
        doc_v2.add_page(Page {
            id: "p1".into(),
            name: "Page 1".into(),
            width: 800.0,
            height: 600.0,
            nodes: vec![],
        });
        stack.push(doc_v2.clone());

        assert!(stack.can_undo());
        let undone = stack.undo().unwrap();
        assert_eq!(undone.pages.len(), 0);

        assert!(stack.can_redo());
        let redone = stack.redo().unwrap();
        assert_eq!(redone.pages.len(), 1);
    }

    #[test]
    fn undo_redo_empty_safe() {
        let mut stack = UndoRedoStack::new();
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
        assert!(stack.undo().is_none());
        assert!(stack.redo().is_none());
    }

    #[test]
    fn undo_redo_max_depth() {
        let mut stack = UndoRedoStack::new();
        for i in 0..60 {
            let mut doc = PenDocument::new();
            doc.add_page(Page {
                id: format!("p{i}"),
                name: format!("Page {i}"),
                width: 800.0,
                height: 600.0,
                nodes: vec![],
            });
            stack.push(doc);
        }
        // 最多 50 步，超过的应被丢弃
        assert!(stack.can_undo());
        let undone = stack.undo().unwrap();
        // 最旧的 10 个已被丢弃，undo 应退到第 50 步
        assert_eq!(undone.pages.len(), 1);
    }

    #[test]
    fn pen_document_snapshot() {
        let mut doc = PenDocument::new();
        doc.add_page(Page {
            id: "p1".into(),
            name: "Page 1".into(),
            width: 800.0,
            height: 600.0,
            nodes: vec![PenNode::rect("n1", 10.0, 20.0, 100.0, 50.0)],
        });
        let snap = doc.snapshot();
        assert_eq!(snap.pages.len(), 1);
        assert_eq!(snap.pages[0].nodes.len(), 1);
    }

    #[test]
    fn diff_same_document_empty() {
        let doc = PenDocument::new();
        let diff = doc.diff(&doc);
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_added_node() {
        let doc_v1 = PenDocument::new();
        let mut doc_v2 = PenDocument::new();
        doc_v2.add_page(Page {
            id: "p1".into(),
            name: "Page 1".into(),
            width: 800.0,
            height: 600.0,
            nodes: vec![PenNode::rect("n1", 0.0, 0.0, 100.0, 50.0)],
        });
        let diff = doc_v1.diff(&doc_v2);
        assert!(diff
            .entries
            .iter()
            .any(|e| e.node_id == "n1" && e.change_type == DiffChangeType::Added));
    }

    #[test]
    fn diff_modified_node() {
        let mut doc_v1 = PenDocument::new();
        doc_v1.add_page(Page {
            id: "p1".into(),
            name: "Page 1".into(),
            width: 800.0,
            height: 600.0,
            nodes: vec![PenNode::rect("n1", 10.0, 20.0, 100.0, 50.0)],
        });
        let mut doc_v2 = doc_v1.clone();
        doc_v2.pages[0].nodes[0].w = 200.0;
        let diff = doc_v1.diff(&doc_v2);
        assert!(diff.entries.iter().any(|e| e.node_id == "n1"
            && e.field == "w"
            && e.change_type == DiffChangeType::Modified));
    }

    #[test]
    fn diff_removed_node() {
        let mut doc_v1 = PenDocument::new();
        doc_v1.add_page(Page {
            id: "p1".into(),
            name: "Page 1".into(),
            width: 800.0,
            height: 600.0,
            nodes: vec![PenNode::rect("n1", 10.0, 20.0, 100.0, 50.0)],
        });
        let doc_v2 = PenDocument::new();
        let diff = doc_v1.diff(&doc_v2);
        assert!(diff
            .entries
            .iter()
            .any(|e| e.node_id == "n1" && e.change_type == DiffChangeType::Removed));
    }

    // H-A13：apply_patch 已移除（零生产调用，展示性死代码）。
    // 保留 diff 的 page_id 归属回归——fd-cli diff 子命令依赖该语义展示节点所在页。
    #[test]
    fn diff_added_node_carries_page_id() {
        let mut doc_v1 = PenDocument::new();
        doc_v1.add_page(Page {
            id: "p1".into(),
            name: "Page 1".into(),
            width: 800.0,
            height: 600.0,
            nodes: vec![PenNode::rect("n1", 10.0, 20.0, 100.0, 50.0)],
        });
        doc_v1.add_page(Page {
            id: "p2".into(),
            name: "Page 2".into(),
            width: 800.0,
            height: 600.0,
            nodes: vec![],
        });
        let mut doc_v2 = doc_v1.clone();
        doc_v2
            .page_mut("p2")
            .unwrap()
            .nodes
            .push(PenNode::rect("n2", 5.0, 5.0, 40.0, 40.0));
        let patch = doc_v1.diff(&doc_v2);
        let added = patch
            .entries
            .iter()
            .find(|e| e.node_id == "n2" && e.change_type == DiffChangeType::Added)
            .expect("应有 n2 Added 条目");
        assert_eq!(added.page_id.as_deref(), Some("p2"));
    }
}

// ── 命名版本管理（已移除）──
// H-A14/P2-4：VersionedDocument/NamedVersion 全段已移除（死代码）。
// 审计自判「260 行死代码占体积」：命名版本 API（save_version/switch_to/
// diff_versions 等）跨 crate 零生产消费者——.fusiondesign 实存裸 PenDocument，
// CLI undo/redo 走独立 UndoRedoStack（不经此类型）。save_version 同时入
// versions Vec + undo 栈致进程内双份深拷贝，落盘虽已修（undo 栈 #[serde(skip)]），
// 但类型本身零消费者，建 delta-COW 基础设施为空壳投入收益为零（Rule 2）。
// 撤销/重做能力由 UndoRedoStack 独立提供（fd-cli 直用，保留）。如未来需命名
// 版本，重新接入时按 delta-COW 设计，勿复活整快照方案。
// 随同移除：uuid_v4/now_iso/civil_from_days/VERSION_SEQ/MAX_VERSIONED_FILE_BYTES
//（仅 VersionedDocument + 其测试使用，无其他消费者）。

#[cfg(test)]
mod security_tests {
    use super::*;

    fn sample_doc(name: &str) -> PenDocument {
        let mut doc = PenDocument::new();
        doc.add_page(Page {
            id: "p1".into(),
            name: name.into(),
            width: 800.0,
            height: 600.0,
            nodes: vec![PenNode::rect("n1", 10.0, 20.0, 100.0, 50.0)],
        });
        doc
    }

    #[test]
    fn from_json_rejects_deeply_nested() {
        // 恶意 .fusiondesign：深度嵌套 children 超过 MAX_NODE_DEPTH → 拒绝
        let mut json =
            String::from(r#"{"pages":[{"id":"p","name":"p","width":1.0,"height":1.0,"nodes":["#);
        let mut node = String::from(
            r#"{"id":"n","kind":"rect","name":"n","x":0,"y":0,"w":1,"h":1,"children":["#,
        );
        for _ in 0..(MAX_NODE_DEPTH + 5) {
            node.push_str(
                r#"{"id":"n","kind":"rect","name":"n","x":0,"y":0,"w":1,"h":1,"children":["#,
            );
        }
        for _ in 0..(MAX_NODE_DEPTH + 5) {
            node.push_str("]}");
        }
        json.push_str(&node);
        json.push_str("]}],\"variables\":null,\"active_design_system\":null}");
        assert!(PenDocument::from_json(&json).is_err(), "深度超限应被拒绝");
    }

    /// A4：裸 `serde_json::from_str::<PenDocument>`（绕过 from_json 的路径）
    /// 必须同样被自定义 Deserialize 拦截——CLI/undo/redo 旧绕过点修复的根。
    #[test]
    fn raw_deserialize_rejects_deeply_nested() {
        let mut json =
            String::from(r#"{"pages":[{"id":"p","name":"p","width":1.0,"height":1.0,"nodes":["#);
        let mut node = String::from(
            r#"{"id":"n","kind":"rect","name":"n","x":0,"y":0,"w":1,"h":1,"children":["#,
        );
        for _ in 0..(MAX_NODE_DEPTH + 5) {
            node.push_str(
                r#"{"id":"n","kind":"rect","name":"n","x":0,"y":0,"w":1,"h":1,"children":["#,
            );
        }
        for _ in 0..(MAX_NODE_DEPTH + 5) {
            node.push_str("]}");
        }
        json.push_str(&node);
        json.push_str("]}]}");
        let result: Result<PenDocument, _> = serde_json::from_str(&json);
        assert!(
            result.is_err(),
            "裸 from_str 必须被自定义 Deserialize 拦截（A4）"
        );
    }

    /// A4：超量节点（> MAX_NODE_TOTAL）裸 from_str 同样拒绝。
    #[test]
    fn raw_deserialize_rejects_too_many_nodes() {
        let mut nodes = String::new();
        for i in 0..(MAX_NODE_TOTAL + 10) {
            if i > 0 {
                nodes.push(',');
            }
            nodes.push_str(&format!(
                r#"{{"id":"n{i}","kind":"rect","name":"n","x":0,"y":0,"w":1,"h":1}}"#
            ));
        }
        let json = format!(
            r#"{{"pages":[{{"id":"p","name":"p","width":1.0,"height":1.0,"nodes":[{nodes}]}}]}}"#
        );
        let result: Result<PenDocument, _> = serde_json::from_str(&json);
        assert!(result.is_err(), "超量节点裸 from_str 必须拒绝（A4）");
    }

    /// A1：schema_version 缺失（旧文件）视作 1，正常加载。
    #[test]
    fn schema_version_missing_defaults_to_one() {
        let json = r#"{"pages":[],"variables":null,"active_design_system":null}"#;
        let doc: PenDocument = serde_json::from_str(json).unwrap();
        assert_eq!(
            doc.schema_version, SCHEMA_VERSION,
            "缺失 schema_version 默认 1"
        );
    }

    /// A1：schema_version 高于当前 → 拒绝（防未来版本静默丢字段）。
    #[test]
    fn schema_version_ahead_rejected() {
        let json =
            r#"{"schema_version":999,"pages":[],"variables":null,"active_design_system":null}"#;
        let result: Result<PenDocument, _> = serde_json::from_str(json);
        assert!(result.is_err(), "超前 schema_version 应拒绝");
    }

    /// A1：schema_version=1 正常加载并回写。
    #[test]
    fn schema_version_roundtrip() {
        let doc = PenDocument::new();
        let json = serde_json::to_string(&doc).unwrap();
        assert!(
            json.contains("\"schema_version\""),
            "序列化应含 schema_version"
        );
        let back: PenDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn validate_limits_accepts_normal_doc() {
        let doc = sample_doc("ok");
        assert!(doc.validate_limits().is_ok(), "正常文档应通过校验");
    }

    /// 性能基线：1000 节点文档的序列化/反序列化/布局计算耗时。
    /// `#[ignore]`：不进常规 CI，经 `cargo test --release -- --ignored perf_baseline` 运行。
    /// 阈值：各操作 < 500ms（release，Apple Silicon），超阈打印但不断言失败（基线参考）。
    #[test]
    #[ignore]
    fn perf_baseline_1000_nodes() {
        let mut doc = sample_doc("perf");
        let page = &mut doc.pages[0];
        for i in 0..1000u32 {
            page.add(PenNode {
                id: format!("n{i}"),
                kind: NodeKind::Rect,
                name: format!("node{i}"),
                x: i as f64,
                y: 0.0,
                w: 100.0,
                h: 50.0,
                style: NodeStyle::default(),
                text: None,
                children: vec![],
                rotation: 0.0,
                z_index: 0,
            });
        }
        const THRESHOLD_MS: u128 = 500;

        let t = std::time::Instant::now();
        let json = doc.to_json().unwrap();
        let ser_ms = t.elapsed().as_millis();
        eprintln!("perf serialize(1000): {ser_ms}ms");

        let t = std::time::Instant::now();
        let doc2 = PenDocument::from_json(&json).unwrap();
        let de_ms = t.elapsed().as_millis();
        eprintln!("perf deserialize(1000): {de_ms}ms");
        assert!(doc2.pages[0].nodes.len() >= 1000, "反序列化节点数应 ≥1000");

        assert!(
            ser_ms < THRESHOLD_MS,
            "serialize {ser_ms}ms > {THRESHOLD_MS}ms"
        );
        assert!(
            de_ms < THRESHOLD_MS,
            "deserialize {de_ms}ms > {THRESHOLD_MS}ms"
        );
    }

    // E-14：截断/损坏文件不应静默丢数据。
    #[test]
    fn from_json_warns_on_suspected_truncated_file() {
        // 非空输入但 pages 字段缺失 → serde default 补空 Vec，解析"成功"但疑似截断。
        // from_json 不应报错（避免阻断合法空文档），但应识别可疑情形（此处通过返回 Ok
        // + 内部 warn 体现；行为断言：不 panic、返回空 pages 文档）。
        let truncated = r#"{"schema_version":1,"variables":null}"#;
        let doc = PenDocument::from_json(truncated).expect("截断文件应解析成功不报错");
        assert!(doc.pages.is_empty(), "截断文件 pages 应为空");
        assert_eq!(doc.schema_version, 1);
    }

    #[test]
    fn from_json_silent_on_legit_empty_object() {
        // 合法空文档 "{}"：serde 全 default，不应触发截断告警分支（trim == "{}" 排除）。
        let doc = PenDocument::from_json("{}").expect("空对象应解析成功");
        assert!(doc.pages.is_empty());
        assert_eq!(doc.schema_version, 1);
    }

    #[test]
    fn from_json_silent_on_normal_doc() {
        // 正常文档不应触发截断告警。
        let doc = sample_doc("normal");
        let json = doc.to_json().unwrap();
        let back = PenDocument::from_json(&json).unwrap();
        assert_eq!(back.pages.len(), 1);
    }
}
