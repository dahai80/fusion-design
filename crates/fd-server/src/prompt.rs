//! 迭代提示词装配（issue#34：stable-prefix + 区域外置 + 变更日志）。
//!
//! 排序契约（配 fusion-mlx `enable_prefix_cache`）：
//! 1. sys 恒定不变——prefix cache 可命中的稳定头部（mlx admin `prefix_cache`
//!    聚合指标验证命中率）；
//! 2. user 内：固定模板头 → 文档 TOC（结构摘要）→ 变更日志 → 区域节点 JSON →
//!    本次指令。挥发内容一律殿后，稳定内容不被逐请求内容污染。
//!
//! 大文档迭代走区域选择：只送 region 节点 JSON + 全量 TOC（O(region) 而非
//! O(doc)），模型契约仍是输出完整页面 JSON，未列出节点由服务端保证不变。

use crate::state::Session;

/// 迭代系统提示词（恒定，勿拼接逐请求内容——prefix cache 稳定头部）。
pub const ITERATE_SYS: &str = "你是 fusion-design UI 迭代器。输入当前页面 JSON 与修改指令，\
输出修改后的完整页面 JSON（{\"page\":{...}} 同 schema）。保持 width/height 与\
未提及节点不变，仅落实指令要求。只输出 JSON，禁止额外文字。";

/// 文档 TOC：每节点一行 `id kind name`，跨页汇总。解析失败返回空串（容错）。
pub fn toc_of(doc_json: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(doc_json) {
        if let Some(pages) = v.get("pages").and_then(|p| p.as_array()) {
            for p in pages {
                let page = p.get("name").and_then(|n| n.as_str()).unwrap_or("page");
                if let Some(nodes) = p.get("nodes").and_then(|n| n.as_array()) {
                    for n in nodes {
                        let id = n.get("id").and_then(|x| x.as_str()).unwrap_or("?");
                        let kind = n.get("kind").and_then(|x| x.as_str()).unwrap_or("?");
                        let name = n.get("name").and_then(|x| x.as_str()).unwrap_or("-");
                        lines.push(format!("- {page}/{id} {kind} {name}"));
                    }
                }
            }
        }
    }
    lines.join("\n")
}

