//! fd-search — design-intelligence data layer (issue #44).
//!
//! Loads curated CSV datasets embedded at compile time (offline hard
//! constraint: no network, no model calls) and answers keyword queries
//! with deterministic scoring. Consumers: fd-server (generation context
//! injection + search API) and fd-cli (`search` subcommand).

use serde::Serialize;

pub const STYLES_CSV: &str = include_str!("../../../datasets/styles.csv");
pub const PALETTES_CSV: &str = include_str!("../../../datasets/palettes.csv");
pub const FONT_PAIRINGS_CSV: &str = include_str!("../../../datasets/font_pairings.csv");
pub const UX_GUIDELINES_CSV: &str = include_str!("../../../datasets/ux_guidelines.csv");
pub const CHARTS_CSV: &str = include_str!("../../../datasets/charts.csv");
pub const STACKS_CSV: &str = include_str!("../../../datasets/stacks.csv");

pub const KINDS: [&str; 6] = [
    "styles",
    "palettes",
    "font_pairings",
    "ux_guidelines",
    "charts",
    "stacks",
];

#[derive(Debug, Clone, Serialize)]
pub struct StyleRecord {
    pub id: String,
    pub name: String,
    pub category: String,
    pub keywords: String,
    pub palette_hint: String,
    pub effects: String,
    pub best_for: String,
    pub avoid_for: String,
    pub dark_ok: String,
    pub complexity: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaletteRecord {
    pub id: String,
    pub name: String,
    pub mood: String,
    pub primary: String,
    pub accent: String,
    pub bg: String,
    pub fg: String,
    pub success: String,
    pub warning: String,
    pub error: String,
    pub dark_bg: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FontPairing {
    pub id: String,
    pub name: String,
    pub mood: String,
    pub heading_font: String,
    pub body_font: String,
    pub best_for: String,
    pub license: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UxGuideline {
    pub id: String,
    pub category: String,
    pub guideline: String,
    pub do_text: String,
    pub dont_text: String,
    pub severity: String,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChartRecord {
    pub id: String,
    pub data_shape: String,
    pub best_chart: String,
    pub when_to_use: String,
    pub avoid_when: String,
    pub keywords: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StackRecord {
    pub id: String,
    pub stack: String,
    pub category: String,
    pub guideline: String,
    pub do_text: String,
    pub dont_text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub kind: &'static str,
    pub id: String,
    pub title: String,
    pub detail: String,
    pub score: i64,
}

#[derive(Debug, Default)]
pub struct DesignIntel {
    pub styles: Vec<StyleRecord>,
    pub palettes: Vec<PaletteRecord>,
    pub font_pairings: Vec<FontPairing>,
    pub ux_guidelines: Vec<UxGuideline>,
    pub charts: Vec<ChartRecord>,
    pub stacks: Vec<StackRecord>,
}

fn tokenize(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_string())
        .collect()
}

fn score_field(tokens: &[String], text: &str, weight: i64) -> i64 {
    let lower = text.to_lowercase();
    tokens.iter().filter(|t| lower.contains(t.as_str())).count() as i64 * weight
}

fn score_name_exact(tokens: &[String], name: &str) -> i64 {
    let lower = name.to_lowercase();
    if !tokens.is_empty() && tokens.iter().any(|t| lower.contains(t.as_str())) {
        3
    } else {
        0
    }
}

fn take_top(mut hits: Vec<Hit>, limit: usize) -> Vec<Hit> {
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    hits.truncate(limit);
    hits
}

fn read_csv(text: &str, expected_cols: usize, what: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(false)
        .from_reader(text.as_bytes());
    for (i, rec) in reader.records().enumerate() {
        match rec {
            Ok(r) => {
                if r.len() != expected_cols {
                    tracing::warn!(
                        dataset = what,
                        row = i + 2,
                        cols = r.len(),
                        "csv row column count mismatch, skipped"
                    );
                    continue;
                }
                rows.push(r.iter().map(|s| s.trim().to_string()).collect());
            }
            Err(e) => {
                tracing::warn!(dataset = what, row = i + 2, error = %e, "csv row parse failed, skipped");
            }
        }
    }
    rows
}

impl DesignIntel {
    pub fn embedded() -> Self {
        let mut intel = DesignIntel::default();
        for row in read_csv(STYLES_CSV, 10, "styles.csv") {
            intel.styles.push(StyleRecord {
                id: row[0].clone(),
                name: row[1].clone(),
                category: row[2].clone(),
                keywords: row[3].clone(),
                palette_hint: row[4].clone(),
                effects: row[5].clone(),
                best_for: row[6].clone(),
                avoid_for: row[7].clone(),
                dark_ok: row[8].clone(),
                complexity: row[9].clone(),
            });
        }
        for row in read_csv(PALETTES_CSV, 12, "palettes.csv") {
            intel.palettes.push(PaletteRecord {
                id: row[0].clone(),
                name: row[1].clone(),
                mood: row[2].clone(),
                primary: row[3].clone(),
                accent: row[4].clone(),
                bg: row[5].clone(),
                fg: row[6].clone(),
                success: row[7].clone(),
                warning: row[8].clone(),
                error: row[9].clone(),
                dark_bg: row[10].clone(),
            });
        }
        for row in read_csv(FONT_PAIRINGS_CSV, 7, "font_pairings.csv") {
            intel.font_pairings.push(FontPairing {
                id: row[0].clone(),
                name: row[1].clone(),
                mood: row[2].clone(),
                heading_font: row[3].clone(),
                body_font: row[4].clone(),
                best_for: row[5].clone(),
                license: row[6].clone(),
            });
        }
        for row in read_csv(UX_GUIDELINES_CSV, 7, "ux_guidelines.csv") {
            intel.ux_guidelines.push(UxGuideline {
                id: row[0].clone(),
                category: row[1].clone(),
                guideline: row[2].clone(),
                do_text: row[3].clone(),
                dont_text: row[4].clone(),
                severity: row[5].clone(),
                platform: row[6].clone(),
            });
        }
        for row in read_csv(CHARTS_CSV, 6, "charts.csv") {
            intel.charts.push(ChartRecord {
                id: row[0].clone(),
                data_shape: row[1].clone(),
                best_chart: row[2].clone(),
                when_to_use: row[3].clone(),
                avoid_when: row[4].clone(),
                keywords: row[5].clone(),
            });
        }
        for row in read_csv(STACKS_CSV, 6, "stacks.csv") {
            intel.stacks.push(StackRecord {
                id: row[0].clone(),
                stack: row[1].clone(),
                category: row[2].clone(),
                guideline: row[3].clone(),
                do_text: row[4].clone(),
                dont_text: row[5].clone(),
            });
        }
        tracing::info!(
            styles = intel.styles.len(),
            palettes = intel.palettes.len(),
            font_pairings = intel.font_pairings.len(),
            ux_guidelines = intel.ux_guidelines.len(),
            charts = intel.charts.len(),
            stacks = intel.stacks.len(),
            "fd-search: embedded design datasets loaded"
        );
        intel
    }

    pub fn styles(&self, query: &str, limit: usize) -> Vec<Hit> {
        let tokens = tokenize(query);
        let mut hits = Vec::new();
        for s in &self.styles {
            let mut score = score_name_exact(&tokens, &s.name);
            score += score_field(&tokens, &s.keywords, 4);
            score += score_field(&tokens, &s.category, 3);
            score += score_field(&tokens, &s.best_for, 3);
            score += score_field(&tokens, &s.palette_hint, 1);
            if score > 0 {
                hits.push(Hit {
                    kind: "styles",
                    id: s.id.clone(),
                    title: s.name.clone(),
                    detail: format!(
                        "palette {} · effects {} · best for {} · avoid {} · dark {} · complexity {}",
                        s.palette_hint, s.effects, s.best_for, s.avoid_for, s.dark_ok, s.complexity
                    ),
                    score,
                });
            }
        }
        take_top(hits, limit)
    }

    pub fn palettes(&self, query: &str, limit: usize) -> Vec<Hit> {
        let tokens = tokenize(query);
        let mut hits = Vec::new();
        for p in &self.palettes {
            let mut score = score_name_exact(&tokens, &p.name);
            score += score_field(&tokens, &p.mood, 4);
            if score > 0 {
                hits.push(Hit {
                    kind: "palettes",
                    id: p.id.clone(),
                    title: p.name.clone(),
                    detail: format!(
                        "primary {} accent {} bg {} fg {} success {} warning {} error {} dark_bg {}",
                        p.primary, p.accent, p.bg, p.fg, p.success, p.warning, p.error, p.dark_bg
                    ),
                    score,
                });
            }
        }
        take_top(hits, limit)
    }

    pub fn font_pairings(&self, query: &str, limit: usize) -> Vec<Hit> {
        let tokens = tokenize(query);
        let mut hits = Vec::new();
        for f in &self.font_pairings {
            let mut score = score_name_exact(&tokens, &f.name);
            score += score_field(&tokens, &f.mood, 4);
            score += score_field(&tokens, &f.best_for, 3);
            score += score_field(&tokens, &f.heading_font, 1);
            if score > 0 {
                hits.push(Hit {
                    kind: "font_pairings",
                    id: f.id.clone(),
                    title: format!("{} + {}", f.heading_font, f.body_font),
                    detail: format!(
                        "mood {} · best for {} · license {}",
                        f.mood, f.best_for, f.license
                    ),
                    score,
                });
            }
        }
        take_top(hits, limit)
    }

    pub fn ux_guidelines(&self, query: &str, limit: usize) -> Vec<Hit> {
        let tokens = tokenize(query);
        let mut hits = Vec::new();
        for g in &self.ux_guidelines {
            let mut score = score_field(&tokens, &g.guideline, 4);
            score += score_field(&tokens, &g.category, 3);
            score += score_field(&tokens, &g.do_text, 1);
            if score > 0 {
                hits.push(Hit {
                    kind: "ux_guidelines",
                    id: g.id.clone(),
                    title: g.guideline.clone(),
                    detail: format!(
                        "do: {} · don't: {} · [{} · {}]",
                        g.do_text, g.dont_text, g.severity, g.platform
                    ),
                    score,
                });
            }
        }
        take_top(hits, limit)
    }

    pub fn charts(&self, query: &str, limit: usize) -> Vec<Hit> {
        let tokens = tokenize(query);
        let mut hits = Vec::new();
        for c in &self.charts {
            let mut score = score_field(&tokens, &c.keywords, 4);
            score += score_field(&tokens, &c.data_shape, 3);
            score += score_field(&tokens, &c.when_to_use, 1);
            if score > 0 {
                hits.push(Hit {
                    kind: "charts",
                    id: c.id.clone(),
                    title: c.best_chart.clone(),
                    detail: format!("use when {} · avoid when {}", c.when_to_use, c.avoid_when),
                    score,
                });
            }
        }
        take_top(hits, limit)
    }

    pub fn stacks(&self, query: &str, limit: usize) -> Vec<Hit> {
        let tokens = tokenize(query);
        let mut hits = Vec::new();
        for s in &self.stacks {
            let mut score = score_field(&tokens, &s.stack, 5);
            score += score_field(&tokens, &s.category, 2);
            score += score_field(&tokens, &s.guideline, 2);
            if score > 0 {
                hits.push(Hit {
                    kind: "stacks",
                    id: s.id.clone(),
                    title: format!("{} ({})", s.stack, s.category),
                    detail: format!(
                        "{} · do: {} · don't: {}",
                        s.guideline, s.do_text, s.dont_text
                    ),
                    score,
                });
            }
        }
        take_top(hits, limit)
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let mut all = Vec::new();
        all.extend(self.styles(query, limit));
        all.extend(self.palettes(query, limit));
        all.extend(self.font_pairings(query, limit));
        all.extend(self.ux_guidelines(query, limit));
        all.extend(self.charts(query, limit));
        all.extend(self.stacks(query, limit));
        take_top(all, limit)
    }

    pub fn search_kind(&self, kind: &str, query: &str, limit: usize) -> Option<Vec<Hit>> {
        match kind {
            "styles" => Some(self.styles(query, limit)),
            "palettes" => Some(self.palettes(query, limit)),
            "font_pairings" => Some(self.font_pairings(query, limit)),
            "ux_guidelines" => Some(self.ux_guidelines(query, limit)),
            "charts" => Some(self.charts(query, limit)),
            "stacks" => Some(self.stacks(query, limit)),
            "all" => Some(self.search(query, limit)),
            _ => None,
        }
    }

    /// One-shot generation context: top style + palette + typography +
    /// ux guardrails + chart hint for a natural-language prompt.
    /// Deterministic keyword retrieval — no model, no network.
    pub fn recommend(&self, prompt: &str, max_ux: usize) -> Option<String> {
        let style = self.styles(prompt, 1).into_iter().next()?;
        let palette = self.palettes(prompt, 1).into_iter().next();
        let typography = self.font_pairings(prompt, 1).into_iter().next();
        let ux = self.ux_guidelines(prompt, max_ux.max(1));
        let chart = self.charts(prompt, 1).into_iter().next();

        let mut block = String::from("[Design intelligence]\n");
        block.push_str(&format!("Style: {} — {}\n", style.title, style.detail));
        if let Some(p) = palette {
            block.push_str(&format!("Palette: {} — {}\n", p.title, p.detail));
        }
        if let Some(t) = typography {
            block.push_str(&format!(
                "Typography: {} — {} ({})\n",
                t.title, t.detail, t.id
            ));
        }
        if !ux.is_empty() {
            block.push_str("UX guardrails:\n");
            for g in ux {
                block.push_str(&format!("- {} (do: {})\n", g.title, g.detail));
            }
        }
        if let Some(c) = chart {
            block.push_str(&format!("Chart: {} — {}\n", c.title, c.detail));
        }
        Some(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intel() -> DesignIntel {
        DesignIntel::embedded()
    }

    #[test]
    fn embedded_datasets_nonempty() {
        let i = intel();
        assert!(i.styles.len() >= 20, "styles {}", i.styles.len());
        assert!(i.palettes.len() >= 20, "palettes {}", i.palettes.len());
        assert!(
            i.font_pairings.len() >= 15,
            "pairings {}",
            i.font_pairings.len()
        );
        assert!(i.ux_guidelines.len() >= 30, "ux {}", i.ux_guidelines.len());
        assert!(i.charts.len() >= 10, "charts {}", i.charts.len());
        assert!(i.stacks.len() >= 10, "stacks {}", i.stacks.len());
    }

    #[test]
    fn unique_ids_per_dataset() {
        let i = intel();
        let check = |ids: Vec<&str>, what: &str| {
            let mut sorted = ids.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), ids.len(), "duplicate id in {what}");
        };
        check(i.styles.iter().map(|s| s.id.as_str()).collect(), "styles");
        check(
            i.palettes.iter().map(|p| p.id.as_str()).collect(),
            "palettes",
        );
        check(
            i.font_pairings.iter().map(|f| f.id.as_str()).collect(),
            "font_pairings",
        );
        check(
            i.ux_guidelines.iter().map(|g| g.id.as_str()).collect(),
            "ux_guidelines",
        );
        check(i.charts.iter().map(|c| c.id.as_str()).collect(), "charts");
        check(i.stacks.iter().map(|s| s.id.as_str()).collect(), "stacks");
    }

    #[test]
    fn palette_hex_fields_wellformed() {
        let i = intel();
        for p in &i.palettes {
            for (field, v) in [
                ("primary", &p.primary),
                ("accent", &p.accent),
                ("bg", &p.bg),
                ("fg", &p.fg),
                ("success", &p.success),
                ("warning", &p.warning),
                ("error", &p.error),
                ("dark_bg", &p.dark_bg),
            ] {
                assert_eq!(v.len(), 7, "{} {} field {}", p.id, field, v);
                assert!(v.starts_with('#'), "{} {} field {}", p.id, field, v);
                assert!(
                    v[1..].chars().all(|c| c.is_ascii_hexdigit()),
                    "{} {} field {}",
                    p.id,
                    field,
                    v
                );
            }
        }
    }

    #[test]
    fn every_palette_has_dark_bg() {
        let i = intel();
        for p in &i.palettes {
            assert_ne!(
                p.dark_bg.to_lowercase(),
                p.bg.to_lowercase(),
                "{} dark_bg equals bg",
                p.id
            );
        }
    }

    #[test]
    fn style_search_ranks_dashboards_first_for_dashboard_query() {
        let i = intel();
        let hits = i.styles("dashboard data dense table", 3);
        assert!(!hits.is_empty());
        assert!(hits[0].score >= hits[hits.len() - 1].score);
        assert!(
            hits.iter().any(|h| h.id == "st-17"),
            "expected st-17 in top 3: {:?}",
            hits
        );
    }

    #[test]
    fn palette_search_finds_trust_blue() {
        let i = intel();
        let hits = i.palettes("professional trust saas", 3);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, "pal-01");
        assert!(hits[0].detail.contains("#2563EB"));
    }

    #[test]
    fn typography_search_finds_mono_for_developer() {
        let i = intel();
        let hits = i.font_pairings("developer terminal mono", 3);
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|h| h.id == "fp-03"), "{:?}", hits);
    }

    #[test]
    fn chart_search_maps_trend_to_line_chart() {
        let i = intel();
        let hits = i.charts("trend over time growth", 2);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].title, "line chart");
    }

    #[test]
    fn stack_search_filters_by_stack_name() {
        let i = intel();
        let hits = i.stacks("react", 10);
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.title.starts_with("react")));
    }

    #[test]
    fn cross_kind_search_returns_multiple_kinds() {
        let i = intel();
        let hits = i.search("dashboard developer dark", 20);
        let kinds: std::collections::HashSet<&str> = hits.iter().map(|h| h.kind).collect();
        assert!(
            kinds.len() >= 2,
            "expected hits from >= 2 kinds: {:?}",
            kinds
        );
    }

    #[test]
    fn unknown_kind_rejected() {
        let i = intel();
        assert!(i.search_kind("nope", "x", 5).is_none());
        assert!(i.search_kind("styles", "dashboard", 5).is_some());
        assert!(i.search_kind("all", "dashboard", 5).is_some());
    }

    #[test]
    fn recommend_builds_context_block() {
        let i = intel();
        let block = i
            .recommend(
                "developer monitoring dashboard with dark console and dense tables",
                3,
            )
            .expect("recommend should return a block");
        assert!(block.starts_with("[Design intelligence]"));
        assert!(block.contains("Style:"));
        assert!(block.contains("Palette:"));
        assert!(block.contains("UX guardrails:"));
    }

    #[test]
    fn recommend_is_deterministic() {
        let i = intel();
        let a = i
            .recommend("luxury fashion premium boutique", 3)
            .unwrap_or_default();
        let b = i
            .recommend("luxury fashion premium boutique", 3)
            .unwrap_or_default();
        assert_eq!(a, b);
    }

    #[test]
    fn recommend_none_for_unmatched_prompt() {
        let i = intel();
        assert!(i.recommend("zzz qqq xx yy zz", 3).is_none());
    }

    #[test]
    fn empty_query_returns_no_hits() {
        let i = intel();
        assert!(i.styles("", 5).is_empty());
        assert!(i.palettes("   ", 5).is_empty());
    }

    #[test]
    fn limit_respected() {
        let i = intel();
        assert_eq!(i.ux_guidelines("form navigation table search", 2).len(), 2);
    }

    // Data-QA harness (issue #44): provenance manifest must stay in sync
    // with the dataset files — row counts and sha256 checksums verified
    // on every test run, so an unedited CSV without a provenance bump
    // fails CI before any consumer trusts stale data.
    const PROVENANCE_JSON: &str = include_str!("../../../datasets/provenance.json");

    #[test]
    fn provenance_manifest_matches_datasets() {
        #[derive(serde::Deserialize)]
        struct FileEntry {
            name: String,
            rows: usize,
            sha256: String,
        }
        #[derive(serde::Deserialize)]
        struct Provenance {
            files: Vec<FileEntry>,
        }
        let manifest: Provenance =
            serde_json::from_str(PROVENANCE_JSON).expect("provenance.json parses");
        let embedded: &[(&str, &str, usize)] = &[
            ("styles.csv", STYLES_CSV, 10),
            ("palettes.csv", PALETTES_CSV, 12),
            ("font_pairings.csv", FONT_PAIRINGS_CSV, 7),
            ("ux_guidelines.csv", UX_GUIDELINES_CSV, 7),
            ("charts.csv", CHARTS_CSV, 6),
            ("stacks.csv", STACKS_CSV, 6),
        ];
        assert_eq!(manifest.files.len(), embedded.len());
        for entry in &manifest.files {
            let (_, text, cols) = embedded
                .iter()
                .find(|(n, _, _)| *n == entry.name)
                .unwrap_or_else(|| panic!("provenance lists unknown dataset {}", entry.name));
            use sha2::Digest;
            let digest = sha2::Sha256::digest(text.as_bytes());
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(
                hex, entry.sha256,
                "{} sha256 drift vs provenance.json",
                entry.name
            );
            let rows = read_csv(text, *cols, &entry.name).len();
            assert_eq!(
                rows, entry.rows,
                "{} row count drift vs provenance.json",
                entry.name
            );
        }
    }
}
