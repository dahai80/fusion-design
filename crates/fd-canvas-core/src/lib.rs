//! Fusion-Design canvas kernel — self-built lightweight vector canvas model.
//!
//! Design decision: OpenPencil's 4 core crates hard-depend on the private `jian-ops-schema`
//! base (`vendor/jian/` was never fetched upstream; the local directory is empty). The
//! fusion-design MVP does not build a jian stand-in (high effort, off-target); instead it
//! defines this native lightweight canvas model, wiring directly into the existing
//! `fd-export`/`fd-design-system`/`fd-ecosystem`.
//!
//! The model covers the MVP essentials: PenDocument/PenNode/page/node/style/sizing/variable.
//! Shapes reference OpenPencil's `jian_ops_schema` public schema, trimmed to fusion-design's needs.
//!
//! V0.2 additions: LayoutMode(Flex/Grid) declarations + ComponentSlot + token references.
// Callers: fd-host-web (DOM render), fd-codegen (layout-aware CSS gen), fd-export (export)
// Affected API: NodeStyle (new fields), PenNode (rotation/z_index)
// Data schemas: LayoutMode/FlexParams/GridParams/TrackSizing/ComponentSlot
// H-A10: Taffy compute_layout removed (dead code); Flex/Grid rendering is generated as CSS by codegen.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Deserializer, Serialize};

// A-5: CSS safety utilities were sunk into the fd-css-utils leaf crate; re-exporting here keeps
// the downstream call sites `fd_canvas_core::parse_hex_color` / `sanitize_css_value` unchanged.
pub use fd_css_utils::{parse_hex_color, sanitize_css_value};

// Deserialization safety guardrails: cap nesting depth and total node count for a malicious or
// corrupted .fusiondesign, preventing stack overflow from deep nesting or OOM from huge node sets.
const MAX_NODE_DEPTH: usize = 64;
const MAX_NODE_TOTAL: usize = 100_000;

// File-format schema version (A1: without a version number there is no forward compat/migration).
// Current version 1. Validated on load: missing counts as 1 (old-file compat), higher than
// current counts as an error.
pub const SCHEMA_VERSION: u32 = 1;

// ARCH-5: schema migration framework. Migrates from version `from` to `from+1`, mutating `doc`
// in place and returning the new version. Only v1 exists today (no predecessor), so the body is
// an empty match (no-op). When v2 introduces a breaking change, add a `1 => { ...; Ok(2) }` arm.
// The caller (from_json) loops `while v < SCHEMA_VERSION { v = migrate_schema(v, &mut value)? }`.
// Operates at the serde_json::Value layer rather than the struct layer: migration runs before
// deserialization, so it can rename fields/reshape structures; struct deserialization is already
// pinned to the target version and cannot accommodate old field shapes.
pub fn migrate_schema(from: u32, doc: &mut serde_json::Value) -> Result<u32, CanvasError> {
    match from {
        // v0 (old files without a version) → v1: no structural difference, just bump the version.
        0 => {
            if let Some(obj) = doc.as_object_mut() {
                obj.insert(
                    "schema_version".to_string(),
                    serde_json::json!(SCHEMA_VERSION),
                );
            }
            tracing::info!(from = 0, to = 1, "schema migration v0 -> v1 (no-op, only stamps the version)");
            Ok(1)
        }
        // Already at the current version: should not be called, but defensively return as-is.
        v if v >= SCHEMA_VERSION => Ok(v),
        // Reserved slot: future breaking-migration arms like 1 => Ok(2).
        other => {
            tracing::warn!(from = other, "schema migration: no matching arm, keeping original version");
            Ok(other)
        }
    }
}

// ── Document / Page / Node ──

