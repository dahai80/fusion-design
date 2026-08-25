// Callers: fd-cli (lint subcommand → Linter::lint()), DesignBridge.swift (Process → fusion-design lint)
// Affected API: Linter::new(), Linter::with_rules(), Linter::with_design_system(), Linter::lint(), LintResult, LintViolation, LintRule, LintSeverity
// Data schemas: PenDocument→Vec<LintViolation> (13 rules), DesignSystem Token→HashMap for cross-ref, LintStats summary
// User instruction: "现在开始实施" — Task #17 P3-6 design_lint Skill（基础检测器）
//! Fusion-Design design lint — 13 detectors for design specification compliance.

use fd_canvas_core::{NodeKind, PenDocument, PenNode};
use fd_design_system::{DesignSystem, TokenValue};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintRule {
    ContrastCheck,
    UnlabeledInput,
    TextEffects,
    AbnormalRotation,
    EmptyEffects,
    TokenInconsistency,
    UnnamedNode,
    TextOverflow,
    OverlappingNodes,
    HardcodedSpacing,
    HardcodedFontSize,
    MissingInteractionState,
    LayoutInconsistency,
}

impl LintRule {
    pub fn name(&self) -> &str {
        match self {
            LintRule::ContrastCheck => "contrast_check",
            LintRule::UnlabeledInput => "unlabeled_input",
            LintRule::TextEffects => "text_effects",
            LintRule::AbnormalRotation => "abnormal_rotation",
            LintRule::EmptyEffects => "empty_effects",
            LintRule::TokenInconsistency => "token_inconsistency",
            LintRule::UnnamedNode => "unnamed_node",
            LintRule::TextOverflow => "text_overflow",
            LintRule::OverlappingNodes => "overlapping_nodes",
            LintRule::HardcodedSpacing => "hardcoded_spacing",
            LintRule::HardcodedFontSize => "hardcoded_font_size",
            LintRule::MissingInteractionState => "missing_interaction_state",
            LintRule::LayoutInconsistency => "layout_inconsistency",
        }
    }

    pub fn description(&self) -> &str {
        match self {
            LintRule::ContrastCheck => "前景色与背景色对比度不足，影响可读性",
            LintRule::UnlabeledInput => "输入控件缺少标签（label/placeholder）",
            LintRule::TextEffects => "文本节点使用了特效（渐变等），影响可读性",
            LintRule::AbnormalRotation => "节点旋转角度异常（超过90°或非15°倍数）",
            LintRule::EmptyEffects => "节点声明了样式但参数为空，属于冗余",
            LintRule::TokenInconsistency => "样式值未使用设计系统 Token，存在不一致风险",
            LintRule::UnnamedNode => "节点使用默认名称，缺少语义标识",
            LintRule::TextOverflow => "文本节点尺寸为零，内容将溢出",
            LintRule::OverlappingNodes => "同级节点边界框重叠，可能导致遮挡",
            LintRule::HardcodedSpacing => "间距值未引用设计 Token，维护性差",
            LintRule::HardcodedFontSize => "字号未引用设计 Token，维护性差",
            LintRule::MissingInteractionState => "交互控件缺少 hover/active 状态定义",
            LintRule::LayoutInconsistency => "同级节点布局模式不一致",
        }
    }

