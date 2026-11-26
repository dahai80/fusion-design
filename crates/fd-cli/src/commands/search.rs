//! A-3: Search subcommand handler (issue #44).
//!
//! Design-intelligence dataset search: deterministic keyword scoring over embedded CSVs
//! (styles/palettes/typography/ux_guidelines/charts/stacks), zero HTTP, zero model calls.
//! `--json` emits hit structs for scripts/cowork pipelines; human-readable text by default.

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