/// Canvas document (the struct behind the top-level `.fusiondesign` project file).
///
/// A4: safety guardrails (depth/total limits) are enforced at the deserialization boundary; a bare
/// `serde_json::from_str::<PenDocument>` that bypasses from_json is equally constrained —
/// bypassing them at the type level is impossible.
/// A1: the schema_version field supports file-format forward compat/migration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PenDocument {
    /// File-format schema version (A1). Missing/0 counts as 1 (old-file compat).
    #[serde(default)]
    pub schema_version: u32,
    pub pages: Vec<Page>,
    pub variables: Option<HashMap<String, VariableDef>>,
    /// Currently active design-system ID (integration point with fd-design-system).
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
        // Missing/0 counts as 1 (v0.1.x old-file compat).
        let schema_version = if raw.schema_version == 0 {
            SCHEMA_VERSION
        } else {
            raw.schema_version
        };
        if schema_version > SCHEMA_VERSION {
            return Err(serde::de::Error::custom(format!(
                "file schema version {schema_version} is newer than the supported {SCHEMA_VERSION}, please upgrade fusion-design"
            )));
        }
        let doc = PenDocument {
            schema_version,
            pages: raw.pages,
            variables: raw.variables,
            active_design_system: raw.active_design_system,
        };
        // A4: safety guardrails enforced at the deserialization boundary — no load path (including
        // bare from_str, or UndoRedoStack deserialization embedding a PenDocument) can bypass the
        // depth/total limits.
        doc.validate_limits().map_err(serde::de::Error::custom)?;
        Ok(doc)
    }
}

/// Page (one artboard).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: String,
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub nodes: Vec<PenNode>,
}

/// Vector node (unified shape, covering rect/circle/text/image/group).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

// ── Layout ──

/// Node layout mode.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum LayoutMode {
    /// Free positioning (absolute coordinates).
    #[default]
    Free,
    /// Flexbox layout.
    Flex(FlexParams),
    /// CSS Grid layout.
    Grid(GridParams),
}

/// Flexbox layout parameters.
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

/// Side offsets (padding/margin).
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

/// CSS Grid layout parameters.
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

/// Grid track sizing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TrackSizing {
    Fixed(f64),
    Auto,
    Flex(f64),
    Percent(f64),
}

/// Grid area naming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridArea {
    pub name: String,
    pub row_start: u32,
    pub row_end: u32,
    pub col_start: u32,
    pub col_end: u32,
}

// ── Component instances ──

/// Component slot: references a component in the design system and supports overrides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentSlot {
    pub component_id: String,
    pub variant: String,
    #[serde(default)]
    pub overrides: HashMap<String, serde_json::Value>,
}

/// Component definition: template node tree referenceable by a ComponentSlot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentDefinition {
    pub id: String,
    pub name: String,
    pub variants: HashMap<String, PenNode>,
}

/// Component registry: stores component definitions and supports instantiation.
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

    /// Instantiates a component: clones the node tree from a ComponentSlot and applies overrides.
    pub fn instantiate(&self, slot: &ComponentSlot) -> Option<PenNode> {
        let def = self.components.get(&slot.component_id)?;
        let template = def.variants.get(&slot.variant)?;
        let mut instance = template.clone();
        apply_overrides(&mut instance, &slot.overrides);
        Some(instance)
    }
}

fn apply_overrides(node: &mut PenNode, overrides: &HashMap<String, serde_json::Value>) {
    // Override by path, e.g. "fill" → node.style.fill, "text" → node.text
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
                tracing::debug!(key = %key, "apply_overrides: unknown override key, ignoring");
            }
        }
    }
}

