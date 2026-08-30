// Callers: fd-cli (lint subcommand → Linter::lint()), DesignBridge.swift (Process → fusion-design lint)
// Affected API: Linter::new(), Linter::with_rules(), Linter::with_design_system(), Linter::lint(), LintResult, LintViolation, LintRule, LintSeverity
// Data schemas: PenDocument→Vec<LintViolation> (13 rules), DesignSystem Token→HashMap for cross-ref, LintStats summary
// User instruction: "现在开始实施" — Task #17 P3-6 design_lint Skill（基础检测器）
//! Fusion-Design design lint — 13 detectors for design specification compliance.
//!
//! ARCH-10 r4：god-file 拆分。lib.rs 仅保留公共类型 + Lint 编排（lint/lint_children/
//! lint_siblings/lint_node 调度）。13 个检测器迁 `detectors.rs`，颜色/对比度纯函数迁
//! `color.rs`，auto-fix + Token 映射迁 `fix.rs`。build_token_map/build_numeric_token_map
//! 为共享叶子辅助（detectors + fix + 测试均消费），留 lib.rs 作单一真相源。

mod color;
mod detectors;
mod fix;

use std::collections::HashMap;

use fd_canvas_core::{PenDocument, PenNode};
use fd_design_system::{DesignSystem, TokenValue};
use serde::{Deserialize, Serialize};
use tracing::info;

