// Callers: fd-cli (lint subcommand → Linter::lint()), DesignBridge.swift (Process → fusion-design lint)
// Affected API: Linter::new(), Linter::with_rules(), Linter::with_design_system(), Linter::lint(), LintResult, LintViolation, LintRule, LintSeverity
// Data schemas: PenDocument→Vec<LintViolation> (13 rules), DesignSystem Token→HashMap for cross-ref, LintStats summary
// User instruction: "start implementing now" — Task #17 P3-6 design_lint Skill (basic detectors)
//! Fusion-Design design lint — 13 detectors for design specification compliance.
//!
//! ARCH-10 r4: god-file split. lib.rs keeps only public types + Lint orchestration (lint/lint_children/
//! lint_siblings/lint_node dispatch). The 13 detectors moved to `detectors.rs`, color/contrast pure
//! functions to `color.rs`, auto-fix + Token mapping to `fix.rs`. build_token_map/build_numeric_token_map
//! are shared leaf helpers (consumed by detectors + fix + tests), kept in lib.rs as the single source of truth.

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
    // Issue #41: desigma acceptance-gate rules (Error severity, desigma profile).
    BareColorLiteral,
    OffTokenSpacing,
    OffTokenRadius,
    MissingDarkPair,
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
            LintRule::BareColorLiteral => "bare_color_literal",
            LintRule::OffTokenSpacing => "off_token_spacing",
            LintRule::OffTokenRadius => "off_token_radius",
            LintRule::MissingDarkPair => "missing_dark_pair",
        }
    }

    pub fn description(&self) -> &str {
        match self {
            LintRule::ContrastCheck => "Foreground/background contrast is insufficient, hurting readability",
            LintRule::UnlabeledInput => "Input control is missing a label (label/placeholder)",
            LintRule::TextEffects => "Text node uses effects (gradient etc.), hurting readability",
            LintRule::AbnormalRotation => "Node rotation angle is abnormal (over 90 deg or not a multiple of 15 deg)",
            LintRule::EmptyEffects => "Node declares a style with empty parameters, which is redundant",
            LintRule::TokenInconsistency => "Style value does not use a design system Token, risking inconsistency",
            LintRule::UnnamedNode => "Node uses a default name, lacking a semantic identifier",
            LintRule::TextOverflow => "Text node has zero size, content will overflow",
            LintRule::OverlappingNodes => "Sibling node bounding boxes overlap, which may cause occlusion",
            LintRule::HardcodedSpacing => "Spacing value does not reference a design Token, hurting maintainability",
            LintRule::HardcodedFontSize => "Font size does not reference a design Token, hurting maintainability",
            LintRule::MissingInteractionState => "Interactive control is missing hover/active state definitions",
            LintRule::LayoutInconsistency => "Sibling nodes use inconsistent layout modes",
            LintRule::BareColorLiteral => {
                "Color is a bare hex/rgb literal matching no design Token (desigma acceptance gate)"
            }
            LintRule::OffTokenSpacing => {
                "Spacing value is off the design Token scale and references no Token (desigma acceptance gate)"
            }
            LintRule::OffTokenRadius => {
                "Radius value is off the design Token scale and references no Token (desigma acceptance gate)"
            }
            LintRule::MissingDarkPair => "Token matched by color lacks a dark-mode pair (desigma acceptance gate)",
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
            LintRule::BareColorLiteral,
            LintRule::OffTokenSpacing,
            LintRule::OffTokenRadius,
            LintRule::MissingDarkPair,
        ]
    }
}

// Issue #41: lint profiles. Default = all 13 legacy rules (unchanged behavior).
// Desigma = acceptance gate for desigma-sourced customer designs: token
// alignment only, every rule Error severity so `--gate` exit code is binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintProfile {
    Default,
    Desigma,
}

impl LintProfile {
    pub fn rules(&self) -> Vec<LintRule> {
        match self {
            LintProfile::Default => LintRule::all(),
            LintProfile::Desigma => vec![
                LintRule::BareColorLiteral,
                LintRule::OffTokenSpacing,
                LintRule::OffTokenRadius,
                LintRule::MissingDarkPair,
            ],
        }
    }