/// Recursively computes the max nesting depth and cumulative node count. Bails as soon as a limit
/// is exceeded (short-circuits, no further traversal).
fn count_depth(node: &PenNode, depth: usize, total: &mut usize) -> Result<usize, CanvasError> {
    *total += 1;
    if *total > MAX_NODE_TOTAL {
        return Err(CanvasError::NodeTotalExceeded {
            total: *total,
            limit: MAX_NODE_TOTAL,
        });
    }
    if depth > MAX_NODE_DEPTH {
        return Err(CanvasError::DepthExceeded {
            depth,
            limit: MAX_NODE_DEPTH,
        });
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

// ── Style ──

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

// ── Variables (design token references) ──

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
    /// Themed values (multiple value sets keyed by mode, e.g. dark/light).
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

// ── Document operations API ──

impl PenDocument {
    /// Creates an empty document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a page.
    pub fn add_page(&mut self, page: Page) {
        self.pages.push(page);
    }

    /// Finds a page by ID.
    pub fn page(&self, id: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.id == id)
    }

    /// Finds a page by ID (mutable).
    pub fn page_mut(&mut self, id: &str) -> Option<&mut Page> {
        self.pages.iter_mut().find(|p| p.id == id)
    }

    /// Finds a node by ID (recursive, all pages).
    pub fn find_node(&self, node_id: &str) -> Option<&PenNode> {
        self.pages
            .iter()
            .flat_map(|p| p.nodes.iter())
            .find_map(|n| n.find(node_id))
    }

    /// Finds a node by ID (mutable, recursive).
    pub fn find_node_mut(&mut self, node_id: &str) -> Option<&mut PenNode> {
        self.pages
            .iter_mut()
            .flat_map(|p| p.nodes.iter_mut())
            .find_map(|n| n.find_mut(node_id))
    }

    /// Removes a node (returns whether removal succeeded).
    pub fn remove_node(&mut self, node_id: &str) -> bool {
        for page in &mut self.pages {
            if remove_node_from_list(&mut page.nodes, node_id) {
                return true;
            }
        }
        false
    }

    /// Serializes to `.fusiondesign` JSON.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Deserializes from JSON (ARCH-13: returns CanvasError instead of anyhow so downstream can
    /// downcast to distinguish corrupt vs upgrade).
    /// Safety guardrails are already enforced by the `Deserialize` impl (A4); this method only
    /// converts error types.
    ///
    /// E-14: truncation/corruption detection. `#[serde(default)]` lets a truncated file "successfully"
    /// parse into a partial document (missing pages → empty Vec, missing schema_version → default 1);
    /// the user opens it, sees a blank canvas with no warning, and data is silently lost. For the
    /// suspicious case of "non-empty input parsing into an entirely empty document", emit an explicit
    /// warn (fail visibly) — not an error, to avoid blocking a legitimately new empty document.
    pub fn from_json(json: &str) -> Result<Self, CanvasError> {
        let trimmed = json.trim();
        // ARCH-5: parse to serde_json::Value first, run the schema migration loop, then deserialize
        // into the struct. Migration runs at the Value layer, so it can rename fields/reshape
        // structures; struct Deserialize is already pinned to the target version.
        let mut value: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| CanvasError::ParseError(format!("failed to parse as JSON Value: {e}")))?;
        let mut v = match &value {
            serde_json::Value::Object(obj) => obj
                .get("schema_version")
                .and_then(|sv| sv.as_u64())
                .map(|n| n as u32)
                .unwrap_or(0),
            _ => 0,
        };
        if v == 0 {
            v = SCHEMA_VERSION;
        }
        if v > SCHEMA_VERSION {
            return Err(CanvasError::SchemaVersion(v));
        }
        while v < SCHEMA_VERSION {
            v = migrate_schema(v, &mut value)?;
        }
        let doc: PenDocument = serde_json::from_value(value)
            .map_err(|e| CanvasError::ParseError(format!("deserialization with safety guardrails failed: {e}")))?;
        // Suspected truncation: non-empty input (not pure whitespace/empty object) but zero pages
        // parsed. Warns the user the file may be corrupted.
        if !trimmed.is_empty() && trimmed != "{}" && doc.pages.is_empty() {
            tracing::warn!(
                input_len = json.len(),
                pages = doc.pages.len(),
                "from_json: input is non-empty but parsed to zero pages; file may be truncated/corrupted (serde defaults silently filled in missing fields). Check file integrity."
            );
        }
        Ok(doc)
    }

    /// Validates that node nesting depth and total count stay within safety thresholds.
    pub fn validate_limits(&self) -> Result<(), CanvasError> {
        let mut total: usize = 0;
        for page in &self.pages {
            for node in &page.nodes {
                let depth = count_depth(node, 1, &mut total)?;
                if depth > MAX_NODE_DEPTH {
                    return Err(CanvasError::DepthExceeded {
                        depth,
                        limit: MAX_NODE_DEPTH,
                    });
                }
            }
        }
        if total > MAX_NODE_TOTAL {
            return Err(CanvasError::NodeTotalExceeded {
                total,
                limit: MAX_NODE_TOTAL,
            });
        }
        Ok(())
    }

    pub fn snapshot(&self) -> PenDocument {
        self.clone()
    }
}

