// ARCH-10 r4: the 13 detectors split out of lib.rs. All are Linter methods reading self.rules / self.design_system.
// Zero behavior change — pure positional move, visibility unchanged (pub(crate) for self-tests).

use fd_canvas_core::{NodeKind, PenNode};

use crate::{
    build_numeric_token_map, build_token_map, color::*, LintRule, LintSeverity, LintViolation,
};

impl super::Linter {
    pub(super) fn check_contrast(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
        parent_fill: Option<&str>,
    ) {
        // L-9: only Text nodes are checked for contrast (rects/circles carry no text; avoids false positives).
        // fg=style.fill (text color; codegen/host-web render text using fill as the color),
        // bg=parent_fill (parent background; when this node has no fill, text sits on the parent background).
        // No parent background -> skip (avoids false negatives, does not invent a black backdrop).
        if node.kind != NodeKind::Text {
            return;
        }
        let fg = match node.style.fill.as_deref() {
            Some(f) if !f.is_empty() => f,
            _ => return,
        };
        let bg = match parent_fill {
            Some(b) if !b.is_empty() => b,
            _ => return,
        };

        let fg_lum = luminance(fg);
        let bg_lum = luminance(bg);

        if fg_lum < 0.0 || bg_lum < 0.0 {
            return;
        }

        // E-14: alpha/opacity blending approximation. When node style.opacity < 1.0, blend the
        // foreground into the background by alpha before computing contrast (effective contrast of
        // rgba semi-transparent text on an opaque background).
        // Simplified as linear luminance blending: L_eff = alpha*fg_lum + (1-alpha)*bg_lum.
        let alpha = node.style.opacity.unwrap_or(1.0);
        let fg_eff = if (0.0..1.0).contains(&alpha) {
            alpha * fg_lum + (1.0 - alpha) * bg_lum
        } else {
            fg_lum
        };

        let ratio = contrast_ratio(fg_eff, bg_lum);
        if ratio < 3.0 {
            violations.push(LintViolation {
                rule: LintRule::ContrastCheck,
                node_id: node.id.clone(),
                message: format!("Contrast {:.1}:1 insufficient (minimum 3:1)", ratio),
                suggestion: "Increase the luminance difference between foreground and background"
                    .to_string(),
                severity: LintSeverity::Error,
            });
        } else if ratio < 4.5 {
            violations.push(LintViolation {
                rule: LintRule::ContrastCheck,
                node_id: node.id.clone(),
                message: format!("Contrast {:.1}:1 low (recommended 4.5:1)", ratio),
                suggestion: "Aim for a contrast of 4.5:1 to meet the WCAG AA standard".to_string(),
                severity: LintSeverity::Warning,
            });
        }
    }