/// 区域选择：按节点 id 过滤各页 nodes，保留页壳（width/height/name）。
/// 返回过滤后的页面 JSON；无任何命中返回 None（调用方 400，fail visibly）。
pub fn select_region(doc_json: &str, node_ids: &[String]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(doc_json).ok()?;
    let mut out = Vec::new();
    if let Some(pages) = v.get("pages").and_then(|p| p.as_array()) {
        for p in pages {
            let nodes = p.get("nodes").and_then(|n| n.as_array())?;
            let kept: Vec<&serde_json::Value> = nodes
                .iter()
                .filter(|n| {
                    n.get("id")
                        .and_then(|x| x.as_str())
                        .map(|id| node_ids.iter().any(|r| r == id))
                        .unwrap_or(false)
                })
                .collect();
            if kept.is_empty() {
                continue;
            }
            let mut page = p.clone();
            page["nodes"] = serde_json::Value::Array(kept.into_iter().cloned().collect());
            out.push(page);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(serde_json::json!({ "pages": out }).to_string())
    }
}

/// 变更日志：取版本 label（生成/迭代指令截断），最近 8 条一行一条；
/// 更早的折叠为一行汇总。首个版本（initial/还原）计一行背景。
pub fn changelog_of(sess: &Session) -> String {
    let mut lines: Vec<String> = Vec::new();
    let total = sess.versions.len();
    let skip = total.saturating_sub(8);
    if skip > 1 {
        lines.push(format!("- （此前共 {} 个版本）", skip));
    }
    for v in sess.versions.iter().skip(skip) {
        lines.push(format!("- v{}: {}", v.version, v.label));
    }
    lines.join("\n")
}

/// 迭代 user 提示词。排序固定：TOC → 变更日志 → 基准 JSON → 指令（挥发殿后）。
/// `region` 非空时基准为区域 JSON（O(region)），并在指令前声明未列出节点不变。
pub fn build_iterate_user(
    doc_json: &str,
    region: Option<&[String]>,
    changelog: &str,
    instruction: &str,
) -> String {
    let toc = toc_of(doc_json);
    let (base_note, base) = match region {
        Some(ids) if !ids.is_empty() => match select_region(doc_json, ids) {
            Some(json) => (
                "以下仅为本次要修改的区域节点。只输出这些节点的修改结果：\
                 {\"page\":{\"nodes\":[...]}}（保留各自 id，不要输出其他节点；\
                 服务端会把结果合并回完整页面，未列出节点由服务端保证不变）"
                    .to_string(),
                json,
            ),
            None => ("当前页面：".to_string(), doc_json.to_string()),
        },
        _ => ("当前页面：".to_string(), doc_json.to_string()),
    };
    format!(
        "文档结构（TOC）：\n{toc}\n\n最近变更：\n{cl}\n\n{note}\n{base}\n\n修改指令：{ins}",
        toc = if toc.is_empty() {
            "（空文档）"
        } else {
            &toc
        },
        cl = if changelog.is_empty() {
            "（无）"
        } else {
            changelog
        },
        note = base_note,
        base = base,
        ins = instruction,
    )
}

/// 区域结果合并回完整页面：base 中命中 region 的节点按序替换为模型输出节点
/// （多余输出节点追加到首页尾部），未命中节点原样保留。确定性拼接（Rule 5：
/// 决策走代码不走模型），模型无法回显未见过的节点。
pub fn merge_region_nodes(
    base_json: &str,
    model_nodes: &[serde_json::Value],
    region_ids: &[String],
) -> Option<String> {
    let mut base: serde_json::Value =
        serde_json::from_str(base_json).unwrap_or_else(|_| serde_json::json!({ "pages": [] }));
    // 按序匹配：模型被要求只输出区域节点（按 base 出现顺序），位置一一对应；
    // 额外节点（无对应 base 槽位）追加到首页尾部。
    let mut positional: Vec<serde_json::Value> = model_nodes.to_vec();
    if model_nodes.is_empty() {
        return None;
    }
    let mut replaced = 0usize;
    if let Some(pages) = base.get_mut("pages").and_then(|p| p.as_array_mut()) {
        for page in pages.iter_mut() {
            if let Some(nodes) = page.get_mut("nodes").and_then(|n| n.as_array_mut()) {
                let mut out: Vec<serde_json::Value> = Vec::with_capacity(nodes.len());
                for n in nodes.drain(..) {
                    let id = n.get("id").and_then(|x| x.as_str()).unwrap_or("");
                    if region_ids.iter().any(|r| r == id) {
                        // 模型明示删除该节点时无替换候选（repl 为 None，原节点不保留）。
                        if let Some(v) = (!positional.is_empty()).then(|| positional.remove(0)) {
                            replaced += 1;
                            out.push(v);
                        }
                    } else {
                        out.push(n);
                    }
                }
                *nodes = out;
            }
        }
        if !positional.is_empty() {
            if let Some(first) = pages.first_mut() {
                if let Some(nodes) = first.get_mut("nodes").and_then(|n| n.as_array_mut()) {
                    nodes.append(&mut positional);
                }
            }
        }
    }
    tracing::info!(
        requested = region_ids.len(),
        replaced,
        "merge_region_nodes: 区域合并完成"
    );
    Some(base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc() -> String {
        serde_json::json!({
            "pages": [{
                "id": "p1", "name": "home", "width": 1440.0, "height": 900.0,
                "nodes": [
                    {"id": "n1", "kind": "rect", "name": "bg", "x": 0.0, "y": 0.0, "w": 100.0, "h": 50.0},
                    {"id": "n2", "kind": "text", "name": "title", "x": 10.0, "y": 10.0, "w": 80.0, "h": 20.0},
                    {"id": "n3", "kind": "circle", "name": "avatar", "x": 20.0, "y": 20.0, "w": 30.0, "h": 30.0}
                ]
            }]
        })
        .to_string()
    }

    #[test]
    fn toc_lists_nodes_with_page_prefix() {
        let toc = toc_of(&sample_doc());
        assert!(toc.contains("home/n1 rect bg"), "{toc}");
        assert!(toc.contains("home/n3 circle avatar"), "{toc}");
        assert_eq!(toc.lines().count(), 3);
        assert_eq!(toc_of("not json"), "");
    }

    #[test]
    fn region_selects_only_matching_nodes() {
        let r = select_region(&sample_doc(), &["n2".to_string()]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        let nodes = v["pages"][0]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1, "{r}");
        assert_eq!(nodes[0]["id"], "n2");
        // 页壳保留（width/height），模型输出整页时尺寸契约不丢。
        assert_eq!(v["pages"][0]["width"], 1440.0);
        assert_eq!(select_region(&sample_doc(), &["nope".to_string()]), None);
    }

    #[test]
    fn changelog_folds_old_versions() {
        let mut sess = crate::state::Session {
            id: "s-t".into(),
            created_at: "t".into(),
            versions: vec![],
            current: 1,
            last_used: 0,
        };
        for i in 1..=12 {
            sess.versions.push(crate::state::VersionEntry {
                version: i,
                label: format!("迭代：指令{i}"),
                created_at: "t".into(),
                node_count: 1,
                document: "{}".into(),
            });
        }
        let cl = changelog_of(&sess);
        assert!(cl.contains("此前共 4 个版本"), "{cl}");
        assert!(cl.contains("v12: 迭代：指令12"), "{cl}");
        assert!(!cl.contains("指令4"), "{cl}");
    }

    #[test]
    fn merge_region_replaces_only_selected() {
        let base = sample_doc();
        let model_nodes = vec![serde_json::json!({
            "id": "n2", "kind": "text", "name": "title", "x": 10.0, "y": 10.0,
            "w": 120.0, "h": 30.0, "text": "BIG"
        })];
        let merged = merge_region_nodes(&base, &model_nodes, &["n2".to_string()]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let nodes = v["pages"][0]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3, "其余节点保留: {merged}");
        assert_eq!(nodes[1]["text"], "BIG", "n2 被模型结果替换");
        assert_eq!(nodes[0]["id"], "n1", "未选中节点原样");
        assert_eq!(nodes[2]["id"], "n3", "未选中节点原样");
        // 模型新增节点追加到首页尾部。
        let extra = vec![
            serde_json::json!({"id": "n2", "kind": "text", "name": "t2", "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0}),
            serde_json::json!({"id": "n9", "kind": "rect", "name": "new", "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0}),
        ];
        let merged2 = merge_region_nodes(&base, &extra, &["n2".to_string()]).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&merged2).unwrap();
        let nodes2 = v2["pages"][0]["nodes"].as_array().unwrap();
        assert_eq!(nodes2.len(), 4, "n2 替换 + n9 追加: {merged2}");
        assert_eq!(nodes2[3]["id"], "n9");
    }

    #[test]
    fn merge_region_without_replacements_fails() {
        // 模型零节点回包 → None（调用方 502，绝不静默丢节点）。
        assert!(merge_region_nodes(&sample_doc(), &[], &["n2".to_string()]).is_none());
    }

    #[test]
    fn iterate_user_orders_stable_to_volatile() {
        let doc = sample_doc();
        let user = build_iterate_user(
            &doc,
            Some(&["n2".to_string()]),
            "- v2: 加暗黑模式",
            "把标题放大",
        );
        let pos_toc = user.find("文档结构").unwrap();
        let pos_cl = user.find("最近变更").unwrap();
        let pos_region = user.find("区域节点").unwrap();
        let pos_ins = user.find("修改指令").unwrap();
        assert!(
            pos_toc < pos_cl && pos_cl < pos_region && pos_region < pos_ins,
            "{user}"
        );
        // 区域模式：基准 JSON 只有 n2 一个节点对象（TOC 提及不算，O(region)）。
        assert_eq!(
            user.matches("\"kind\"").count(),
            1,
            "区域 JSON 不得携带未选节点对象: {user}"
        );
        assert!(user.contains("服务端会把结果合并回完整页面"), "{user}");
        // 无区域：回退全量文档。
        let full = build_iterate_user(&doc, None, "", "改背景色");
        assert!(full.contains("当前页面"), "{full}");
        assert!(full.contains("\"avatar\""), "{full}");
    }
}