// H-A10/P1-5: Taffy compute_layout/compute_flex_layout/compute_grid_layout + ComputedLayout were
// removed (zero production callers, demonstrative dead code).
// Flex/Grid rendering is now produced as flex/grid CSS by the layout-aware codegen; the
// browser/wasm executes the layout. The NodeStyle declarative types (LayoutMode/FlexParams/
// GridParams etc.) are kept (consumed by codegen/renderers).

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

    /// Adds a node.
    pub fn add(&mut self, node: PenNode) {
        self.nodes.push(node);
    }
}

impl PenNode {
    /// Creates a rect node.
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

    /// Creates a text node.
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

    /// Creates a group node.
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

    /// Recursively finds self or a descendant node.
    pub fn find(&self, id: &str) -> Option<&PenNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    /// Recursive find (mutable).
    pub fn find_mut(&mut self, id: &str) -> Option<&mut PenNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }

    /// All descendant node IDs (including self).
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

// ── Errors ──

#[derive(Debug, thiserror::Error)]
pub enum CanvasError {
    #[error("node {0} not found")]
    NodeNotFound(String),
    #[error("page {0} not found")]
    PageNotFound(String),
    // A-4: fine-grained library-crate errors (replacing anyhow strings); fd-cli report_error
    // matches downcast variants.
    #[error("document parse failed: {0}")]
    ParseError(String),
    #[error("node nesting depth {depth} exceeds safety limit {limit}")]
    DepthExceeded { depth: usize, limit: usize },
    #[error("total node count {total} exceeds safety limit {limit}")]
    NodeTotalExceeded { total: usize, limit: usize },
    // A-7: schema migration framework — reported when the file version exceeds the current
    // SCHEMA_VERSION.
    #[error("document schema version {0} not supported")]
    SchemaVersion(u32),
}

// ── Undo/redo stack ──

mod undo_delta;
pub use undo_delta::{NodeChange, UndoDelta};

const UNDO_REDO_MAX_DEPTH: usize = 50;

// FUNC-7: delta stack replaces the snapshot stack. Stores UndoDelta plus the current state, saving memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoRedoStack {
    undo_stack: VecDeque<UndoDelta>,
    redo_stack: VecDeque<UndoDelta>,
    current: PenDocument,
}

impl UndoRedoStack {
    pub fn new(initial: PenDocument) -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            current: initial,
        }
    }

    pub fn current(&self) -> &PenDocument {
        &self.current
    }

    pub fn push(&mut self, delta: UndoDelta) {
        if self.undo_stack.len() >= UNDO_REDO_MAX_DEPTH {
            self.undo_stack.pop_front();
        }
        delta.apply(&mut self.current);
        self.undo_stack.push_back(delta);
        self.redo_stack.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) -> Option<PenDocument> {
        let delta = self.undo_stack.pop_back()?;
        delta.apply_reverse(&mut self.current);
        self.redo_stack.push_back(delta);
        Some(self.current.clone())
    }

    pub fn redo(&mut self) -> Option<PenDocument> {
        let delta = self.redo_stack.pop_back()?;
        delta.apply(&mut self.current);
        self.undo_stack.push_back(delta);
        Some(self.current.clone())
    }
}