    pub fn name(&self) -> &str {
        match self {
            LintProfile::Default => "default",
            LintProfile::Desigma => "desigma",
        }
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

    // Issue #41: profile-based construction (desigma gate). Design system is
    // still optional; the dark-pair rule no-ops without one.
    pub fn with_profile(profile: LintProfile) -> Self {
        Self {
            rules: profile.rules(),
            design_system: None,
        }
    }

    pub fn with_design_system(mut self, system: DesignSystem) -> Self {
        self.design_system = Some(system);
        self
    }

    pub fn lint(&self, doc: &PenDocument) -> LintResult {
        info!(
            "lint: starting PenDocument check, rules={}",
            self.rules.len()
        );
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
            "lint: check finished, total_nodes={}, violations={}, error={}, warning={}, info={}",
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
        // L-10: lint_siblings moved out of the for-child loop — each sibling set is checked once;
        // the old impl called it once per child = N x repeated scans + N x repeated violations.
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
                LintRule::BareColorLiteral => self.check_bare_color_literal(node, violations),
                LintRule::OffTokenSpacing => self.check_off_token_spacing(node, violations),
                LintRule::OffTokenRadius => self.check_off_token_radius(node, violations),
                LintRule::MissingDarkPair => self.check_missing_dark_pair(node, violations),
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
            // E-13: Number tokens (builtin specs use Number for spacing/font_size/radius mostly);
            // the old impl only handled parseable String digits and missed the Number variant —
            // gap=16 could not match a `spacing.md` token reference, breaking auto-fix. Added the Number arm to the map.
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
        info!("auto_fix: starting auto-fix, rules={}", self.rules.len());
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
            info!("auto_fix: no design system, skipping Token fixes");
        }

        info!(
            "auto_fix: finished, applied={}, skipped={}",
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
        // L-9 correct semantics: fg = Text node fill (text color), bg = parent node fill (background).
        // Parent rect fill=#333333 dark-gray background + child Text fill=#000000 black text -> contrast ~1.6:1 -> must be detected.
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
            "black text on a dark-gray background with insufficient contrast must be detected"
        );
    }