    pub(super) fn check_unlabeled_input(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
    ) {
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
                message: format!("Input control '{}' is missing a label", node.name),
                suggestion: "Add a label or placeholder child node".to_string(),
                severity: LintSeverity::Error,
            });
        }
    }

    pub(super) fn check_text_effects(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
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
                    message: format!("Text node '{}' uses a gradient fill", node.name),
                    suggestion: "Text gradients reduce readability; prefer a solid fill"
                        .to_string(),
                    severity: LintSeverity::Warning,
                });
            }
        }
    }

    pub(super) fn check_abnormal_rotation(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
    ) {
        let rotation = node.rotation;

        if rotation.abs() > 90.0 {
            violations.push(LintViolation {
                rule: LintRule::AbnormalRotation,
                node_id: node.id.clone(),
                message: format!(
                    "Node '{}' rotation angle {:.1} deg exceeds 90 deg",
                    node.name, rotation
                ),
                suggestion:
                    "Rotation over 90 deg can make content unreadable; confirm it is intended"
                        .to_string(),
                severity: LintSeverity::Error,
            });
        } else if rotation != 0.0 && (rotation % 15.0).abs() > 0.01 {
            violations.push(LintViolation {
                rule: LintRule::AbnormalRotation,
                node_id: node.id.clone(),
                message: format!(
                    "Node '{}' rotation angle {:.1} deg is not a multiple of 15 deg",
                    node.name, rotation
                ),
                suggestion:
                    "Prefer rotation angles in multiples of 15 deg to keep alignment consistent"
                        .to_string(),
                severity: LintSeverity::Info,
            });
        }
    }

    pub(super) fn check_empty_effects(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        let style = &node.style;

        if let Some(ref fill) = style.fill {
            if fill.trim().is_empty() {
                violations.push(LintViolation {
                    rule: LintRule::EmptyEffects,
                    node_id: node.id.clone(),
                    message: format!(
                        "Node '{}' declares a fill but its value is empty",
                        node.name
                    ),
                    suggestion: "Remove the empty fill declaration or set a color value"
                        .to_string(),
                    severity: LintSeverity::Warning,
                });
            }
        }

        if let Some(ref stroke) = style.stroke {
            if stroke.trim().is_empty() {
                violations.push(LintViolation {
                    rule: LintRule::EmptyEffects,
                    node_id: node.id.clone(),
                    message: format!(
                        "Node '{}' declares a stroke but its value is empty",
                        node.name
                    ),
                    suggestion: "Remove the empty stroke declaration or set a color value"
                        .to_string(),
                    severity: LintSeverity::Warning,
                });
            }
        }

        if let Some(ref ff) = style.font_family {
            if ff.trim().is_empty() {
                violations.push(LintViolation {
                    rule: LintRule::EmptyEffects,
                    node_id: node.id.clone(),
                    message: format!(
                        "Node '{}' declares a font family but its value is empty",
                        node.name
                    ),
                    suggestion: "Remove the empty font declaration or set a font family"
                        .to_string(),
                    severity: LintSeverity::Warning,
                });
            }
        }
    }

    pub(super) fn check_token_inconsistency(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
    ) {
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
                        "Node '{}' fill '{}' matches Token '{}'; prefer referencing the Token directly",
                        node.name, fill, token_name
                    ),
                    suggestion: format!("Use a Token reference instead of the hardcoded value: {}", token_name),
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
                        "Node '{}' stroke '{}' matches Token '{}'; prefer referencing the Token directly",
                        node.name, stroke, token_name
                    ),
                    suggestion: format!("Use a Token reference instead of the hardcoded value: {}", token_name),
                    severity: LintSeverity::Info,
                });
            }
        }
    }

    pub(super) fn check_unnamed_node(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        let default_names = ["Rect", "Text", "Circle", "Image", "Group"];
        if default_names.contains(&node.name.as_str()) {
            violations.push(LintViolation {
                rule: LintRule::UnnamedNode,
                node_id: node.id.clone(),
                message: format!(
                    "Node '{}' uses a default name, lacking a semantic identifier",
                    node.name
                ),
                suggestion: "Give the node a meaningful name for maintainability".to_string(),
                severity: LintSeverity::Warning,
            });
        }
    }

    pub(super) fn check_text_overflow(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        if node.kind != NodeKind::Text {
            return;
        }
        // L-11: only flag overflow when text is non-empty and the size cannot fit it.
        // The old impl flagged zero dims + empty text as Error = false positive (placeholder/empty text nodes were mis-reported).
        let text = node.text.as_deref().unwrap_or("").trim();
        if text.is_empty() {
            return;
        }
        let font_size = node.style.font_size.unwrap_or(16.0);
        // Rough estimate: each character is ~font_size wide; needed width = char count x font_size.
        let needed_w = text.chars().count() as f64 * font_size;
        if node.w > 0.0 && needed_w <= node.w {
            return;
        }
        violations.push(LintViolation {
            rule: LintRule::TextOverflow,
            node_id: node.id.clone(),
            message: format!(
                "Text node '{}' content will overflow (w={}, h={}, text length={})",
                node.name,
                node.w,
                node.h,
                text.chars().count()
            ),
            suggestion: "Set width/height on the text node, or use adaptive layout".to_string(),
            severity: LintSeverity::Error,
        });
    }

    pub(super) fn check_overlapping_nodes(
        &self,
        siblings: &[PenNode],
        violations: &mut Vec<LintViolation>,
    ) {
        // PERF-3: degrade when the sibling group exceeds the threshold — O(n^2) worst-case is unbounded.
        // Small documents (<=200) keep exact detection; large documents warn + skip (fail visibly).
        const OVERLAP_CHECK_THRESHOLD: usize = 200;
        if siblings.len() > OVERLAP_CHECK_THRESHOLD {
            tracing::warn!(
                count = siblings.len(),
                threshold = OVERLAP_CHECK_THRESHOLD,
                "too many nodes, skipping overlap detection (O(n^2) degradation), worst-case blowup"
            );
            return;
        }
        // E-25: the old impl only checked when `a.z_index == b.z_index` -> overlaps across different z_index (e.g. a modal over a button) were missed.
        // Now reports on visual overlap (intersecting bboxes); a large z_index difference downgrades to Info (possibly intentional stacking).
        for i in 0..siblings.len() {
            for j in (i + 1)..siblings.len() {
                let a = &siblings[i];
                let b = &siblings[j];
                if !rects_overlap(a, b) {
                    continue;
                }
                let z_diff = (a.z_index - b.z_index).abs();
                let severity = if z_diff > 1 {
                    LintSeverity::Info
                } else {
                    LintSeverity::Warning
                };
                violations.push(LintViolation {
                    rule: LintRule::OverlappingNodes,
                    node_id: format!("{}+{}", a.id, b.id),
                    message: format!(
                        "Nodes '{}' and '{}' have overlapping bounding boxes",
                        a.name, b.name
                    ),
                    suggestion:
                        "Adjust node positions or use different z-index values to avoid occlusion"
                            .to_string(),
                    severity,
                });
            }
        }
    }

    pub(super) fn check_hardcoded_spacing(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
    ) {
        use fd_canvas_core::LayoutMode;
        let layout = &node.style.layout;
        match layout {
            LayoutMode::Flex(flex) => {
                if flex.gap != 0.0 && !node.style.design_token_refs.contains_key("gap") {
                    violations.push(LintViolation {
                        rule: LintRule::HardcodedSpacing,
                        node_id: node.id.clone(),
                        message: format!(
                            "Node '{}' Flex gap={} does not reference a design Token",
                            node.name, flex.gap
                        ),
                        suggestion: "Use design_token_refs to reference a spacing Token"
                            .to_string(),
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
                            "Node '{}' padding(t={}/r={}/b={}/l={}) does not reference a design Token",
                            node.name, p.top, p.right, p.bottom, p.left
                        ),
                        suggestion: "Use design_token_refs to reference a spacing Token"
                            .to_string(),
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
                            "Node '{}' Grid gap=({}, {}) does not reference a design Token",
                            node.name, grid.gap.0, grid.gap.1
                        ),
                        suggestion: "Use design_token_refs to reference a spacing Token"
                            .to_string(),
                        severity: LintSeverity::Info,
                    });
                }
            }
            LayoutMode::Free => {}
        }
    }

    pub(super) fn check_hardcoded_font_size(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
    ) {
        if let Some(fs) = node.style.font_size {
            if fs > 0.0 && !node.style.design_token_refs.contains_key("font_size") {
                violations.push(LintViolation {
                    rule: LintRule::HardcodedFontSize,
                    node_id: node.id.clone(),
                    message: format!(
                        "Node '{}' font_size={} does not reference a design Token",
                        node.name, fs
                    ),
                    suggestion: "Use design_token_refs to reference a typography Token".to_string(),
                    severity: LintSeverity::Info,
                });
            }
        }
    }

    pub(super) fn check_missing_interaction_state(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
    ) {
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
                message: format!(
                    "Interactive control '{}' is missing hover/active state definitions",
                    node.name
                ),
                suggestion: "Add hover/active/focus state child nodes".to_string(),
                severity: LintSeverity::Warning,
            });
        }
    }

    pub(super) fn check_layout_inconsistency(
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
                        "Sibling nodes inconsistent: '{}' uses {} layout, '{}' uses {} layout",
                        siblings[0].name, first_layout_mode, sibling.name, mode
                    ),
                    suggestion: "Sibling nodes should use a consistent layout mode".to_string(),
                    severity: LintSeverity::Warning,
                });
            }
        }
    }

    // ── Issue #41: desigma acceptance-gate detectors (all Error severity) ──

    // Bare hex/rgb/named color literal that matches no token in the design
    // system. A literal that equals a token value is fine (token_inconsistency
    // territory, Info); the gate only rejects off-palette colors. Nodes that
    // carry a `fill`/`stroke` design_token_ref are already token-bound.
    pub(super) fn check_bare_color_literal(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
    ) {
        let system = match &self.design_system {
            Some(s) => s,
            None => return,
        };
        let token_map = build_token_map(system);
        let style = &node.style;
        for (slot, value) in [
            ("fill", style.fill.as_deref()),
            ("stroke", style.stroke.as_deref()),
        ] {
            let color = match value {
                Some(c) if !c.is_empty() && c != "none" && c != "transparent" => c,
                _ => continue,
            };
            if parse_color_any(color).is_none() {
                continue;
            }
            if style.design_token_refs.contains_key(slot) {
                continue;
            }
            if token_map.contains_key(color.trim()) {
                continue;
            }
            let palette: Vec<&str> = token_map
                .iter()
                .filter(|(val, _)| parse_color_any(val).is_some())
                .map(|(_, name)| name.as_str())
                .collect();
            violations.push(LintViolation {
                rule: LintRule::BareColorLiteral,
                node_id: node.id.clone(),
                message: format!(
                    "Node '{}' uses bare color literal '{}' on {} (not a design token)",
                    node.name, color, slot
                ),
                suggestion: format!(
                    "Bind a semantic token via design_token_refs[\"{slot}\"]; available color tokens: {}",
                    palette.join(", ")
                ),
                severity: LintSeverity::Error,
            });
        }
    }

    // Spacing values (flex gap/padding, grid gap) that are neither token-bound
    // nor on the design-system spacing scale. Values on the scale pass (hint:
    // bind the token ref for auto-fix), off-scale values fail the gate.
    pub(super) fn check_off_token_spacing(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
    ) {
        use fd_canvas_core::LayoutMode;
        let system = match &self.design_system {
            Some(s) => s,
            None => return,
        };
        let scale = build_numeric_token_map(system);
        let refs = &node.style.design_token_refs;

        let push = |what: String, val: f64, violations: &mut Vec<LintViolation>| {
            violations.push(LintViolation {
                rule: LintRule::OffTokenSpacing,
                node_id: node.id.clone(),
                message: format!(
                    "Node '{}' {}={} is off the spacing token scale",
                    node.name, what, val
                ),
                suggestion: format!(
                    "Use a spacing token value (available: {})",
                    token_family_hint(system, "spacing")
                ),
                severity: LintSeverity::Error,
            });
        };

        match &node.style.layout {
            LayoutMode::Flex(flex) => {
                if flex.gap != 0.0
                    && !refs.contains_key("gap")
                    && !scale.contains_key(&fmt_num(flex.gap))
                {
                    push("flex gap".to_string(), flex.gap, violations);
                }
                let p = &flex.padding;
                let pad = [p.top, p.right, p.bottom, p.left];
                if pad.iter().any(|v| *v != 0.0) && !refs.contains_key("padding") {
                    for (side, v) in ["top", "right", "bottom", "left"].iter().zip(pad) {
                        if v != 0.0 && !scale.contains_key(&fmt_num(v)) {
                            push(format!("padding.{side}"), v, violations);
                        }
                    }
                }
            }
            LayoutMode::Grid(grid) => {
                for (axis, v) in [("gap.x", grid.gap.0), ("gap.y", grid.gap.1)] {
                    if v != 0.0 && !refs.contains_key("gap") && !scale.contains_key(&fmt_num(v)) {
                        push(axis.to_string(), v, violations);
                    }
                }
            }
            LayoutMode::Free => {}
        }
    }

    // Corner radius not token-bound and not on the radius token scale.
    pub(super) fn check_off_token_radius(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
    ) {
        let system = match &self.design_system {
            Some(s) => s,
            None => return,
        };
        let scale = build_numeric_token_map(system);
        let style = &node.style;
        let radius = match style.radius {
            Some(r) if r > 0.0 => r,
            _ => return,
        };
        if style.design_token_refs.contains_key("radius") {
            return;
        }
        if scale.contains_key(&fmt_num(radius)) {
            return;
        }
        violations.push(LintViolation {
            rule: LintRule::OffTokenRadius,
            node_id: node.id.clone(),
            message: format!(
                "Node '{}' radius={} is off the radius token scale",
                node.name, radius
            ),
            suggestion: format!(
                "Use a radius token value or bind design_token_refs[\"radius\"] (available: {})",
                token_family_hint(system, "radius")
            ),
            severity: LintSeverity::Error,
        });
    }

    // A node color matches a light-mode token whose name has no counterpart in
    // the design system's dark_tokens overlay — that color would stay frozen at
    // its light value in dark mode (desigma Light/Dark collection must map to
    // both shell themes).
    pub(super) fn check_missing_dark_pair(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
    ) {
        let system = match &self.design_system {
            Some(s) => s,
            None => return,
        };
        let dark = match &system.dark_tokens {
            Some(d) if !d.is_empty() => d,
            _ => return,
        };
        let token_map = build_token_map(system);
        let style = &node.style;
        for (slot, value) in [
            ("fill", style.fill.as_deref()),
            ("stroke", style.stroke.as_deref()),
        ] {
            let color = match value {
                Some(c) if !c.is_empty() => c.trim(),
                _ => continue,
            };
            let token_name = match token_map.get(color) {
                Some(n) => n,
                None => continue,
            };
            if dark.iter().any(|t| t.name == *token_name) {
                continue;
            }
            violations.push(LintViolation {
                rule: LintRule::MissingDarkPair,
                node_id: node.id.clone(),
                message: format!(
                    "Node '{}' {} uses token '{}' which has no dark-mode pair",
                    node.name, slot, token_name
                ),
                suggestion: format!(
                    "Add '{}' to the design system's dark_tokens overlay (or extend the desigma Theme collection's Dark mode)",
                    token_name
                ),
                severity: LintSeverity::Error,
            });
        }
    }
}

// f64 keys in a HashMap come from token Numbers formatted with `{}`; format
// node values the same way so 16.0 matches "16".
fn fmt_num(v: f64) -> String {
    format!("{v}")
}

// Hint listing a token family's Number values straight from the design system
// (not the value-keyed numeric map, where equal values across families collide).
fn token_family_hint(system: &fd_design_system::DesignSystem, family: &str) -> String {
    let mut values: Vec<(i64, &str)> = system
        .tokens
        .iter()
        .filter_map(|t| match &t.value {
            fd_design_system::TokenValue::Number(n) if t.name.contains(family) => {
                Some((*n as i64, t.name.as_str()))
            }
            _ => None,
        })
        .collect();
    values.sort();
    values
        .iter()
        .map(|(v, n)| format!("{v}={n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn rects_overlap(a: &PenNode, b: &PenNode) -> bool {
    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
}