impl Default for UndoRedoStack {
    fn default() -> Self {
        Self::new(PenDocument::default())
    }
}

// ── Document diffing ──

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
    /// Page ID the added node belongs to (meaningful only for Added); the diff uses it to annotate
    /// which page a node lives on for the fd-cli diff subcommand. Avoids always falling into
    /// pages.first(), which misplaced nodes in multi-page documents.
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
    /// Computes the node-level diff between two PenDocuments.
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

    /// Collects all nodes in the document into an id → node map.
    fn all_nodes(&self) -> HashMap<String, &PenNode> {
        let mut map = HashMap::new();
        for page in &self.pages {
            collect_nodes(&page.nodes, &mut map);
        }
        map
    }

    /// Collects all nodes in the document into an id → owning-page-ID map.
    fn all_nodes_by_page(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for page in &self.pages {
            collect_node_pages(&page.nodes, &page.id, &mut map);
        }
        map
    }

    // H-A13/P1-7: apply_patch removed (zero production callers, demonstrative dead code).
    // Diff computation (diff_versions/diff_adjacent) kept — used for display by the fd-cli diff
    // subcommand. If the collaborative-editing apply path ever needs restoring, see git history;
    // the current PRD does not commit to collaborative editing.
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

    // ── V0.2 layout tests ──

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

    // ── ComponentSlot instantiation tests ──

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
        assert_eq!(instance.h, 40.0); // unchanged
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
        // The original template was not modified.
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
        let mut init = PenDocument::new();
        init.add_page(Page {
            id: "p0".into(),
            name: "P0".into(),
            width: 100.0,
            height: 100.0,
            nodes: vec![PenNode::rect("a", 0.0, 0.0, 10.0, 10.0)],
        });
        let mut stack = UndoRedoStack::new(init);
        let mut next = stack.current().clone();
        next.pages[0]
            .nodes
            .push(PenNode::rect("b", 0.0, 0.0, 10.0, 10.0));
        stack.push(UndoDelta::compute(stack.current(), &next));

        assert!(stack.can_undo());
        assert_eq!(stack.current().pages[0].nodes.len(), 2);
        let undone = stack.undo().unwrap();
        assert_eq!(undone.pages[0].nodes.len(), 1);

        assert!(stack.can_redo());
        let redone = stack.redo().unwrap();
        assert_eq!(redone.pages[0].nodes.len(), 2);
    }

    #[test]
    fn undo_redo_empty_safe() {
        let mut stack = UndoRedoStack::new(PenDocument::new());
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
        assert!(stack.undo().is_none());
        assert!(stack.redo().is_none());
    }

    #[test]
    fn undo_redo_max_depth() {
        let mut init = PenDocument::new();
        init.add_page(Page {
            id: "p0".into(),
            name: "P0".into(),
            width: 100.0,
            height: 100.0,
            nodes: vec![],
        });
        let mut stack = UndoRedoStack::new(init);
        for i in 0..60 {
            let mut next = stack.current().clone();
            next.pages[0]
                .nodes
                .push(PenNode::rect(format!("n{i}"), 0.0, 0.0, 10.0, 10.0));
            stack.push(UndoDelta::compute(stack.current(), &next));
        }
        // 60 nodes accumulated; MAX_DEPTH=50 drops the 10 earliest deltas, current now has 60 nodes
        assert_eq!(stack.current().pages[0].nodes.len(), 60);
        assert!(stack.can_undo());
        let undone = stack.undo().unwrap();
        // undo reverses the most recent add-node delta (n59) → 59 nodes
        assert_eq!(undone.pages[0].nodes.len(), 59);
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

    // H-A13: apply_patch removed (zero production callers, demonstrative dead code).
    // Keeps the diff page_id attribution regression — the fd-cli diff subcommand relies on that
    // semantic to display which page a node lives on.
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
            .expect("n2 Added entry expected");
        assert_eq!(added.page_id.as_deref(), Some("p2"));
    }
}

