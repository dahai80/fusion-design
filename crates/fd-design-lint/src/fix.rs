// ARCH-10 r4：auto-fix 与 Token 映射从 lib.rs 拆出。apply_tokens_to_document 为 pub
// （fd-cli 消费），fix_detail/FixResult 类型留 lib.rs（公共 API），此处 import。
// 行为零变更——纯位置迁移。

use std::collections::HashMap;

use fd_canvas_core::{NodeKind, PenDocument, PenNode};
use fd_design_system::DesignSystem;
use tracing::info;

use crate::{build_numeric_token_map, build_token_map, FixDetail, FixResult, LintRule};

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