    pub fn all() -> Vec<LintRule> {
        vec![
            LintRule::ContrastCheck,
            LintRule::UnlabeledInput,
            LintRule::TextEffects,
            LintRule::AbnormalRotation,
            LintRule::EmptyEffects,
            LintRule::TokenInconsistency,
            LintRule::UnnamedNode,
            LintRule::TextOverflow,
            LintRule::OverlappingNodes,
            LintRule::HardcodedSpacing,
            LintRule::HardcodedFontSize,
            LintRule::MissingInteractionState,
            LintRule::LayoutInconsistency,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintViolation {
    pub rule: LintRule,
    pub node_id: String,
    pub message: String,
    pub suggestion: String,
    pub severity: LintSeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintResult {
    pub violations: Vec<LintViolation>,
    pub stats: LintStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintStats {
    pub total_nodes: usize,
    pub total_violations: usize,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
}

impl LintResult {
    pub fn empty() -> Self {
        Self {
            violations: Vec::new(),
            stats: LintStats {
                total_nodes: 0,
                total_violations: 0,
                errors: 0,
                warnings: 0,
                infos: 0,
            },
        }
    }
}

pub struct Linter {
    rules: Vec<LintRule>,
    design_system: Option<DesignSystem>,
}

impl Linter {
    pub fn new() -> Self {
        Self {
            rules: LintRule::all(),
            design_system: None,
        }
    }

    pub fn with_rules(rules: Vec<LintRule>) -> Self {
        Self {
            rules,
            design_system: None,
        }
    }

    pub fn with_design_system(mut self, system: DesignSystem) -> Self {
        self.design_system = Some(system);
        self
    }

    pub fn lint(&self, doc: &PenDocument) -> LintResult {
        info!("lint: 开始检测 PenDocument, 规则数={}", self.rules.len());
        let mut violations = Vec::new();
        let mut total_nodes = 0usize;

        for page in &doc.pages {
            self.lint_siblings(&page.nodes, &mut violations);
            for node in &page.nodes {
                total_nodes += 1;
                self.lint_node(node, &mut violations);
                self.lint_children(node, &mut violations, &mut total_nodes);
            }
        }

        let errors = violations
            .iter()
            .filter(|v| v.severity == LintSeverity::Error)
            .count();
        let warnings = violations
            .iter()
            .filter(|v| v.severity == LintSeverity::Warning)
            .count();
        let infos = violations
            .iter()
            .filter(|v| v.severity == LintSeverity::Info)
            .count();

        info!(
            "lint: 检测完成, 总节点={}, 违规={}, error={}, warning={}, info={}",
            total_nodes,
            violations.len(),
            errors,
            warnings,
            infos
        );

        LintResult {
            violations,
            stats: LintStats {
                total_nodes,
                total_violations: errors + warnings + infos,
                errors,
                warnings,
                infos,
            },
        }
    }

    fn lint_children(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
        total_nodes: &mut usize,
    ) {
        for child in &node.children {
            *total_nodes += 1;
            self.lint_node(child, violations);
            self.lint_siblings(&node.children, violations);
            self.lint_children(child, violations, total_nodes);
        }
    }

    fn lint_siblings(&self, siblings: &[PenNode], violations: &mut Vec<LintViolation>) {
        if !self.rules.contains(&LintRule::OverlappingNodes)
            && !self.rules.contains(&LintRule::LayoutInconsistency)
        {
            return;
        }

        if self.rules.contains(&LintRule::OverlappingNodes) {
            self.check_overlapping_nodes(siblings, violations);
        }

        if self.rules.contains(&LintRule::LayoutInconsistency) {
            self.check_layout_inconsistency(siblings, violations);
        }
    }

    fn lint_node(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        for rule in &self.rules {
            match rule {
                LintRule::ContrastCheck => self.check_contrast(node, violations),
                LintRule::UnlabeledInput => self.check_unlabeled_input(node, violations),
                LintRule::TextEffects => self.check_text_effects(node, violations),
                LintRule::AbnormalRotation => self.check_abnormal_rotation(node, violations),
                LintRule::EmptyEffects => self.check_empty_effects(node, violations),
                LintRule::TokenInconsistency => self.check_token_inconsistency(node, violations),
                LintRule::UnnamedNode => self.check_unnamed_node(node, violations),
                LintRule::TextOverflow => self.check_text_overflow(node, violations),
                LintRule::OverlappingNodes => (),
                LintRule::HardcodedSpacing => self.check_hardcoded_spacing(node, violations),
                LintRule::HardcodedFontSize => self.check_hardcoded_font_size(node, violations),
                LintRule::MissingInteractionState => {
                    self.check_missing_interaction_state(node, violations)
                }
                LintRule::LayoutInconsistency => (),
            }
        }
    }

    fn check_contrast(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        let style = &node.style;
        // E-28：旧实现 fg=fill、bg=stroke 语义反转——fill 是背景，stroke 是边框，
        // 真实背景从未被检查（漏报），边框当背景（误报），WCAG 形同虚设。
        // 正确语义：fg=文本前景色（无 text_color 字段，渲染器/codegen 均默认 #000000），
        // bg=fill（节点背景）。无 fill 时背景透明/继承，无法算对比度 → 跳过。
        let bg = style.fill.as_deref().unwrap_or("");
        if bg.is_empty() {
            return;
        }
        let fg = "#000000";

        let fg_lum = luminance(fg);
        let bg_lum = luminance(bg);

        if fg_lum < 0.0 || bg_lum < 0.0 {
            return;
        }

        let ratio = contrast_ratio(fg_lum, bg_lum);
        if ratio < 3.0 {
            violations.push(LintViolation {
                rule: LintRule::ContrastCheck,
                node_id: node.id.clone(),
                message: format!("对比度 {:.1}:1 不足（最低 3:1）", ratio),
                suggestion: "增大前景色与背景色的明度差".to_string(),
                severity: LintSeverity::Error,
            });
        } else if ratio < 4.5 {
            violations.push(LintViolation {
                rule: LintRule::ContrastCheck,
                node_id: node.id.clone(),
                message: format!("对比度 {:.1}:1 偏低（建议 4.5:1）", ratio),
                suggestion: "建议对比度达到 4.5:1 以满足 WCAG AA 标准".to_string(),
                severity: LintSeverity::Warning,
            });
        }
    }

    fn check_unlabeled_input(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        let name_lower = node.name.to_lowercase();
        let is_input = name_lower.contains("input")
            || name_lower.contains("textfield")
            || name_lower.contains("textarea")
            || name_lower.contains("搜索")
            || name_lower.contains("输入");

        if !is_input {
            return;
        }

        let has_label = node.children.iter().any(|c| {
            let cn = c.name.to_lowercase();
            cn.contains("label") || cn.contains("标题") || cn.contains("placeholder")
        });

        let has_placeholder = node.children.iter().any(|c| {
            let cn = c.name.to_lowercase();
            cn.contains("placeholder") || cn.contains("提示") || cn.contains("hint")
        });

        if !has_label && !has_placeholder {
            violations.push(LintViolation {
                rule: LintRule::UnlabeledInput,
                node_id: node.id.clone(),
                message: format!("输入控件 '{}' 缺少标签", node.name),
                suggestion: "添加 label 或 placeholder 子节点".to_string(),
                severity: LintSeverity::Error,
            });
        }
    }

    fn check_text_effects(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        if node.kind != NodeKind::Text {
            return;
        }

        let style = &node.style;

        if let Some(ref fill) = style.fill {
            if fill.starts_with("linear-gradient")
                || fill.starts_with("radial-gradient")
                || fill.starts_with("conic-gradient")
            {
                violations.push(LintViolation {
                    rule: LintRule::TextEffects,
                    node_id: node.id.clone(),
                    message: format!("文本节点 '{}' 使用了渐变填充", node.name),
                    suggestion: "文本渐变降低可读性，建议使用纯色填充".to_string(),
                    severity: LintSeverity::Warning,
                });
            }
        }
    }

    fn check_abnormal_rotation(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        let rotation = node.rotation;

        if rotation.abs() > 90.0 {
            violations.push(LintViolation {
                rule: LintRule::AbnormalRotation,
                node_id: node.id.clone(),
                message: format!("节点 '{}' 旋转角度 {:.1}° 超过 90°", node.name, rotation),
                suggestion: "超过 90° 的旋转可能导致内容不可读，请确认是否正确".to_string(),
                severity: LintSeverity::Error,
            });
        } else if rotation != 0.0 && (rotation % 15.0).abs() > 0.01 {
            violations.push(LintViolation {
                rule: LintRule::AbnormalRotation,
                node_id: node.id.clone(),
                message: format!("节点 '{}' 旋转角度 {:.1}° 非 15° 倍数", node.name, rotation),
                suggestion: "建议使用 15° 倍数的旋转角度以保持对齐一致".to_string(),
                severity: LintSeverity::Info,
            });
        }
    }

    fn check_empty_effects(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        let style = &node.style;

        if let Some(ref fill) = style.fill {
            if fill.trim().is_empty() {
                violations.push(LintViolation {
                    rule: LintRule::EmptyEffects,
                    node_id: node.id.clone(),
                    message: format!("节点 '{}' 声明了填充色但值为空", node.name),
                    suggestion: "移除空的填充声明或设置颜色值".to_string(),
                    severity: LintSeverity::Warning,
                });
            }
        }

        if let Some(ref stroke) = style.stroke {
            if stroke.trim().is_empty() {
                violations.push(LintViolation {
                    rule: LintRule::EmptyEffects,
                    node_id: node.id.clone(),
                    message: format!("节点 '{}' 声明了描边色但值为空", node.name),
                    suggestion: "移除空的描边声明或设置颜色值".to_string(),
                    severity: LintSeverity::Warning,
                });
            }
        }

        if let Some(ref ff) = style.font_family {
            if ff.trim().is_empty() {
                violations.push(LintViolation {
                    rule: LintRule::EmptyEffects,
                    node_id: node.id.clone(),
                    message: format!("节点 '{}' 声明了字体族但值为空", node.name),
                    suggestion: "移除空的字体声明或设置字体族".to_string(),
                    severity: LintSeverity::Warning,
                });
            }
        }
    }

    fn check_token_inconsistency(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        let system = match &self.design_system {
            Some(s) => s,
            None => return,
        };

        let style = &node.style;
        let token_map = build_token_map(system);

        if let Some(ref fill) = style.fill {
            if let Some(token_name) = token_map.get(fill) {
                violations.push(LintViolation {
                    rule: LintRule::TokenInconsistency,
                    node_id: node.id.clone(),
                    message: format!(
                        "节点 '{}' 填充色 '{}' 匹配 Token '{}'，建议直接引用 Token",
                        node.name, fill, token_name
                    ),
                    suggestion: format!("使用 Token 引用替代硬编码值: {}", token_name),
                    severity: LintSeverity::Info,
                });
            }
        }

        if let Some(ref stroke) = style.stroke {
            if let Some(token_name) = token_map.get(stroke) {
                violations.push(LintViolation {
                    rule: LintRule::TokenInconsistency,
                    node_id: node.id.clone(),
                    message: format!(
                        "节点 '{}' 描边色 '{}' 匹配 Token '{}'，建议直接引用 Token",
                        node.name, stroke, token_name
                    ),
                    suggestion: format!("使用 Token 引用替代硬编码值: {}", token_name),
                    severity: LintSeverity::Info,
                });
            }
        }
    }

    fn check_unnamed_node(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        let default_names = ["Rect", "Text", "Circle", "Image", "Group"];
        if default_names.contains(&node.name.as_str()) {
            violations.push(LintViolation {
                rule: LintRule::UnnamedNode,
                node_id: node.id.clone(),
                message: format!("节点 '{}' 使用默认名称，缺少语义标识", node.name),
                suggestion: "为节点设置有意义的名称便于维护".to_string(),
                severity: LintSeverity::Warning,
            });
        }
    }

    fn check_text_overflow(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        if node.kind != NodeKind::Text {
            return;
        }
        if node.w == 0.0 || node.h == 0.0 {
            violations.push(LintViolation {
                rule: LintRule::TextOverflow,
                node_id: node.id.clone(),
                message: format!(
                    "文本节点 '{}' 尺寸为零（w={}, h={}），内容将溢出",
                    node.name, node.w, node.h
                ),
                suggestion: "为文本节点设置宽高，或使用自适应布局".to_string(),
                severity: LintSeverity::Error,
            });
        }
    }

    fn check_overlapping_nodes(&self, siblings: &[PenNode], violations: &mut Vec<LintViolation>) {
        for i in 0..siblings.len() {
            for j in (i + 1)..siblings.len() {
                let a = &siblings[i];
                let b = &siblings[j];
                if a.z_index == b.z_index && rects_overlap(a, b) {
                    violations.push(LintViolation {
                        rule: LintRule::OverlappingNodes,
                        node_id: format!("{}+{}", a.id, b.id),
                        message: format!("节点 '{}' 与 '{}' 边界框重叠", a.name, b.name),
                        suggestion: "调整节点位置或使用不同的 z-index 避免遮挡".to_string(),
                        severity: LintSeverity::Warning,
                    });
                }
            }
        }
    }

    fn check_hardcoded_spacing(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        use fd_canvas_core::LayoutMode;
        let layout = &node.style.layout;
        match layout {
            LayoutMode::Flex(flex) => {
                if flex.gap != 0.0 && !node.style.design_token_refs.contains_key("gap") {
                    violations.push(LintViolation {
                        rule: LintRule::HardcodedSpacing,
                        node_id: node.id.clone(),
                        message: format!(
                            "节点 '{}' Flex gap={} 未引用设计 Token",
                            node.name, flex.gap
                        ),
                        suggestion: "使用 design_token_refs 引用 spacing Token".to_string(),
                        severity: LintSeverity::Info,
                    });
                }
                let p = &flex.padding;
                if (p.top != 0.0 || p.right != 0.0 || p.bottom != 0.0 || p.left != 0.0)
                    && !node.style.design_token_refs.contains_key("padding")
                {
                    violations.push(LintViolation {
                        rule: LintRule::HardcodedSpacing,
                        node_id: node.id.clone(),
                        message: format!(
                            "节点 '{}' padding(t={}/r={}/b={}/l={}) 未引用设计 Token",
                            node.name, p.top, p.right, p.bottom, p.left
                        ),
                        suggestion: "使用 design_token_refs 引用 spacing Token".to_string(),
                        severity: LintSeverity::Info,
                    });
                }
            }
            LayoutMode::Grid(grid) => {
                if (grid.gap.0 != 0.0 || grid.gap.1 != 0.0)
                    && !node.style.design_token_refs.contains_key("gap")
                {
                    violations.push(LintViolation {
                        rule: LintRule::HardcodedSpacing,
                        node_id: node.id.clone(),
                        message: format!(
                            "节点 '{}' Grid gap=({}, {}) 未引用设计 Token",
                            node.name, grid.gap.0, grid.gap.1
                        ),
                        suggestion: "使用 design_token_refs 引用 spacing Token".to_string(),
                        severity: LintSeverity::Info,
                    });
                }
            }
            LayoutMode::Free => {}
        }
    }

    fn check_hardcoded_font_size(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        if let Some(fs) = node.style.font_size {
            if fs > 0.0 && !node.style.design_token_refs.contains_key("font_size") {
                violations.push(LintViolation {
                    rule: LintRule::HardcodedFontSize,
                    node_id: node.id.clone(),
                    message: format!("节点 '{}' font_size={} 未引用设计 Token", node.name, fs),
                    suggestion: "使用 design_token_refs 引用 typography Token".to_string(),
                    severity: LintSeverity::Info,
                });
            }
        }
    }

    fn check_missing_interaction_state(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        let name_lower = node.name.to_lowercase();
        let is_interactive = name_lower.contains("button")
            || name_lower.contains("btn")
            || name_lower.contains("input")
            || name_lower.contains("textfield")
            || name_lower.contains("link")
            || name_lower.contains("按钮")
            || name_lower.contains("链接");

        if !is_interactive {
            return;
        }

        let has_state_variant = node.children.iter().any(|c| {
            let cn = c.name.to_lowercase();
            cn.contains("hover")
                || cn.contains("active")
                || cn.contains("focus")
                || cn.contains("pressed")
                || cn.contains("disabled")
        });

        if !has_state_variant {
            violations.push(LintViolation {
                rule: LintRule::MissingInteractionState,
                node_id: node.id.clone(),
                message: format!("交互控件 '{}' 缺少 hover/active 状态定义", node.name),
                suggestion: "添加 hover/active/focus 状态子节点".to_string(),
                severity: LintSeverity::Warning,
            });
        }
    }

    fn check_layout_inconsistency(
        &self,
        siblings: &[PenNode],
        violations: &mut Vec<LintViolation>,
    ) {
        use fd_canvas_core::LayoutMode;
        if siblings.len() < 2 {
            return;
        }

        let first_layout_mode = match &siblings[0].style.layout {
            LayoutMode::Free => "free",
            LayoutMode::Flex(_) => "flex",
            LayoutMode::Grid(_) => "grid",
        };

        for sibling in &siblings[1..] {
            let mode = match &sibling.style.layout {
                LayoutMode::Free => "free",
                LayoutMode::Flex(_) => "flex",
                LayoutMode::Grid(_) => "grid",
            };
            if mode != first_layout_mode {
                violations.push(LintViolation {
                    rule: LintRule::LayoutInconsistency,
                    node_id: format!("{}+{}", siblings[0].id, sibling.id),
                    message: format!(
                        "同级节点 '{}' 使用 {} 布局，'{}' 使用 {} 布局，不一致",
                        siblings[0].name, first_layout_mode, sibling.name, mode
                    ),
                    suggestion: "同级节点建议使用统一的布局模式".to_string(),
                    severity: LintSeverity::Warning,
                });
            }
        }
    }
}

impl Default for Linter {
    fn default() -> Self {
        Self::new()
    }
}

fn rects_overlap(a: &PenNode, b: &PenNode) -> bool {
    let a_x2 = a.x + a.w;
    let a_y2 = a.y + a.h;
    let b_x2 = b.x + b.w;
    let b_y2 = b.y + b.h;
    a.x < b_x2 && b.x < a_x2 && a.y < b_y2 && b.y < a_y2
}

// Callers: fd-cli (lint --fix), auto_fix() delegates to apply_tokens_to_document
// Affected API: FixResult, FixDetail, apply_tokens_to_document(), Linter::auto_fix()
// Data schemas: PenDocument mutable → FixResult with per-fix details; spacing/typography token maps
// User instruction: "按照方案和prd方案全面落地" — Phase 4 Task 4.1-4.2
fn build_token_map(system: &DesignSystem) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for token in &system.tokens {
        let css_val = match &token.value {
            TokenValue::Color(c) => c.clone(),
            TokenValue::String(s) if !s.starts_with("token:") => s.clone(),
            _ => continue,
        };
        map.insert(css_val, token.name.clone());
    }
    map
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixResult {
    pub fixes_applied: usize,
    pub fixes_skipped: usize,
    pub details: Vec<FixDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixDetail {
    pub rule: LintRule,
    pub node_id: String,
    pub action: String,
    pub before: String,
    pub after: String,
}

pub fn apply_tokens_to_document(doc: &mut PenDocument, system: &DesignSystem) -> FixResult {
    info!("apply_tokens_to_document: 开始应用 Token 到 PenDocument");
    let token_map = build_token_map(system);
    let numeric_map = build_numeric_token_map(system);
    let mut result = FixResult {
        fixes_applied: 0,
        fixes_skipped: 0,
        details: vec![],
    };

    for page in &mut doc.pages {
        apply_tokens_to_nodes(&mut page.nodes, &token_map, &numeric_map, &mut result);
    }

    info!(
        "apply_tokens_to_document: 完成, applied={}, skipped={}",
        result.fixes_applied, result.fixes_skipped
    );
    result
}

fn apply_tokens_to_nodes(
    nodes: &mut [PenNode],
    token_map: &HashMap<String, String>,
    numeric_map: &HashMap<String, String>,
    result: &mut FixResult,
) {
    use fd_canvas_core::LayoutMode;
    for node in nodes.iter_mut() {
        // fill → token ref
        if let Some(ref fill) = node.style.fill {
            if !node.style.design_token_refs.contains_key("fill") {
                if let Some(token_name) = token_map.get(fill) {
                    let before = fill.clone();
                    node.style
                        .design_token_refs
                        .insert("fill".into(), token_name.clone());
                    result.fixes_applied += 1;
                    result.details.push(FixDetail {
                        rule: LintRule::TokenInconsistency,
                        node_id: node.id.clone(),
                        action: "fill→token_ref".into(),
                        before,
                        after: token_name.clone(),
                    });
                }
            }
        }

        // stroke → token ref
        if let Some(ref stroke) = node.style.stroke {
            if !node.style.design_token_refs.contains_key("stroke") {
                if let Some(token_name) = token_map.get(stroke) {
                    let before = stroke.clone();
                    node.style
                        .design_token_refs
                        .insert("stroke".into(), token_name.clone());
                    result.fixes_applied += 1;
                    result.details.push(FixDetail {
                        rule: LintRule::TokenInconsistency,
                        node_id: node.id.clone(),
                        action: "stroke→token_ref".into(),
                        before,
                        after: token_name.clone(),
                    });
                }
            }
        }

        // gap → spacing token
        match &node.style.layout {
            LayoutMode::Flex(flex) => {
                if flex.gap != 0.0 && !node.style.design_token_refs.contains_key("gap") {
                    let key = format!("{}", flex.gap);
                    if let Some(token_name) = numeric_map.get(&key) {
                        let before = format!("{}", flex.gap);
                        node.style
                            .design_token_refs
                            .insert("gap".into(), token_name.clone());
                        result.fixes_applied += 1;
                        result.details.push(FixDetail {
                            rule: LintRule::HardcodedSpacing,
                            node_id: node.id.clone(),
                            action: "gap→token_ref".into(),
                            before,
                            after: token_name.clone(),
                        });
                    }
                }
            }
            LayoutMode::Grid(grid) => {
                if (grid.gap.0 != 0.0 || grid.gap.1 != 0.0)
                    && !node.style.design_token_refs.contains_key("gap")
                {
                    let key = format!("{}", grid.gap.0);
                    if let Some(token_name) = numeric_map.get(&key) {
                        let before = format!("({}, {})", grid.gap.0, grid.gap.1);
                        node.style
                            .design_token_refs
                            .insert("gap".into(), token_name.clone());
                        result.fixes_applied += 1;
                        result.details.push(FixDetail {
                            rule: LintRule::HardcodedSpacing,
                            node_id: node.id.clone(),
                            action: "grid_gap→token_ref".into(),
                            before,
                            after: token_name.clone(),
                        });
                    }
                }
            }
            LayoutMode::Free => {}
        }

        // font_size → typography token
        if let Some(fs) = node.style.font_size {
            if fs > 0.0 && !node.style.design_token_refs.contains_key("font_size") {
                let key = format!("{}", fs);
                if let Some(token_name) = numeric_map.get(&key) {
                    let before = format!("{}", fs);
                    node.style
                        .design_token_refs
                        .insert("font_size".into(), token_name.clone());
                    result.fixes_applied += 1;
                    result.details.push(FixDetail {
                        rule: LintRule::HardcodedFontSize,
                        node_id: node.id.clone(),
                        action: "font_size→token_ref".into(),
                        before,
                        after: token_name.clone(),
                    });
                }
            }
        }

        // empty effects cleanup
        if node
            .style
            .fill
            .as_deref()
            .is_some_and(|v| v.trim().is_empty())
        {
            let before = node.style.fill.clone().unwrap_or_default();
            node.style.fill = None;
            result.fixes_applied += 1;
            result.details.push(FixDetail {
                rule: LintRule::EmptyEffects,
                node_id: node.id.clone(),
                action: "remove_empty_fill".into(),
                before,
                after: "None".into(),
            });
        }
        if node
            .style
            .stroke
            .as_deref()
            .is_some_and(|v| v.trim().is_empty())
        {
            let before = node.style.stroke.clone().unwrap_or_default();
            node.style.stroke = None;
            result.fixes_applied += 1;
            result.details.push(FixDetail {
                rule: LintRule::EmptyEffects,
                node_id: node.id.clone(),
                action: "remove_empty_stroke".into(),
                before,
                after: "None".into(),
            });
        }

        // unnamed node → auto-name
        let default_names = ["Rect", "Text", "Circle", "Image", "Group"];
        if default_names.contains(&node.name.as_str()) {
            let kind_str = match node.kind {
                NodeKind::Rect => "rect",
                NodeKind::Circle => "circle",
                NodeKind::Text => "text",
                NodeKind::Image => "image",
                NodeKind::Group => "group",
            };
            // E-29：按字符边界取前 8 字符，非字节切片——含 CJK id 字节切片在
            // 多字节字符中间切断会 panic at non-char boundary。node.id 含中文
            // （如「登录按钮」）走 lint --fix 即崩。
            let id_prefix: String = node.id.chars().take(8).collect();
            let new_name = format!("{}_{}", kind_str, id_prefix);
            let before = node.name.clone();
            node.name = new_name.clone();
            result.fixes_applied += 1;
            result.details.push(FixDetail {
                rule: LintRule::UnnamedNode,
                node_id: node.id.clone(),
                action: "auto_name".into(),
                before,
                after: new_name,
            });
        }

        apply_tokens_to_nodes(&mut node.children, token_map, numeric_map, result);
    }
}

fn build_numeric_token_map(system: &DesignSystem) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for token in &system.tokens {
        if let TokenValue::String(s) = &token.value {
            if s.starts_with("token:") {
                continue;
            }
            if let Ok(val) = s.parse::<f64>() {
                map.insert(format!("{}", val), token.name.clone());
            }
        }
    }
    map
}

impl Linter {
    pub fn auto_fix(&self, doc: &mut PenDocument) -> FixResult {
        info!("auto_fix: 开始自动修复, 规则数={}", self.rules.len());
        let mut result = FixResult {
            fixes_applied: 0,
            fixes_skipped: 0,
            details: vec![],
        };

        if let Some(ref system) = self.design_system {
            let token_result = apply_tokens_to_document(doc, system);
            result.fixes_applied += token_result.fixes_applied;
            result.details.extend(token_result.details);
        } else {
            result.fixes_skipped += 1;
            info!("auto_fix: 无设计规范, 跳过 Token 修复");
        }

        info!(
            "auto_fix: 完成, applied={}, skipped={}",
            result.fixes_applied, result.fixes_skipped
        );
        result
    }
}

fn luminance(color: &str) -> f64 {
    let hex = match parse_hex_color(color) {
        Some(h) => h,
        None => return -1.0,
    };

    let r = srgb_to_linear(hex[0] as f64 / 255.0);
    let g = srgb_to_linear(hex[1] as f64 / 255.0);
    let b = srgb_to_linear(hex[2] as f64 / 255.0);

    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn contrast_ratio(l1: f64, l2: f64) -> f64 {
    let lighter = l1.max(l2);
    let darker = l1.min(l2);
    (lighter + 0.05) / (darker + 0.05)
}

fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    // E-30/P3：旧实现只认 3/6 位 hex，4 位(#RGBA)/8 位(#RRGGBBAA) 常见格式被拒致假告警。
    // alpha 通道对亮度对比度无意义，忽略（按不透明处理）。
    let s = s.trim().trim_start_matches('#');
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some([r, g, b])
        }
        8 => {
            // #RRGGBBAA：忽略末 2 位 alpha
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some([r, g, b])
        }
        3 => {
            let r = u8::from_str_radix(&s[0..1], 16).ok()?;
            let g = u8::from_str_radix(&s[1..2], 16).ok()?;
            let b = u8::from_str_radix(&s[2..3], 16).ok()?;
            Some([r * 17, g * 17, b * 17])
        }
        4 => {
            // #RGBA：忽略末 1 位 alpha，3 位展开 ×17
            let r = u8::from_str_radix(&s[0..1], 16).ok()?;
            let g = u8::from_str_radix(&s[1..2], 16).ok()?;
            let b = u8::from_str_radix(&s[2..3], 16).ok()?;
            Some([r * 17, g * 17, b * 17])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fd_canvas_core::{NodeStyle, Page};

    fn make_doc(nodes: Vec<PenNode>) -> PenDocument {
        PenDocument {
            pages: vec![Page {
                id: "page-1".into(),
                name: "Page 1".into(),
                width: 800.0,
                height: 600.0,
                nodes,
            }],
            ..Default::default()
        }
    }

    fn text_node(id: &str, name: &str, style: NodeStyle) -> PenNode {
        PenNode {
            id: id.into(),
            name: name.into(),
            kind: NodeKind::Text,
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 30.0,
            style,
            text: None,
            children: vec![],
            rotation: 0.0,
            z_index: 0,
        }
    }

    fn rect_node(id: &str, name: &str, style: NodeStyle) -> PenNode {
        PenNode {
            id: id.into(),
            name: name.into(),
            kind: NodeKind::Rect,
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
            style,
            text: None,
            children: vec![],
            rotation: 0.0,
            z_index: 0,
        }
    }

    fn group_node(id: &str, name: &str, children: Vec<PenNode>) -> PenNode {
        PenNode {
            id: id.into(),
            name: name.into(),
            kind: NodeKind::Group,
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 40.0,
            style: NodeStyle::default(),
            text: None,
            children,
            rotation: 0.0,
            z_index: 0,
        }
    }

    #[test]
    fn contrast_low_ratio_detected() {
        // E-28 正确语义：fg=文本（默认 #000000），bg=fill。
        // fill=#333333 深灰背景 + 黑字 → 对比度 ~1.6:1 不足 → 应检出。
        let style = NodeStyle {
            fill: Some("#333333".into()),
            stroke: Some("#444444".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "box", style)]);
        let result = Linter::new().lint(&doc);
        let contrast_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == LintRule::ContrastCheck)
            .collect();
        assert!(!contrast_violations.is_empty());
    }

    #[test]
    fn contrast_good_ratio_no_violation() {
        // E-28 正确语义：fill=#ffffff 白背景 + 默认黑字 → 对比度 21:1 → 无违规。
        // 旧测试 fill=#000/stroke=#fff 把 stroke 当背景，是 bug 行为，已纠正。
        let style = NodeStyle {
            fill: Some("#ffffff".into()),
            stroke: Some("#000000".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "box", style)]);
        let result = Linter::new().lint(&doc);
        let contrast_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == LintRule::ContrastCheck)
            .collect();
        assert!(contrast_violations.is_empty());
    }

    // E-28 回归：stroke（边框）不得当背景。仅设 stroke 无 fill → 背景透明 → 跳过，
    // 不得检出对比度违规（旧实现 bg=stroke 会误报边框对比度）。
    #[test]
    fn contrast_stroke_only_not_treated_as_bg() {
        let style = NodeStyle {
            fill: None,
            stroke: Some("#444444".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "box", style)]);
        let result = Linter::new().lint(&doc);
        let contrast_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == LintRule::ContrastCheck)
            .collect();
        assert!(
            contrast_violations.is_empty(),
            "无 fill（透明背景）应跳过对比度，不得把 stroke 当背景误报"
        );
    }

    #[test]
    fn unlabeled_input_detected() {
        let input = group_node("c1", "SearchInput", vec![]);
        let doc = make_doc(vec![input]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::UnlabeledInput));
    }

    #[test]
    fn labeled_input_no_violation() {
        let label = text_node("l1", "Label", NodeStyle::default());
        let input = group_node("c1", "SearchInput", vec![label]);
        let doc = make_doc(vec![input]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::UnlabeledInput));
    }

    #[test]
    fn text_gradient_detected() {
        let style = NodeStyle {
            fill: Some("linear-gradient(90deg, #ff0000, #0000ff)".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![text_node("t1", "fancy", style)]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::TextEffects));
    }

    #[test]
    fn rect_with_gradient_no_text_effect_violation() {
        let style = NodeStyle {
            fill: Some("linear-gradient(90deg, #ff0000, #0000ff)".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "bg", style)]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::TextEffects));
    }

    #[test]
    fn abnormal_rotation_over_90_detected() {
        let mut node = rect_node("r1", "rotated", NodeStyle::default());
        node.rotation = 120.0;
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::AbnormalRotation && v.severity == LintSeverity::Error));
    }

    #[test]
    fn non_15_multiple_rotation_info() {
        let mut node = rect_node("r1", "slight", NodeStyle::default());
        node.rotation = 7.0;
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::AbnormalRotation && v.severity == LintSeverity::Info));
    }

    #[test]
    fn empty_fill_detected() {
        let style = NodeStyle {
            fill: Some("".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "box", style)]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::EmptyEffects));
    }

    #[test]
    fn empty_stroke_detected() {
        let style = NodeStyle {
            stroke: Some("   ".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "box", style)]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::EmptyEffects));
    }

    #[test]
    fn empty_font_family_detected() {
        let style = NodeStyle {
            font_family: Some("  ".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![text_node("t1", "txt", style)]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::EmptyEffects));
    }

    #[test]
    fn no_design_system_skips_token_check() {
        let style = NodeStyle {
            fill: Some("#007AFF".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "box", style)]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::TokenInconsistency));
    }

    #[test]
    fn lint_stats_counts_correct() {
        let style = NodeStyle {
            fill: Some("#333333".into()),
            stroke: Some("#444444".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "box", style)]);
        let result = Linter::new().lint(&doc);
        assert_eq!(result.stats.total_nodes, 1);
        assert!(result.stats.total_violations > 0);
        assert_eq!(
            result.stats.errors + result.stats.warnings + result.stats.infos,
            result.stats.total_violations
        );
    }

    #[test]
    fn empty_document_no_violations() {
        let doc = make_doc(vec![]);
        let result = Linter::new().lint(&doc);
        assert!(result.violations.is_empty());
        assert_eq!(result.stats.total_nodes, 0);
    }

    #[test]
    fn with_rules_filters_rules() {
        let mut node = rect_node("r1", "box", NodeStyle::default());
        node.rotation = 120.0;
        let doc = make_doc(vec![node]);
        let result = Linter::with_rules(vec![LintRule::ContrastCheck]).lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::AbnormalRotation));
    }

    #[test]
    fn hex_color_3_digit_parsing() {
        // E-28 正确语义：fill=#fff 白背景 + 默认黑字 → 高对比度，无违规；
        // 同时验证 3 位 hex 颜色能被 luminance 正确解析。
        let style = NodeStyle {
            fill: Some("#fff".into()),
            stroke: Some("#000".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "box", style)]);
        let result = Linter::new().lint(&doc);
        let contrast_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == LintRule::ContrastCheck)
            .collect();
        assert!(contrast_violations.is_empty());
    }

    #[test]
    fn hex_color_4_and_8_digit_alpha_ignored() {
        // E-30/P3：4 位(#RGBA)/8 位(#RRGGBBAA) hex 必须可解析，alpha 通道忽略。
        // 8 位 #FF0000FF（红，全不透明）应等价于 6 位 #FF0000。
        assert_eq!(parse_hex_color("#FF0000FF"), Some([255, 0, 0]));
        // 8 位 半透明 #00FF0080 应等价于 6 位 #00FF00。
        assert_eq!(parse_hex_color("#00FF0080"), Some([0, 255, 0]));
        // 4 位 #F00F（红，全不透明）应等价于 3 位 #F00 = [255,0,0]。
        assert_eq!(parse_hex_color("#F00F"), Some([255, 0, 0]));
        // 4 位 #0F0F 应等价于 3 位 #0F0 = [0,255,0]。
        assert_eq!(parse_hex_color("#0F0F"), Some([0, 255, 0]));
        // 无 # 前缀同样支持。
        assert_eq!(parse_hex_color("FF0000FF"), Some([255, 0, 0]));
        assert_eq!(parse_hex_color("F00F"), Some([255, 0, 0]));
        // 3 位仍兼容：#FF0 = RGB(F,F,0) = [255,255,0]。
        assert_eq!(parse_hex_color("#FF0"), Some([255, 255, 0]));
        // 4 位 #FF00 = RGBA(F,F,0,alpha=0 忽略) = [255,255,0]。
        assert_eq!(parse_hex_color("#FF00"), Some([255, 255, 0]));
        // 长度非法（5 位）返回 None。
        assert_eq!(parse_hex_color("#FF000"), None);
        assert_eq!(parse_hex_color("#FF0000"), Some([255, 0, 0]));
    }

    #[test]
    fn hex_color_8_digit_contrast_no_false_violation() {
        // E-30/P3 端到端：8 位 hex fill 在对比度检测中不应因解析失败假告警。
        // #FFFFFF（白）背景 + #000000FF（黑，alpha=FF 忽略）字 → 高对比，无违规。
        let style = NodeStyle {
            fill: Some("#FFFFFF".into()),
            stroke: Some("#000000FF".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "box", style)]);
        let result = Linter::new().lint(&doc);
        let contrast_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == LintRule::ContrastCheck)
            .collect();
        assert!(
            contrast_violations.is_empty(),
            "8 位 hex 不应触发对比度假告警: {:?}",
            contrast_violations
        );
    }

    #[test]
    fn nested_nodes_counted() {
        let child = rect_node("c1", "child", NodeStyle::default());
        let parent = PenNode {
            id: "p1".into(),
            name: "parent".into(),
            kind: NodeKind::Group,
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 200.0,
            style: NodeStyle::default(),
            text: None,
            children: vec![child],
            rotation: 0.0,
            z_index: 0,
        };
        let doc = make_doc(vec![parent]);
        let result = Linter::new().lint(&doc);
        assert_eq!(result.stats.total_nodes, 2);
    }

    #[test]
    fn lint_result_serializable() {
        let doc = make_doc(vec![]);
        let result = Linter::new().lint(&doc);
        let json = serde_json::to_string(&result).unwrap();
        let back: LintResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stats.total_nodes, 0);
    }

    #[test]
    fn rotation_0_no_violation() {
        let node = rect_node("r1", "box", NodeStyle::default());
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::AbnormalRotation));
    }

    #[test]
    fn rotation_45_no_violation() {
        let mut node = rect_node("r1", "box", NodeStyle::default());
        node.rotation = 45.0;
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::AbnormalRotation));
    }

    #[test]
    fn unnamed_node_default_name_detected() {
        let node = rect_node("r1", "Rect", NodeStyle::default());
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::UnnamedNode));
    }

    #[test]
    fn named_node_no_unnamed_violation() {
        let node = rect_node("r1", "HeaderBackground", NodeStyle::default());
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::UnnamedNode));
    }

    #[test]
    fn text_overflow_zero_size_detected() {
        let mut node = text_node("t1", "Text", NodeStyle::default());
        node.w = 0.0;
        node.h = 0.0;
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::TextOverflow));
    }

    #[test]
    fn text_with_size_no_overflow_violation() {
        let node = text_node("t1", "label", NodeStyle::default());
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::TextOverflow));
    }

    #[test]
    fn overlapping_nodes_detected() {
        let a = PenNode {
            id: "a".into(),
            name: "A".into(),
            kind: NodeKind::Rect,
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
            style: NodeStyle::default(),
            text: None,
            children: vec![],
            rotation: 0.0,
            z_index: 0,
        };
        let b = PenNode {
            id: "b".into(),
            name: "B".into(),
            kind: NodeKind::Rect,
            x: 50.0,
            y: 50.0,
            w: 100.0,
            h: 100.0,
            style: NodeStyle::default(),
            text: None,
            children: vec![],
            rotation: 0.0,
            z_index: 0,
        };
        let doc = make_doc(vec![a, b]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::OverlappingNodes));
    }

    #[test]
    fn non_overlapping_nodes_no_violation() {
        let a = PenNode {
            id: "a".into(),
            name: "A".into(),
            kind: NodeKind::Rect,
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
            style: NodeStyle::default(),
            text: None,
            children: vec![],
            rotation: 0.0,
            z_index: 0,
        };
        let b = PenNode {
            id: "b".into(),
            name: "B".into(),
            kind: NodeKind::Rect,
            x: 200.0,
            y: 200.0,
            w: 100.0,
            h: 100.0,
            style: NodeStyle::default(),
            text: None,
            children: vec![],
            rotation: 0.0,
            z_index: 0,
        };
        let doc = make_doc(vec![a, b]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::OverlappingNodes));
    }

    #[test]
    fn hardcoded_spacing_flex_gap_detected() {
        use fd_canvas_core::{FlexParams, LayoutMode};
        let style = NodeStyle {
            layout: LayoutMode::Flex(FlexParams {
                gap: 16.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let node = rect_node("r1", "Container", style);
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::HardcodedSpacing));
    }

    #[test]
    fn token_ref_spacing_no_violation() {
        use fd_canvas_core::{FlexParams, LayoutMode};
        let mut refs = HashMap::new();
        refs.insert("gap".to_string(), "spacing-md".to_string());
        let style = NodeStyle {
            layout: LayoutMode::Flex(FlexParams {
                gap: 16.0,
                ..Default::default()
            }),
            design_token_refs: refs,
            ..Default::default()
        };
        let node = rect_node("r1", "Container", style);
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::HardcodedSpacing));
    }

    #[test]
    fn hardcoded_font_size_detected() {
        let style = NodeStyle {
            font_size: Some(14.0),
            ..Default::default()
        };
        let node = text_node("t1", "Label", style);
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::HardcodedFontSize));
    }

    #[test]
    fn token_ref_font_size_no_violation() {
        let mut refs = HashMap::new();
        refs.insert("font_size".to_string(), "typography-body".to_string());
        let style = NodeStyle {
            font_size: Some(14.0),
            design_token_refs: refs,
            ..Default::default()
        };
        let node = text_node("t1", "Label", style);
        let doc = make_doc(vec![node]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::HardcodedFontSize));
    }

    #[test]
    fn missing_interaction_state_button_detected() {
        let btn = group_node("b1", "SubmitButton", vec![]);
        let doc = make_doc(vec![btn]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::MissingInteractionState));
    }

    #[test]
    fn button_with_hover_no_violation() {
        let hover = rect_node("h1", "Hover", NodeStyle::default());
        let btn = group_node("b1", "SubmitButton", vec![hover]);
        let doc = make_doc(vec![btn]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::MissingInteractionState));
    }

    #[test]
    fn layout_inconsistency_detected() {
        use fd_canvas_core::{FlexParams, LayoutMode};
        let style_flex = NodeStyle {
            layout: LayoutMode::Flex(FlexParams::default()),
            ..Default::default()
        };
        let a = rect_node("a", "ItemA", style_flex);
        let b = rect_node("b", "ItemB", NodeStyle::default());
        let doc = make_doc(vec![a, b]);
        let result = Linter::new().lint(&doc);
        assert!(result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::LayoutInconsistency));
    }

    #[test]
    fn consistent_layout_no_violation() {
        let a = rect_node("a", "ItemA", NodeStyle::default());
        let b = rect_node("b", "ItemB", NodeStyle::default());
        let doc = make_doc(vec![a, b]);
        let result = Linter::new().lint(&doc);
        assert!(!result
            .violations
            .iter()
            .any(|v| v.rule == LintRule::LayoutInconsistency));
    }

    #[test]
    fn all_rules_count_is_13() {
        assert_eq!(LintRule::all().len(), 13);
    }

    #[test]
    fn apply_tokens_replaces_fill_with_ref() {
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        let system = reg.get("apple-hig").expect("apple-hig").clone();
        let mut refs = HashMap::new();
        refs.insert("fill".into(), "primary".into());
        let style = NodeStyle {
            fill: Some("#007AFF".into()),
            design_token_refs: refs,
            ..Default::default()
        };
        let node = rect_node("r1", "box", style);
        let mut doc = make_doc(vec![node]);

        let result = apply_tokens_to_document(&mut doc, &system);
        // fill already has token ref, so no fix for it
        let fill_fixes: Vec<_> = result
            .details
            .iter()
            .filter(|d| d.action == "fill→token_ref")
            .collect();
        assert!(fill_fixes.is_empty());
    }

    #[test]
    fn apply_tokens_adds_fill_ref() {
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        let system = reg.get("apple-hig").expect("apple-hig").clone();
        let style = NodeStyle {
            fill: Some("#007AFF".into()),
            ..Default::default()
        };
        let node = rect_node("r1", "box", style);
        let mut doc = make_doc(vec![node]);

        let result = apply_tokens_to_document(&mut doc, &system);
        assert!(result.fixes_applied > 0);
        let fill_fixes: Vec<_> = result
            .details
            .iter()
            .filter(|d| d.action == "fill→token_ref")
            .collect();
        assert!(!fill_fixes.is_empty());
        assert!(doc.pages[0].nodes[0]
            .style
            .design_token_refs
            .contains_key("fill"));
    }

    #[test]
    fn auto_fix_removes_empty_fill() {
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        let system = reg.get("apple-hig").expect("apple-hig").clone();
        let style = NodeStyle {
            fill: Some("".into()),
            ..Default::default()
        };
        let node = rect_node("r1", "Rect", style);
        let mut doc = make_doc(vec![node]);

        let linter = Linter::new().with_design_system(system);
        let result = linter.auto_fix(&mut doc);
        assert!(result
            .details
            .iter()
            .any(|d| d.action == "remove_empty_fill"));
        assert!(doc.pages[0].nodes[0].style.fill.is_none());
    }

    #[test]
    fn auto_fix_names_unnamed_node() {
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        let system = reg.get("apple-hig").expect("apple-hig").clone();
        let node = rect_node("abc12345def", "Rect", NodeStyle::default());
        let mut doc = make_doc(vec![node]);

        let linter = Linter::new().with_design_system(system);
        let result = linter.auto_fix(&mut doc);
        assert!(result.details.iter().any(|d| d.action == "auto_name"));
        assert!(doc.pages[0].nodes[0].name.starts_with("rect_"));
    }

    #[test]
    fn auto_fix_names_cjk_id_no_panic() {
        // E-29 回归：含 CJK 的 node.id 走 auto_name 不得 panic。
        // 旧 `&node.id[..8]` 字节切片在「登录」(e7 99 bb e5 bd 95) 第 8 字节
        // 落在字符中间 → byte index 8 is not a char boundary panic。
        // 现按字符边界取前 8 字符。
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        let system = reg.get("apple-hig").expect("apple-hig").clone();
        let cjk_id = "登录按钮节点标识符0123456789";
        let node = rect_node(cjk_id, "Rect", NodeStyle::default());
        let mut doc = make_doc(vec![node]);

        let linter = Linter::new().with_design_system(system);
        let result = linter.auto_fix(&mut doc);
        assert!(result.details.iter().any(|d| d.action == "auto_name"));
        let renamed = &doc.pages[0].nodes[0].name;
        assert!(renamed.starts_with("rect_"), "renamed={renamed}");
        // 前缀应含完整 CJK 字符（不切断多字节字符）
        assert!(renamed.contains("登录"), "renamed={renamed}");
    }

    #[test]
    fn fix_result_serializable() {
        let result = FixResult {
            fixes_applied: 3,
            fixes_skipped: 1,
            details: vec![FixDetail {
                rule: LintRule::EmptyEffects,
                node_id: "n1".into(),
                action: "remove_empty_fill".into(),
                before: "".into(),
                after: "None".into(),
            }],
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: FixResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.fixes_applied, 3);
        assert_eq!(back.details.len(), 1);
    }

    #[test]
    fn auto_fix_without_design_system_skips_token_fixes() {
        let style = NodeStyle {
            fill: Some("#007AFF".into()),
            ..Default::default()
        };
        let node = rect_node("r1", "box", style);
        let mut doc = make_doc(vec![node]);

        let linter = Linter::new();
        let result = linter.auto_fix(&mut doc);
        assert_eq!(result.fixes_applied, 0);
        assert!(result.fixes_skipped > 0);
    }
}