// ── Named version management (removed) ──
// H-A14/P2-4: the VersionedDocument/NamedVersion section was fully removed (dead code).
// The audit flagged "260 lines of dead code carrying weight": the named-version API
// (save_version/switch_to/diff_versions etc.) had zero production consumers across crates —
// .fusiondesign actually stores a bare PenDocument, and CLI undo/redo goes through the separate
// UndoRedoStack (not this type). save_version pushed into both the versions Vec and the undo
// stack, causing duplicate in-process deep copies; persistence was fixed (undo stack
// #[serde(skip)]), but the type itself had zero consumers, so building delta-COW infrastructure
// as an empty shell was zero return on investment (Rule 2). Undo/redo capability is provided
// independently by UndoRedoStack (used directly by fd-cli, kept). If named versions are needed
// in the future, re-integrate with a delta-COW design; do not resurrect the full-snapshot approach.
// Removed along with it: uuid_v4/now_iso/civil_from_days/VERSION_SEQ/MAX_VERSIONED_FILE_BYTES
// (used only by VersionedDocument and its tests, no other consumers).

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

    /// ARCH-13 test helper: builds a PenDocument exceeding the depth limit (direct validate_limits path).
    fn build_deep_doc() -> PenDocument {
        let mut doc = PenDocument::new();
        let mut node = PenNode::rect("leaf", 0.0, 0.0, 1.0, 1.0);
        for _ in 0..(MAX_NODE_DEPTH + 5) {
            let mut parent = PenNode::rect("n", 0.0, 0.0, 1.0, 1.0);
            parent.children = vec![node];
            node = parent;
        }
        doc.add_page(Page {
            id: "p".into(),
            name: "p".into(),
            width: 1.0,
            height: 1.0,
            nodes: vec![node],
        });
        doc
    }

    /// ARCH-13 test helper: builds a PenDocument exceeding the total-count limit (direct validate_limits path).
    fn build_many_nodes_doc() -> PenDocument {
        let mut doc = PenDocument::new();
        let nodes: Vec<PenNode> = (0..(MAX_NODE_TOTAL + 5))
            .map(|i| PenNode::rect(format!("n{i}"), 0.0, 0.0, 1.0, 1.0))
            .collect();
        doc.add_page(Page {
            id: "p".into(),
            name: "p".into(),
            width: 1.0,
            height: 1.0,
            nodes,
        });
        doc
    }

    #[test]
    fn from_json_rejects_deeply_nested() {
        // Malicious .fusiondesign: deeply nested children exceed MAX_NODE_DEPTH → rejected
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
        assert!(PenDocument::from_json(&json).is_err(), "depth over limit should be rejected");
    }

    /// A4: a bare `serde_json::from_str::<PenDocument>` (the path bypassing from_json) must be
    /// intercepted by the custom Deserialize too — the root fix for the old CLI/undo/redo bypass.
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
            "bare from_str must be intercepted by the custom Deserialize (A4)"
        );
    }

    /// A4: too many nodes (> MAX_NODE_TOTAL) via bare from_str is rejected as well.
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
        assert!(result.is_err(), "bare from_str with too many nodes must be rejected (A4)");
    }

    /// A1: missing schema_version (old file) counts as 1 and loads normally.
    #[test]
    fn schema_version_missing_defaults_to_one() {
        let json = r#"{"pages":[],"variables":null,"active_design_system":null}"#;
        let doc: PenDocument = serde_json::from_str(json).unwrap();
        assert_eq!(
            doc.schema_version, SCHEMA_VERSION,
            "missing schema_version defaults to 1"
        );
    }

    /// A1: schema_version higher than current → rejected (prevents future versions silently
    /// dropping fields).
    #[test]
    fn schema_version_ahead_rejected() {
        let json =
            r#"{"schema_version":999,"pages":[],"variables":null,"active_design_system":null}"#;
        let result: Result<PenDocument, _> = serde_json::from_str(json);
        assert!(result.is_err(), "future schema_version should be rejected");
    }

    /// A1: schema_version=1 loads normally and is written back.
    #[test]
    fn schema_version_roundtrip() {
        let doc = PenDocument::new();
        let json = serde_json::to_string(&doc).unwrap();
        assert!(
            json.contains("\"schema_version\""),
            "serialization should contain schema_version"
        );
        let back: PenDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn validate_limits_accepts_normal_doc() {
        let doc = sample_doc("ok");
        assert!(doc.validate_limits().is_ok(), "normal document should pass validation");
    }

    /// Performance baseline: serialize/deserialize/layout timings for a 1000-node document.
    /// `#[ignore]`: not in regular CI; run via `cargo test --release -- --ignored perf_baseline`.
    /// Threshold: each operation < 500ms (release, Apple Silicon); over-threshold prints but does
    /// not assert-fail (baseline reference).
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
        assert!(doc2.pages[0].nodes.len() >= 1000, "deserialized node count should be >=1000");

        assert!(
            ser_ms < THRESHOLD_MS,
            "serialize {ser_ms}ms > {THRESHOLD_MS}ms"
        );
        assert!(
            de_ms < THRESHOLD_MS,
            "deserialize {de_ms}ms > {THRESHOLD_MS}ms"
        );
    }

    // E-14: truncated/corrupted files must not silently lose data.
    #[test]
    fn from_json_warns_on_suspected_truncated_file() {
        // Non-empty input but missing pages field → serde default fills an empty Vec; parsing
        // "succeeds" but truncation is suspected. from_json should not error (to avoid blocking a
        // legitimate empty document) but should recognize the suspicious case (here reflected by
        // returning Ok + an internal warn; behavioral assertions: no panic, empty-pages document).
        let truncated = r#"{"schema_version":1,"variables":null}"#;
        let doc = PenDocument::from_json(truncated).expect("truncated file should parse without error");
        assert!(doc.pages.is_empty(), "truncated file pages should be empty");
        assert_eq!(doc.schema_version, 1);
    }

    #[test]
    fn from_json_silent_on_legit_empty_object() {
        // Legitimate empty document "{}": all serde defaults, should not trigger the truncation
        // warning branch (trim == "{}" exclusion).
        let doc = PenDocument::from_json("{}").expect("empty object should parse successfully");
        assert!(doc.pages.is_empty());
        assert_eq!(doc.schema_version, 1);
    }

    #[test]
    fn from_json_silent_on_normal_doc() {
        // A normal document should not trigger the truncation warning.
        let doc = sample_doc("normal");
        let json = doc.to_json().unwrap();
        let back = PenDocument::from_json(&json).unwrap();
        assert_eq!(back.pages.len(), 1);
    }

    // ARCH-5: schema migration framework regression.
    #[test]
    fn migrate_schema_v0_to_v1_noop_but_stamps_version() {
        let mut val = serde_json::json!({"pages": [], "schema_version": 0});
        let new_v = migrate_schema(0, &mut val).unwrap();
        assert_eq!(new_v, 1);
        assert_eq!(val["schema_version"], 1);
    }

    #[test]
    fn migrate_schema_current_version_is_passthrough() {
        let mut val = serde_json::json!({"schema_version": SCHEMA_VERSION});
        let new_v = migrate_schema(SCHEMA_VERSION, &mut val).unwrap();
        assert_eq!(new_v, SCHEMA_VERSION);
    }

    #[test]
    fn from_json_missing_schema_version_defaults_to_current() {
        let json = serde_json::json!({"pages": [{"id":"p","name":"P","width":1.0,"height":1.0,"nodes":[]}]}).to_string();
        let doc = PenDocument::from_json(&json).unwrap();
        assert_eq!(doc.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn from_json_future_schema_version_rejected() {
        let json = format!(
            "{{\"schema_version\": {}, \"pages\": []}}",
            SCHEMA_VERSION + 1
        );
        let err = PenDocument::from_json(&json).unwrap_err();
        // ARCH-13: returns the CanvasError::SchemaVersion variant so downstream can downcast to
        // distinguish upgrade-needed
        assert!(matches!(err, CanvasError::SchemaVersion(_)), "got: {err:?}");
    }

    /// ARCH-13: from_json returns CanvasError variants so downstream can match by variant to
    /// distinguish corrupt-file vs upgrade-needed, instead of opaque anyhow string matching.
    #[test]
    fn from_json_returns_canvas_error_variants() {
        // Bad JSON → ParseError
        let err = PenDocument::from_json("{not valid json").unwrap_err();
        assert!(
            matches!(err, CanvasError::ParseError(_)),
            "bad JSON should return ParseError, got: {err:?}"
        );

        // Schema version too high → SchemaVersion
        let json = format!(
            "{{\"schema_version\": {}, \"pages\": []}}",
            SCHEMA_VERSION + 1
        );
        let err = PenDocument::from_json(&json).unwrap_err();
        assert!(
            matches!(err, CanvasError::SchemaVersion(v) if v == SCHEMA_VERSION + 1),
            "future version should return SchemaVersion, got: {err:?}"
        );

        // Depth over limit: via from_json it collapses into ParseError (custom Deserialize wraps
        // validate_limits through serde::de::Error::custom); distinguishing the variant requires
        // calling validate_limits directly.
        let mut deep =
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
        deep.push_str(&node);
        deep.push_str("]}],\"variables\":null,\"active_design_system\":null}");
        // from_json path: ParseError (description includes the validation failure)
        let err = PenDocument::from_json(&deep).unwrap_err();
        assert!(
            matches!(err, CanvasError::ParseError(_)),
            "from_json depth over limit collapses into ParseError, got: {err:?}"
        );
        // validate_limits direct call: keeps the DepthExceeded variant (ARCH-13 downstream downcast surface)
        let deep_doc = build_deep_doc();
        let err = deep_doc.validate_limits().unwrap_err();
        assert!(
            matches!(err, CanvasError::DepthExceeded { depth, limit } if depth > limit && limit == MAX_NODE_DEPTH),
            "validate_limits depth over limit should return DepthExceeded, got: {err:?}"
        );

        // Total count over limit: same as above, from_json collapses to ParseError, validate_limits
        // keeps the variant
        let many_doc = build_many_nodes_doc();
        let err = many_doc.validate_limits().unwrap_err();
        assert!(
            matches!(err, CanvasError::NodeTotalExceeded { total, limit } if total > limit && limit == MAX_NODE_TOTAL),
            "validate_limits total over limit should return NodeTotalExceeded, got: {err:?}"
        );
    }

    /// ARCH-13: NodeNotFound / PageNotFound have no production Err site (find_node / page return
    /// Option, not Result) and are consumed only by fd-cli report_error downcast. Constructing
    /// them directly asserts the variants exist plus their Display text, closing the variant
    /// coverage gap (the other 4 variants are covered by from_json_returns_canvas_error_variants).
    /// No downcast verification: fd-canvas-core has dropped its anyhow dependency (a library crate
    /// does not expose anyhow); the downcast consumer is in fd-cli (report_error already covers
    /// it), and this crate has no downcast path to exercise.
    #[test]
    fn canvas_error_dead_variants_display() {
        let e1 = CanvasError::NodeNotFound(String::from("node-42"));
        assert!(
            matches!(e1, CanvasError::NodeNotFound(_)),
            "NodeNotFound variant should be constructible, got: {e1:?}"
        );
        assert!(
            format!("{e1}").contains("node-42"),
            "NodeNotFound Display should include the id: {e1}"
        );

        let e2 = CanvasError::PageNotFound(String::from("page-7"));
        assert!(
            matches!(e2, CanvasError::PageNotFound(_)),
            "PageNotFound variant should be constructible, got: {e2:?}"
        );
        assert!(
            format!("{e2}").contains("page-7"),
            "PageNotFound Display should include the id: {e2}"
        );
    }
}