    #[test]
    fn contrast_good_ratio_no_violation() {
        // E-28 correct semantics: fill=#ffffff white background + default black text -> contrast 21:1 -> no violation.
        // The old test fill=#000/stroke=#fff treated stroke as background — that was buggy behavior, now corrected.
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

    // E-14 regression: the old impl returned -1.0 luminance and skipped for rgb()/rgba()/named colors -> false-negative misses.
    // After the fix rgb/rgba/named colors parse, so dark background + light text must be detected as insufficient contrast.
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
            "black text on an rgb() dark-gray background must be detected (after the E-14 fix, no longer returns -1.0 and skips)"
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
        // white on black = 21:1, no violation (verifies named colors pass correctly after parsing)
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
            "white on black named colors with 21:1 contrast must have no violation"
        );
    }

    #[test]
    fn contrast_rgba_parsing_no_skip() {
        // rgba must parse successfully (not skipped as -1.0). White text rgba(255,255,255,1.0) on black = 21:1, no violation.
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
            "rgba() white on black at 21:1 must have no violation (E-14 parses rgba instead of skipping)"
        );
    }

    // E-28 regression: stroke (border) must not be treated as background. stroke-only with no fill -> transparent background -> skip;
    // must not detect a contrast violation (the old impl used bg=stroke and mis-reported border contrast).
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
            "no fill (transparent background) must skip contrast and must not mis-report stroke as background"
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
        // L-9/L-11 regression: trigger TextOverflow with a Text node carrying overflowing text, ensuring stats are non-empty.
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
        // E-28 correct semantics: fill=#fff white background + default black text -> high contrast, no violation;
        // also verifies 3-digit hex colors parse correctly in luminance.
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
        // E-30/P3: 4-digit (#RGBA)/8-digit (#RRGGBBAA) hex must parse, alpha channel ignored.
        // 8-digit #FF0000FF (red, fully opaque) must be equivalent to 6-digit #FF0000.
        assert_eq!(parse_hex_color("#FF0000FF"), Some([255, 0, 0]));
        // 8-digit semi-transparent #00FF0080 must be equivalent to 6-digit #00FF00.
        assert_eq!(parse_hex_color("#00FF0080"), Some([0, 255, 0]));
        // 4-digit #F00F (red, fully opaque) must be equivalent to 3-digit #F00 = [255,0,0].
        assert_eq!(parse_hex_color("#F00F"), Some([255, 0, 0]));
        // 4-digit #0F0F must be equivalent to 3-digit #0F0 = [0,255,0].
        assert_eq!(parse_hex_color("#0F0F"), Some([0, 255, 0]));
        // Without a # prefix is supported too.
        assert_eq!(parse_hex_color("FF0000FF"), Some([255, 0, 0]));
        assert_eq!(parse_hex_color("F00F"), Some([255, 0, 0]));
        // 3-digit stays compatible: #FF0 = RGB(F,F,0) = [255,255,0].
        assert_eq!(parse_hex_color("#FF0"), Some([255, 255, 0]));
        // 4-digit #FF00 = RGBA(F,F,0,alpha=0 ignored) = [255,255,0].
        assert_eq!(parse_hex_color("#FF00"), Some([255, 255, 0]));
        // Invalid length (5 digits) returns None.
        assert_eq!(parse_hex_color("#FF000"), None);
        assert_eq!(parse_hex_color("#FF0000"), Some([255, 0, 0]));
    }

    #[test]
    fn hex_color_8_digit_contrast_no_false_violation() {
        // E-30/P3 end-to-end: an 8-digit hex fill must not trigger a false contrast alert due to parse failure.
        // #FFFFFF (white) background + #000000FF (black, alpha=FF ignored) text -> high contrast, no violation.
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
            "8-digit hex must not trigger a false contrast alert: {:?}",
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
        // L-11: zero dims + non-empty text -> overflow must be detected (the old impl also mis-reported empty text, now corrected).
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
        // PERF-3: a >200-node sibling group should warn + skip the O(n^2) overlap check -> 0 OverlappingNodes violations.
        // Note: check_overlapping_nodes is a private method, reached via Linter::new().lint(&doc) -> lint_siblings.
        // 201 nodes all overlapping (same bbox, same z_index) would produce ~20100 OverlappingNodes pairs without the threshold.
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
            ">200 nodes should skip the overlap check and produce no OverlappingNodes violations"
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

    // E-25 regression: the old impl only checked overlap when `a.z_index == b.z_index`, missing overlaps across different z_index values.
    // After the fix, different z_index values are still checked: a z_index difference > 1 downgrades to Info (intentional stacking), a difference <= 1 stays Warning.
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
            "overlapping nodes with different z_index must be detected (after the E-25 fix, no longer skipped due to unequal z_index)"
        );
        assert!(
            overlap_violations
                .iter()
                .all(|v| v.severity == LintSeverity::Info),
            "stacking with a z_index difference > 1 must downgrade to Info"
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
    fn all_rules_count_is_17() {
        assert_eq!(LintRule::all().len(), 17);
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
        // E-29 regression: auto_name on a node.id containing CJK must not panic.
        // The old `&node.id[..8]` byte slice cut "登录" (e7 99 bb e5 bd 95) at byte 8,
        // which lands mid-character -> "byte index 8 is not a char boundary" panic.
        // Now takes the first 8 characters on char boundaries.
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
        // The prefix must contain complete CJK characters (not cutting multi-byte characters)
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

    // E-13: build_numeric_token_map missed the TokenValue::Number variant — builtin specs
    // mostly use Number for spacing/font_size/radius; the old impl only put parseable String
    // digits into the map, so gap=8 could not match robot-sim `spacing.gauge` (Number(8.0)), breaking auto-fix.
    #[test]
    fn build_numeric_token_map_includes_number_variant() {
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        let system = reg.get("robot-sim").expect("robot-sim").clone();
        let map = build_numeric_token_map(&system);
        // robot-sim spacing.gauge = Number(8.0) -> key "8" -> value "spacing.gauge"
        assert_eq!(map.get("8"), Some(&"spacing.gauge".to_string()));
        // radius.panel = Number(4.0) -> key "4"
        assert_eq!(map.get("4"), Some(&"radius.panel".to_string()));
        // font.size.body = Number(13.0) -> key "13"
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
            "gap=8 must match the robot-sim spacing.gauge Number(8.0) token ref"
        );
        assert_eq!(
            doc.pages[0].nodes[0].style.design_token_refs.get("gap"),
            Some(&"spacing.gauge".to_string())
        );
    }

    // ── Issue #41: desigma acceptance gate ──

    fn desigma_system() -> fd_design_system::DesignSystem {
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        reg.get("desigma-aligned").expect("desigma-aligned").clone()
    }

    fn lint_with_profile(
        doc: &PenDocument,
        profile: LintProfile,
        system: Option<fd_design_system::DesignSystem>,
    ) -> LintResult {
        let mut linter = Linter::with_profile(profile);
        if let Some(s) = system {
            linter = linter.with_design_system(s);
        }
        linter.lint(doc)
    }

    #[test]
    fn desigma_profile_is_gate_rules_all_error() {
        let rules = LintProfile::Desigma.rules();
        assert_eq!(
            rules,
            vec![
                LintRule::BareColorLiteral,
                LintRule::OffTokenSpacing,
                LintRule::OffTokenRadius,
                LintRule::MissingDarkPair,
            ]
        );
    }

    #[test]
    fn desigma_aligned_sample_passes() {
        use fd_canvas_core::{FlexParams, LayoutMode};
        // Aligned sample: token-bound fill, on-scale gap + radius with refs.
        let style = NodeStyle {
            fill: Some("#FFFFFF".into()),
            radius: Some(8.0),
            layout: LayoutMode::Flex(FlexParams {
                gap: 16.0,
                ..Default::default()
            }),
            design_token_refs: [
                ("fill".to_string(), "color.bg".to_string()),
                ("gap".to_string(), "spacing.md".to_string()),
                ("radius".to_string(), "radius.md".to_string()),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "Card", style)]);
        let result = lint_with_profile(&doc, LintProfile::Desigma, Some(desigma_system()));
        assert_eq!(
            result.stats.errors, 0,
            "aligned sample must pass: {:?}",
            result.violations
        );
    }

    #[test]
    fn desigma_bare_hex_fails_with_actionable_hint() {
        let style = NodeStyle {
            fill: Some("#123456".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "Banner", style)]);
        let result = lint_with_profile(&doc, LintProfile::Desigma, Some(desigma_system()));
        assert_eq!(result.stats.errors, 1);
        let v = &result.violations[0];
        assert_eq!(v.rule, LintRule::BareColorLiteral);
        assert_eq!(v.node_id, "r1");
        assert!(v.suggestion.contains("design_token_refs"));
        assert!(
            v.suggestion.contains("color.bg"),
            "hint lists available tokens: {}",
            v.suggestion
        );
    }

    #[test]
    fn desigma_missing_dark_pair_fails() {
        // robot-sim dark overlay lacks color.muted / color.warning.
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        let system = reg.get("robot-sim").expect("robot-sim").clone();
        let style = NodeStyle {
            fill: Some("#7D8590".into()),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "Label", style)]);
        let result = lint_with_profile(&doc, LintProfile::Desigma, Some(system));
        assert_eq!(result.stats.errors, 1);
        let v = &result.violations[0];
        assert_eq!(v.rule, LintRule::MissingDarkPair);
        assert!(v.message.contains("color.muted"));
        assert!(v.suggestion.contains("dark_tokens"));
    }

    #[test]
    fn desigma_off_token_spacing_and_radius_fail() {
        use fd_canvas_core::{FlexParams, LayoutMode};
        let style = NodeStyle {
            radius: Some(7.0),
            layout: LayoutMode::Flex(FlexParams {
                gap: 13.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "Row", style)]);
        let result = lint_with_profile(&doc, LintProfile::Desigma, Some(desigma_system()));
        let rules: Vec<LintRule> = result.violations.iter().map(|v| v.rule).collect();
        assert!(rules.contains(&LintRule::OffTokenSpacing), "{:?}", rules);
        assert!(rules.contains(&LintRule::OffTokenRadius), "{:?}", rules);
        assert_eq!(result.stats.errors, 2);
    }

    #[test]
    fn desigma_on_scale_values_pass_without_refs() {
        use fd_canvas_core::{FlexParams, LayoutMode};
        // Values on the token scale pass the gate even without token refs
        // (HardcodedSpacing-style refs are an auto-fix concern, not alignment).
        let style = NodeStyle {
            fill: Some("#FFFFFF".into()),
            radius: Some(8.0),
            layout: LayoutMode::Flex(FlexParams {
                gap: 16.0,
                ..Default::default()
            }),
            ..Default::default()
        };
        let doc = make_doc(vec![rect_node("r1", "Card", style)]);
        let result = lint_with_profile(&doc, LintProfile::Desigma, Some(desigma_system()));
        assert_eq!(result.stats.errors, 0, "{:?}", result.violations);
    }

    #[test]
    fn desigma_default_profile_unchanged() {
        // Default profile keeps the full legacy rule set (17 rules now).
        assert_eq!(LintProfile::Default.rules().len(), 17);
    }
}
