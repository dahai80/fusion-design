//! A-3：Search 子命令 handler（issue #44）。
//!
//! Design-intelligence 数据集检索：内置 CSV（styles/palettes/typography/
//! ux_guidelines/charts/stacks）确定性关键词打分，零 HTTP 零模型。
//! `--json` 输出 hit 结构供脚本/cowork 管道消费，默认人读文本。

pub fn run(kind: String, query: String, limit: usize, json: bool) -> anyhow::Result<()> {
    let intel = fd_search::DesignIntel::embedded();
    let hits = intel.search_kind(&kind, &query, limit).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown kind: {} (valid: {})",
            kind,
            fd_search::KINDS.join(" | ") + " | all"
        )
    })?;
    if json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(());
    }
    if hits.is_empty() {
        println!("no matches for query: {query}");
        return Ok(());
    }
    for h in hits {
        println!("{} [{}] {} — {}", h.id, h.kind, h.title, h.detail);
    }
    Ok(())
}
