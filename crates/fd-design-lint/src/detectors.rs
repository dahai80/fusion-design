// ARCH-10 r4：13 个检测器从 lib.rs 拆出。均为 Linter 方法，读 self.rules / self.design_system。
// 行为零变更——纯位置迁移，可见性不变（pub(crate) 自测）。

use fd_canvas_core::{NodeKind, PenNode};

use crate::{build_token_map, color::*, LintRule, LintSeverity, LintViolation};

impl super::Linter {
    pub(super) fn check_contrast(
        &self,
        node: &PenNode,
        violations: &mut Vec<LintViolation>,
        parent_fill: Option<&str>,
    ) {
        // L-9：仅 Text 节点查对比度（矩形/圆无文字不查，消假阳性）。
        // fg=style.fill（文字色，codegen/host-web 渲染文本用 fill 作色），
        // bg=parent_fill（父节点背景，本节点无 fill 时文本落在父背景上）。
        // 无父背景 → 跳过（消假阴性，不臆造黑底）。
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

        // E-14：alpha/opacity 混合近似。节点 style.opacity < 1.0 时，前景按 alpha
        // 混合到背景再做对比度（rgba 半透文本落在不透明背景上的有效对比度）。
        // 简化为线性亮度混合：L_eff = alpha*fg_lum + (1-alpha)*bg_lum。
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
                message: format!("输入控件 '{}' 缺少标签", node.name),
                suggestion: "添加 label 或 placeholder 子节点".to_string(),
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
                    message: format!("文本节点 '{}' 使用了渐变填充", node.name),
                    suggestion: "文本渐变降低可读性，建议使用纯色填充".to_string(),
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

    pub(super) fn check_empty_effects(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
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

    pub(super) fn check_unnamed_node(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
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

    pub(super) fn check_text_overflow(&self, node: &PenNode, violations: &mut Vec<LintViolation>) {
        if node.kind != NodeKind::Text {
            return;
        }
        // L-11：仅当文本非空且尺寸装不下才标溢出。
        // 旧实现零维 + 空文本也标 Error = 假阳性（占位/空文本节点被误报）。
        let text = node.text.as_deref().unwrap_or("").trim();
        if text.is_empty() {
            return;
        }
        let font_size = node.style.font_size.unwrap_or(16.0);
        // 粗略估算：每字符宽 ≈ font_size，需求宽度 = 字符数 × font_size。
        let needed_w = text.chars().count() as f64 * font_size;
        if node.w > 0.0 && needed_w <= node.w {
            return;
        }
        violations.push(LintViolation {
            rule: LintRule::TextOverflow,
            node_id: node.id.clone(),
            message: format!(
                "文本节点 '{}' 内容将溢出（w={}, h={}, 文本长={}）",
                node.name,
                node.w,
                node.h,
                text.chars().count()
            ),
            suggestion: "为文本节点设置宽高，或使用自适应布局".to_string(),
            severity: LintSeverity::Error,
        });
    }

    pub(super) fn check_overlapping_nodes(
        &self,
        siblings: &[PenNode],
        violations: &mut Vec<LintViolation>,
    ) {
        // PERF-3：兄弟组节点数 > 阈值时降级——O(n²) worst-case 无界。
        // 小文档（≤200）保持精确检测，大文档 warn + skip（fail visibly）。
        const OVERLAP_CHECK_THRESHOLD: usize = 200;
        if siblings.len() > OVERLAP_CHECK_THRESHOLD {
            tracing::warn!(
                count = siblings.len(),
                threshold = OVERLAP_CHECK_THRESHOLD,
                "节点过多跳过重叠检测（O(n²) 降级），worst-case 退化"
            );
            return;
        }
        // E-25：旧实现 `a.z_index == b.z_index` 才检测 → 不同 z_index 的重叠（如弹层叠按钮）漏检。
        // 改为检测视觉重叠（bbox 相交）即报，z_index 差值大时降级为 Info（可能有意叠层）。
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
                    message: format!("节点 '{}' 与 '{}' 边界框重叠", a.name, b.name),
                    suggestion: "调整节点位置或使用不同的 z-index 避免遮挡".to_string(),
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
                    message: format!("节点 '{}' font_size={} 未引用设计 Token", node.name, fs),
                    suggestion: "使用 design_token_refs 引用 typography Token".to_string(),
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
                message: format!("交互控件 '{}' 缺少 hover/active 状态定义", node.name),
                suggestion: "添加 hover/active/focus 状态子节点".to_string(),
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

fn rects_overlap(a: &PenNode, b: &PenNode) -> bool {
    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
}