pub use fix::apply_tokens_to_document;

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
                self.lint_node(node, &mut violations, None);
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
        // L-10：lint_siblings 移出 for child 循环——每兄弟集只查一次，
        // 旧实现每 child 调一次 = N× 重复扫描 + N× 重复违规。
        self.lint_siblings(&node.children, violations);
        let parent_fill = node.style.fill.as_deref();
        for child in &node.children {
            *total_nodes += 1;
            self.lint_node(child, violations, parent_fill);
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

    fn lint_node(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
        parent_fill: Option<&str>,
    ) {
        for rule in &self.rules {
            match rule {
                LintRule::ContrastCheck => self.check_contrast(node, violations, parent_fill),
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
}

impl Default for Linter {
    fn default() -> Self {
        Self::new()
    }
}

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

fn build_numeric_token_map(system: &DesignSystem) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for token in &system.tokens {
        match &token.value {
            // E-13：Number token（内置规范 spacing/font_size/radius 多用 Number），
            // 旧实现仅处理 String 可解析数字，漏掉 Number 变体——gap=16 无法匹配
            // `spacing.md` token 引用，auto-fix 失效。补 Number arm 入 map。
            TokenValue::Number(n) => {
                map.insert(format!("{}", n), token.name.clone());
            }
            TokenValue::String(s) if !s.starts_with("token:") => {
                if let Ok(val) = s.parse::<f64>() {
                    map.insert(format!("{}", val), token.name.clone());
                }
            }
            _ => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use fd_canvas_core::{parse_hex_color, NodeKind, NodeStyle, Page};

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
        // L-9 正确语义：fg=Text 节点 fill（文字色），bg=父节点 fill（背景）。
        // 父 rect fill=#333333 深灰背景 + 子 Text fill=#000000 黑字 → 对比度 ~1.6:1 → 应检出。
        let parent = rect_node(
            "r1",
            "box",
            NodeStyle {
                fill: Some("#333333".into()),
                ..Default::default()
            },
        );
        let mut parent = parent;
        parent.children = vec![text_node(
            "t1",
            "label",
            NodeStyle {
                fill: Some("#000000".into()),
                ..Default::default()
            },
        )];
        let doc = make_doc(vec![parent]);
        let result = Linter::new().lint(&doc);
        let contrast_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == LintRule::ContrastCheck)
            .collect();
        assert!(
            !contrast_violations.is_empty(),
            "深灰背景上的黑字对比度不足应检出"
        );
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

    // E-14 回归：rgb()/rgba()/命名色旧实现 luminance 返 -1.0 跳过 → 假阴性漏检。
    // 修复后 rgb/rgba/命名色可解析，深底浅字应检出对比度不足。
    #[test]
    fn contrast_rgb_color_detected() {
        let parent = rect_node(
            "r1",
            "box",
            NodeStyle {
                fill: Some("rgb(51,51,51)".into()),
                ..Default::default()
            },
        );
        let mut parent = parent;
        parent.children = vec![text_node(
            "t1",
            "label",
            NodeStyle {
                fill: Some("#000000".into()),
                ..Default::default()
            },
        )];
        let doc = make_doc(vec![parent]);
        let result = Linter::new().lint(&doc);
        let contrast_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == LintRule::ContrastCheck)
            .collect();
        assert!(
            !contrast_violations.is_empty(),
            "rgb() 深灰背景上的黑字应检出对比度不足（E-14 修复后不再返 -1.0 跳过）"
        );
    }

    #[test]
    fn contrast_named_color_white_on_black_detected() {
        let parent = rect_node(
            "r1",
            "box",
            NodeStyle {
                fill: Some("black".into()),
                ..Default::default()
            },
        );
        let mut parent = parent;
        // white on black = 21:1，无违规（验证命名色解析后正确通过）
        parent.children = vec![text_node(
            "t1",
            "label",
            NodeStyle {
                fill: Some("white".into()),
                ..Default::default()
            },
        )];
        let doc = make_doc(vec![parent]);
        let result = Linter::new().lint(&doc);
        let contrast_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == LintRule::ContrastCheck)
            .collect();
        assert!(
            contrast_violations.is_empty(),
            "white on black 命名色对比度 21:1 应无违规"
        );
    }

    #[test]
    fn contrast_rgba_parsing_no_skip() {
        // rgba 应解析成功（非 -1.0 跳过）。黑底 rgba(255,255,255,1.0) 白字 = 21:1，无违规。
        let parent = rect_node(
            "r1",
            "box",
            NodeStyle {
                fill: Some("#000000".into()),
                ..Default::default()
            },
        );
        let mut parent = parent;
        parent.children = vec![text_node(
            "t1",
            "label",
            NodeStyle {
                fill: Some("rgba(255,255,255,1.0)".into()),
                ..Default::default()
            },
        )];
        let doc = make_doc(vec![parent]);
        let result = Linter::new().lint(&doc);
        let contrast_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == LintRule::ContrastCheck)
            .collect();
        assert!(
            contrast_violations.is_empty(),
            "rgba() 白字黑底 21:1 应无违规（E-14 解析 rgba 不再跳过）"
        );
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
        // L-9/L-11 回归：用 Text 节点带溢出文本触发 TextOverflow，保证 stats 非空。
        let mut t = text_node(
            "t1",
            "label",
            NodeStyle {
                fill: Some("#000000".into()),
                ..Default::default()
            },
        );
        t.w = 10.0;
        t.text = Some("一段很长会溢出容器宽度的文本内容".into());
        let doc = make_doc(vec![t]);
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
        // L-11：零维 + 非空文本 → 溢出应检出（旧实现空文本也误报，已纠）。
        let mut node = text_node("t1", "Text", NodeStyle::default());
        node.w = 0.0;
        node.h = 0.0;
        node.text = Some("有内容但容器为零必然溢出".into());
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
    fn check_overlapping_skips_when_over_threshold() {
        // PERF-3：>200 节点兄弟组应 warn + 跳过 O(n²) 重叠检测 → 0 OverlappingNodes 违规。
        // 注：check_overlapping_nodes 为私有方法，经 Linter::new().lint(&doc) → lint_siblings 到达。
        // 201 节点全重叠（同 bbox 同 z_index），无阈值会产 ~20100 对 OverlappingNodes。
        let mut siblings = Vec::new();
        for i in 0..201 {
            siblings.push(PenNode {
                id: format!("n{i}"),
                name: format!("n{i}"),
                kind: NodeKind::Rect,
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                style: NodeStyle::default(),
                text: None,
                children: vec![],
                rotation: 0.0,
                z_index: 0,
            });
        }
        let doc = make_doc(siblings);
        let result = Linter::new().lint(&doc);
        assert!(
            !result
                .violations
                .iter()
                .any(|v| v.rule == LintRule::OverlappingNodes),
            ">200 节点应跳过重叠检测，不应产出 OverlappingNodes 违规"
        );
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

    // E-25 回归：旧实现 `a.z_index == b.z_index` 才检测重叠，不同 z_index 的重叠漏检。
    // 修复后不同 z_index 仍检测：z_index 差值 > 1 降级 Info（有意叠层），差值 ≤ 1 保持 Warning。
    #[test]
    fn overlapping_different_zindex_detected_as_info() {
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
            z_index: 5,
        };
        let doc = make_doc(vec![a, b]);
        let result = Linter::new().lint(&doc);
        let overlap_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| v.rule == LintRule::OverlappingNodes)
            .collect();
        assert!(
            !overlap_violations.is_empty(),
            "不同 z_index 的重叠节点应检出（E-25 修复后不再因 z_index 不等跳过）"
        );
        assert!(
            overlap_violations
                .iter()
                .all(|v| v.severity == LintSeverity::Info),
            "z_index 差值 > 1 的叠层应降级为 Info"
        );
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

    // E-13：build_numeric_token_map 遗漏 TokenValue::Number 变体——内置规范
    // spacing/font_size/radius 多用 Number，旧实现仅 String 可解析数字入 map，
    // 导致 gap=8 无法匹配 robot-sim `spacing.gauge`（Number(8.0)），auto-fix 失效。
    #[test]
    fn build_numeric_token_map_includes_number_variant() {
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        let system = reg.get("robot-sim").expect("robot-sim").clone();
        let map = build_numeric_token_map(&system);
        // robot-sim spacing.gauge = Number(8.0) → key "8" → value "spacing.gauge"
        assert_eq!(map.get("8"), Some(&"spacing.gauge".to_string()));
        // radius.panel = Number(4.0) → key "4"
        assert_eq!(map.get("4"), Some(&"radius.panel".to_string()));
        // font.size.body = Number(13.0) → key "13"
        assert_eq!(map.get("13"), Some(&"font.size.body".to_string()));
    }

    #[test]
    fn auto_fix_gap_matches_number_spacing_token() {
        use fd_canvas_core::{FlexParams, LayoutMode};
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        let system = reg.get("robot-sim").expect("robot-sim").clone();
        let style = NodeStyle {
            layout: LayoutMode::Flex(FlexParams {
                gap: 8.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let node = rect_node("r1", "Container", style);
        let mut doc = make_doc(vec![node]);

        let result = apply_tokens_to_document(&mut doc, &system);
        let gap_fixes: Vec<_> = result
            .details
            .iter()
            .filter(|d| d.action == "gap→token_ref")
            .collect();
        assert!(
            !gap_fixes.is_empty(),
            "gap=8 应匹配 robot-sim spacing.gauge Number(8.0) token ref"
        );
        assert_eq!(
            doc.pages[0].nodes[0].style.design_token_refs.get("gap"),
            Some(&"spacing.gauge".to_string())
        );
    }
}
