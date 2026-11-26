//! Fusion-Design CLI — command-line batch generation/export.
//!
//! Extends PRD module 6 ("Fusion CLI integration").

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod commands;
mod common;
mod logging;

#[derive(Parser)]
#[command(
    name = "fusion-design",
    version,
    about = "Fusion-Design local AI design workbench CLI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// List registered design systems
    ListDesignSystems,
    /// Activate a design system
    Activate { id: String },
    /// Export canvas pages to the given format
    Export {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, value_enum)]
        format: ExportFormatArg,
        #[arg(long)]
        out: PathBuf,
        /// Send export result via fd-ecosystem IPC
        #[arg(long)]
        ipc_base: Option<PathBuf>,
    },
    /// Batch-export multiple pages (input is a JSON array)
    ExportBatch {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, value_enum)]
        format: ExportFormatArg,
        /// Multi-format combined export, comma-separated (e.g. html,svg,json); overrides --format
        #[arg(long, value_delimiter = ',')]
        formats: Option<Vec<ExportFormatArg>>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Text-to-UI: natural-language description → PenDocument JSON
    Generate {
        #[arg(long)]
        prompt: String,
        #[arg(long, default_value = "Home")]
        page: String,
        #[arg(long, default_value = "Qwen3.5-9B-4bit")]
        model: String,
        #[arg(long, default_value = "")]
        endpoint: String,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Send result via fd-ecosystem IPC (instead of direct output)
        #[arg(long)]
        ipc_base: Option<PathBuf>,
        /// Stream SSE tokens (for GUI pipelines to read)
        #[arg(long)]
        stream: bool,
    },
    /// Machine-readable streaming chat: streaming inference interface shared by the
    /// fusion-studio subprocess and CLI pipelines.
    /// Auth / X-Fusion-Route header / endpoint resolution reuse fd-ai-adapter; callers do
    /// not reimplement them.
    ///
    /// Consumer declaration (issue #17 → issue #20 evolution): issue #17 originally
    /// envisioned this subcommand as the studio subprocess entry replacing direct MLX
    /// access; early verification assumed studio went through fusion-gateway TCP so there
    /// was no studio consumer. Later verification showed studio DesignBridge.sendDesignChat
    /// is actually inline URLSession HTTP (self-managed Bearer + SSE parsing +
    /// X-Fusion-Route), duplicating adapter protection logic.
    /// issue #20 closing direction: studio calls this subcommand; auth/RouteGuard/
    /// validate_localhost/false-green(check_generate) reuse the adapter, callers do not
    /// reimplement them. For compatibility with studio's existing runFusionDesignStream
    /// subprocess parser (takes choices[0].delta.content on the `data: ` prefix, ends at
    /// `data: [DONE]`), `--format sse` was added to emit raw OpenAI text/event-stream.
    /// `--format ndjson` (default) keeps the existing issue #17 contract (one
    /// delta/chat_done/error frame per line).
    ///
    /// Streaming and the gateway: the default endpoint goes through fusion-gateway(11432).
    /// The gateway streaming-forward 502 bug (fusion-gateway#108: stream=true connection
    /// refused) was fixed on 2026-08-25 (PR #111 local-first ordering); the real streaming
    /// probe passed (SSE deltas produced normally). If the upstream regresses,
    /// `FUSION_MLX_BASE_URL=http://127.0.0.1:11434` bypasses the gateway to hit MLX directly.
    /// Default model `Qwen3.5-9B-4bit` is a commonly used built-in MLX text model (verified
    /// with real inference), but the MLX deployed model list varies by environment; pass
    /// `--model` with a locally loaded model id explicitly for cross-deployment robustness
    /// (probe real availability with `check-mlx --endpoint ...`).
    Chat {
        #[arg(long, default_value = "Qwen3.5-9B-4bit")]
        model: String,
        #[arg(long, default_value = "")]
        endpoint: String,
        /// Inline system prompt (mutually exclusive with --system-prompt-file, which wins)
        #[arg(long, default_value = "")]
        system_prompt: String,
        #[arg(long)]
        system_prompt_file: Option<PathBuf>,
        /// JSON multi-turn history: [{"role":"user|assistant|system","content":".."}]
        #[arg(long)]
        messages_file: Option<PathBuf>,
        /// RAG context, appended to the tail of the system prompt
        #[arg(long)]
        rag_context_file: Option<PathBuf>,
        #[arg(long, default_value = "4096")]
        max_tokens: u32,
        /// Stream output, enabled by default
        #[arg(long, default_value = "true")]
        stream: bool,
        /// Framing (kept for compatibility; superseded by --format, frames per --format when true)
        #[arg(long, default_value = "true")]
        json: bool,
        /// issue #20: streaming output format. ndjson (default, issue #17 contract) / sse
        /// (raw OpenAI text/event-stream, aligned with the fusion-studio
        /// runFusionDesignStream parser)
        #[arg(long, value_enum, default_value_t = ChatStreamFormatArg::Ndjson)]
        format: ChatStreamFormatArg,
    },
    /// Image-to-UI: sketch/reference image → PenDocument JSON
    ImageToUi {
        #[arg(long)]
        sketch: PathBuf,
        #[arg(long, default_value = "")]
        hint: String,
        #[arg(long, default_value = "Home")]
        page: String,
        #[arg(long, default_value = "Qwen3.5-9B-4bit")]
        model: String,
        #[arg(long, default_value = "")]
        endpoint: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Multi-variant comparison: generate 3 differently styled designs at once
    MultiVariants {
        #[arg(long)]
        prompt: String,
        #[arg(long, default_value = "Home")]
        page: String,
        /// Three styles, comma-separated (defaults to the three default styles)
        #[arg(long, value_delimiter = ',')]
        styles: Option<Vec<String>>,
        #[arg(long, default_value = "Qwen3.5-9B-4bit")]
        model: String,
        #[arg(long, default_value = "")]
        endpoint: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// SpecDoc: AI-generated design spec document
    SpecDoc {
        #[arg(long)]
        input: PathBuf,
        // i18n: user-facing
        #[arg(long, default_value = "设计规范文档")]
        title: String,
        #[arg(long, default_value = "Qwen3.5-9B-4bit")]
        model: String,
        #[arg(long, default_value = "")]
        endpoint: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// PageFlow: batch-generate multiple pages from a flow description
    PageFlow {
        #[arg(long)]
        flow: String,
        #[arg(long, default_value = "")]
        style_hint: String,
        #[arg(long, default_value = "Qwen3.5-9B-4bit")]
        model: String,
        #[arg(long, default_value = "")]
        endpoint: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Validate a frontend static-asset directory
    CheckFrontend {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value = "")]
        backend: String,
    },
    /// Validate that the fusion-mlx endpoint is localhost
    CheckMlx {
        #[arg(long, default_value = "")]
        endpoint: String,
        /// Model id for the inference probe; if empty, uses the FUSION_MLX_MODEL env var,
        /// then falls back to the first entry of /v1/models (the gateway mixes cloud and
        /// local models and the first may not be loaded; pass a local mlx model id
        /// explicitly for a real availability verdict).
        #[arg(long, default_value = "")]
        model: String,
    },
    /// HTML → PenDocument JSON conversion
    ParseHtml {
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        page: Option<String>,
    },
    /// Emit CSS Custom Properties for the active design system
    TokenCSS {
        #[arg(long, default_value = "apple-hig")]
        design_system: String,
    },
    /// Design-spec lint: PenDocument → LintResult
    Lint {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "apple-hig")]
        design_system: Option<String>,
        #[arg(long, value_enum, default_value = "default")]
        profile: LintProfileArg,
        #[arg(long, value_enum)]
        rules: Option<Vec<LintRuleArg>>,
        /// Auto-fix fixable violations (token references, empty-value cleanup, auto naming)
        #[arg(long)]
        fix: bool,
        /// Preview fixes only, do not write files
        #[arg(long, requires = "fix")]
        dry_run: bool,
        /// CI gate mode: exit 1 when any Error-severity violation exists
        #[arg(long)]
        gate: bool,
    },
    /// Design-intelligence dataset search (issue #44): curated styles,
    /// palettes, typography, ux guidelines, charts, per-stack notes.
    Search {
        /// Dataset kind: styles | palettes | font_pairings | ux_guidelines | charts | stacks | all
        #[arg(long, default_value = "all")]
        kind: String,
        /// Free-text query (keyword scoring, deterministic)
        #[arg(long)]
        query: String,
        /// Max hits per query
        #[arg(long, default_value_t = 5)]
        limit: usize,
        /// Machine-readable JSON output (hit structs)
        #[arg(long)]
        json: bool,
    },
    /// Code export: PenDocument → HTML / React+Tailwind / Tailwind-only
    Codegen {
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "html")]
        target: CodegenTargetArg,
        #[arg(long, default_value = "MyComponent")]
        component: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Undo: return to the previous PenDocument snapshot
    Undo {
        #[arg(long)]
        input: PathBuf,
    },
    /// Redo: return to the next PenDocument snapshot
    Redo {
        #[arg(long)]
        input: PathBuf,
    },
    /// Probe fusion-mlx health
    Health {
        #[arg(long, default_value = "")]
        endpoint: String,
    },
    /// Compare two PenDocuments
    Diff {
        #[arg(long)]
        old: PathBuf,
        #[arg(long)]
        new: PathBuf,
    },
    /// Emit CSS Custom Properties for the given theme mode
    Theme {
        #[arg(long, default_value = "apple-hig")]
        design_system: String,
        #[arg(long, value_enum, default_value = "light")]
        mode: ThemeModeArg,
    },
    /// Fine-tune a model on design corpus (invokes the fusion-trainer CLI as a subprocess)
    Train {
        #[arg(long)]
        dataset: PathBuf,
        #[arg(long)]
        model: String,
        /// sft (default) | grpo
        #[arg(long, default_value = "sft")]
        method: String,
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

// Callers: DesignBridge (Swift Process call), CLI direct usage.
// Affected API: Command::Export/ExportBatch now accept PenDocument JSON, Codegen subcommand.
// Data schemas: PenDocument JSON→SVG/HTML/JSON export, DesignSystem→CSS Custom Properties, PenDocument→HTML/React/Tailwind.
// User instruction: "now start implementing" — Task #16 P3-5 fd-export PNG/SVG/HTML batch export

#[derive(Clone, clap::ValueEnum)]
pub enum CodegenTargetArg {
    Html,
    ReactTailwind,
    TailwindOnly,
    SwiftUi,
}

impl From<CodegenTargetArg> for fd_codegen::CodegenTarget {
    fn from(v: CodegenTargetArg) -> Self {
        match v {
            CodegenTargetArg::Html => Self::Html,
            CodegenTargetArg::ReactTailwind => Self::ReactTailwind,
            CodegenTargetArg::TailwindOnly => Self::TailwindOnly,
            CodegenTargetArg::SwiftUi => Self::SwiftUi,
        }
    }
}

#[derive(Clone, clap::ValueEnum)]
pub enum LintRuleArg {
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
    BareColorLiteral,
    OffTokenSpacing,
    OffTokenRadius,
    MissingDarkPair,
}

impl From<LintRuleArg> for fd_design_lint::LintRule {
    fn from(v: LintRuleArg) -> Self {
        match v {
            LintRuleArg::ContrastCheck => Self::ContrastCheck,
            LintRuleArg::UnlabeledInput => Self::UnlabeledInput,
            LintRuleArg::TextEffects => Self::TextEffects,
            LintRuleArg::AbnormalRotation => Self::AbnormalRotation,
            LintRuleArg::EmptyEffects => Self::EmptyEffects,
            LintRuleArg::TokenInconsistency => Self::TokenInconsistency,
            LintRuleArg::UnnamedNode => Self::UnnamedNode,
            LintRuleArg::TextOverflow => Self::TextOverflow,
            LintRuleArg::OverlappingNodes => Self::OverlappingNodes,
            LintRuleArg::HardcodedSpacing => Self::HardcodedSpacing,
            LintRuleArg::HardcodedFontSize => Self::HardcodedFontSize,
            LintRuleArg::MissingInteractionState => Self::MissingInteractionState,
            LintRuleArg::LayoutInconsistency => Self::LayoutInconsistency,
            LintRuleArg::BareColorLiteral => Self::BareColorLiteral,
            LintRuleArg::OffTokenSpacing => Self::OffTokenSpacing,
            LintRuleArg::OffTokenRadius => Self::OffTokenRadius,
            LintRuleArg::MissingDarkPair => Self::MissingDarkPair,
        }
    }
}

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum LintProfileArg {
    Default,
    Desigma,
}

impl From<LintProfileArg> for fd_design_lint::LintProfile {
    fn from(v: LintProfileArg) -> Self {
        match v {
            LintProfileArg::Default => Self::Default,
            LintProfileArg::Desigma => Self::Desigma,
        }
    }
}

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum ExportFormatArg {
    Html,
    Svg,
    Json,
    Png,
    Pdf,
}

impl From<ExportFormatArg> for fd_export::ExportFormat {
    fn from(v: ExportFormatArg) -> Self {
        match v {
            ExportFormatArg::Html => Self::Html,
            ExportFormatArg::Svg => Self::Svg,
            ExportFormatArg::Json => Self::Json,
            ExportFormatArg::Png => Self::Png,
            ExportFormatArg::Pdf => Self::Pdf,
        }
    }
}

// issue #20: chat streaming output format enum.
// ndjson = the existing contract (issue #17), one delta/chat_done/error frame per line,
// consumed by CLI pipelines/scripts.
// sse = raw OpenAI text/event-stream (data: {...} + data: [DONE]), aligned with the
// fusion-studio DesignBridge.runFusionDesignStream stdout parser — studio's subprocess
// pipeline consumes it with zero changes.
#[derive(Clone, Debug, clap::ValueEnum)]
pub enum ChatStreamFormatArg {
    Ndjson,
    Sse,
}

#[derive(Clone, clap::ValueEnum)]
pub enum ThemeModeArg {
    Light,
    Dark,
}

impl From<ThemeModeArg> for fd_design_system::Theme {
    fn from(v: ThemeModeArg) -> Self {
        match v {
            ThemeModeArg::Light => Self::Light,
            ThemeModeArg::Dark => Self::Dark,
        }
    }
}

fn main() {
    // OPS-13: file logging + stdout dual write. The guard must be bound and held until
    // main() exits; dropping it flushes and closes the file writer. A bare call would drop
    // it immediately and lose logs.
    let _file_log_guard = logging::init_logging();

    let cli = Cli::parse();
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[fusion-design] runtime initialization failed: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = rt.block_on(run(cli)) {
        report_error(&e);
        std::process::exit(1);
    }
}

/// Classify an anyhow error into an actionable commercial-grade message instead of a raw stack.
fn report_error(e: &anyhow::Error) {
    let msg = format!("{e}");
    // E-9: prefer downcasting to explicit thiserror variants (export/canvas errors have
    // named types since A-4), then fall back to substring matching (HTTP errors come from
    // reqwest and have no named type in this project).
    let (category, hint): (String, String) = if let Some(ex) =
        e.downcast_ref::<fd_export::ExportError>()
    {
        match ex {
            fd_export::ExportError::UnsupportedFormat(_) => (
                "unsupported export format".into(),
                "check the --format argument; supported: html/svg/png/pdf/json".into(),
            ),
            fd_export::ExportError::BatchPartial { count, .. } => (
                "batch export partially failed".into(),
                format!(
                    "{} page(s) failed to export; see the failure list in the logs; exported files are kept",
                    count
                ),
            ),
            fd_export::ExportError::RenderFailed(_) => (
                "render failed".into(),
                "check that canvas size/elements are valid; PDF requires system CJK fonts".into(),
            ),
            fd_export::ExportError::Io(_) => {
                ("file IO error".into(), "check output directory permissions and disk space".into())
            }
            fd_export::ExportError::Serialize(_) => {
                ("serialization error".into(), "check document data structure integrity".into())
            }
        }
    } else if let Some(c) = e.downcast_ref::<fd_canvas_core::CanvasError>() {
        match c {
            fd_canvas_core::CanvasError::NodeNotFound(_) => (
                "node not found".into(),
                "check that the target node id exists in the current document".into(),
            ),
            fd_canvas_core::CanvasError::PageNotFound(_) => (
                "page not found".into(),
                "check that the target page id exists in the current document".into(),
            ),
            fd_canvas_core::CanvasError::ParseError(_) => (
                "document parse failed".into(),
                "check that the .fusiondesign file is valid JSON and matches the schema".into(),
            ),
            fd_canvas_core::CanvasError::DepthExceeded { .. }
            | fd_canvas_core::CanvasError::NodeTotalExceeded { .. } => (
                "document limits exceeded".into(),
                "the input .fusiondesign has nodes nested too deeply or too many nodes; check whether the file is corrupted".into(),
            ),
            fd_canvas_core::CanvasError::SchemaVersion(_) => (
                "document schema version not supported".into(),
                "the file was produced by a newer fusion-design; upgrade this program or open it with the older version".into(),
            ),
        }
    } else if msg.contains("HTTP 401") || msg.contains("Unauthorized") {
        (
            "authentication failed".into(),
            "check that FUSION_CLIENT_KEY is the gateway master_key or a fusion-mlx backend key"
                .into(),
        )
    } else if msg.contains("HTTP 404") || msg.contains("connection refused") {
        (
            "service unreachable".into(),
            "confirm fusion-mlx(11434)/gateway(11432) is running; point FUSION_MLX_BASE_URL at the correct endpoint".into(),
        )
    } else if msg.contains("HTTP 5") || msg.contains("502") {
        (
            "upstream service error".into(),
            "fusion-mlx/gateway is temporarily unavailable; check the model is loaded and retry"
                .into(),
        )
    // i18n: user-facing (match key for legacy error text, do not translate)
    } else if msg.contains("Empty choices") || msg.contains("空 choices") {
        (
            "model returned empty".into(),
            "the model produced no content; check the model name and max_tokens settings".into(),
        )
    } else {
        (
            "runtime error".into(),
            "see the logs above; set RUST_LOG=debug for more detail".into(),
        )
    };
    eprintln!("[fusion-design] failed: {category}");
    eprintln!("  reason: {msg}");
    eprintln!("  hint: {hint}");
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::ListDesignSystems => commands::design_system::list_design_systems().await,
        Command::Activate { id } => commands::design_system::activate(id).await,
        Command::Export {
            input,
            format,
            out,
            ipc_base,
        } => commands::export::run(input, format, out, ipc_base).await,
        Command::ExportBatch {
            input,
            format,
            formats,
            out,
        } => commands::io_cmd::export_batch(input, format, formats, out).await,
        Command::Generate {
            prompt,
            page,
            model,
            endpoint,
            out,
            ipc_base,
            stream,
        } => commands::generate::run(prompt, page, model, endpoint, out, ipc_base, stream).await,
        Command::Chat {
            model,
            endpoint,
            system_prompt,
            system_prompt_file,
            messages_file,
            rag_context_file,
            max_tokens,
            stream,
            json,
            format,
        } => {
            commands::chat::run(
                model,
                endpoint,
                system_prompt,
                system_prompt_file,
                messages_file,
                rag_context_file,
                max_tokens,
                stream,
                json,
                format,
            )
            .await
        }
        Command::ImageToUi {
            sketch,
            hint,
            page,
            model,
            endpoint,
            out,
        } => commands::ai_skill::image_to_ui(sketch, hint, page, model, endpoint, out).await,
        Command::MultiVariants {
            prompt,
            page,
            styles,
            model,
            endpoint,
            out,
        } => commands::ai_skill::multi_variants(prompt, page, styles, model, endpoint, out).await,
        Command::SpecDoc {
            input,
            title,
            model,
            endpoint,
            out,
        } => commands::ai_skill::spec_doc(input, title, model, endpoint, out).await,
        Command::PageFlow {
            flow,
            style_hint,
            model,
            endpoint,
            out,
        } => commands::ai_skill::page_flow(flow, style_hint, model, endpoint, out).await,
        Command::CheckFrontend { dir, backend } => {
            commands::io_cmd::check_frontend(dir, backend).await
        }
        Command::CheckMlx { endpoint, model } => commands::check_mlx::run(endpoint, model).await,
        Command::ParseHtml { input, page } => commands::io_cmd::parse_html(input, page).await,
        Command::TokenCSS { design_system } => {
            commands::design_system::token_css(design_system).await
        }
        Command::Lint {
            input,
            design_system,
            profile,
            rules,
            fix,
            dry_run,
            gate,
        } => commands::lint::run(input, design_system, profile, rules, fix, dry_run, gate),
        Command::Search {
            kind,
            query,
            limit,
            json,
        } => commands::search::run(kind, query, limit, json),
        Command::Codegen {
            input,
            target,
            component,
            out,
        } => commands::io_cmd::codegen(input, target, component, out).await,
        Command::Undo { input } => commands::history::undo(input).await,
        Command::Redo { input } => commands::history::redo(input).await,
        Command::Health { endpoint } => commands::health::run(endpoint).await,
        Command::Diff { old, new } => commands::io_cmd::diff(old, new).await,
        Command::Theme {
            design_system,
            mode,
        } => commands::design_system::theme(design_system, mode).await,
        Command::Train {
            dataset,
            model,
            method,
            config,
        } => commands::ai_skill::train(dataset, model, method, config).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{
        ndjson_frame_delta, ndjson_frame_done, ndjson_frame_error, pick_multi_variant_styles,
    };
    use clap::Parser;

    #[test]
    fn parse_image_to_ui_defaults() {
        let cli = Cli::parse_from([
            "fusion-design",
            "image-to-ui",
            "--sketch",
            "/tmp/sketch.png",
        ]);
        match cli.command {
            Command::ImageToUi {
                sketch,
                hint,
                page,
                model,
                endpoint,
                out,
            } => {
                assert_eq!(sketch, PathBuf::from("/tmp/sketch.png"));
                assert_eq!(hint, "");
                assert_eq!(page, "Home");
                assert_eq!(model, "Qwen3.5-9B-4bit");
                // --endpoint defaults to an empty string; resolve_endpoint resolves it at
                // runtime (env / default fallback)
                assert_eq!(endpoint, "");
                assert!(out.is_none());
            }
            _ => panic!("expected ImageToUi"),
        }
    }

    #[test]
    fn parse_multi_variants_custom_styles() {
        let cli = Cli::parse_from([
            "fusion-design",
            "multi-variants",
            "--prompt",
            "login page",
            "--styles",
            "neon,glass,flat",
        ]);
        match cli.command {
            Command::MultiVariants { prompt, styles, .. } => {
                assert_eq!(prompt, "login page");
                let styles = styles.expect("styles should be provided");
                assert_eq!(styles, vec!["neon", "glass", "flat"]);
            }
            _ => panic!("expected MultiVariants"),
        }
    }

    #[test]
    fn pick_multi_variant_styles_preserves_full_user_input() {
        // E-27/P3: >=3 user styles preserved in full, no padding.
        let picked =
            pick_multi_variant_styles(Some(vec!["neon".into(), "glass".into(), "flat".into()]));
        assert_eq!(picked, ["neon", "glass", "flat"]);
    }

    #[test]
    fn pick_multi_variant_styles_fills_under3_with_defaults() {
        // E-27/P3: 1 user style must be kept; pad to 3 with defaults; never drop user input.
        let picked = pick_multi_variant_styles(Some(vec!["neon".into()]));
        assert_eq!(picked[0], "neon", "user-provided style must be kept");
        assert_eq!(picked.len(), 3);
        // Padded entries must come from the defaults, without duplicating neon
        let defaults = ["极简风", "卡片风", "深色风"];
        assert!(
            picked[1..].iter().all(|s| defaults.contains(&s.as_str())),
            "padded entries must be defaults: {picked:?}"
        );
    }

    #[test]
    fn pick_multi_variant_styles_none_uses_all_defaults() {
        // E-27/P3: no styles provided → all three defaults.
        let picked = pick_multi_variant_styles(None);
        assert_eq!(picked, ["极简风", "卡片风", "深色风"]);
    }

    #[test]
    fn pick_multi_variant_styles_empty_uses_all_defaults() {
        // E-27/P3: empty styles array counts as not provided → all defaults.
        let picked = pick_multi_variant_styles(Some(vec![]));
        assert_eq!(picked, ["极简风", "卡片风", "深色风"]);
    }

    #[test]
    fn parse_spec_doc_defaults() {
        let cli = Cli::parse_from(["fusion-design", "spec-doc", "--input", "doc.json"]);
        match cli.command {
            Command::SpecDoc {
                input,
                title,
                endpoint,
                ..
            } => {
                assert_eq!(input, PathBuf::from("doc.json"));
                // i18n: user-facing (matches the default_value literal)
                assert_eq!(title, "设计规范文档");
                assert_eq!(endpoint, "");
            }
            _ => panic!("expected SpecDoc"),
        }
    }

    #[test]
    fn parse_page_flow_defaults() {
        // i18n: user-facing (CJK input fixture)
        let cli = Cli::parse_from(["fusion-design", "page-flow", "--flow", "登录→首页→设置"]);
        match cli.command {
            Command::PageFlow {
                flow, style_hint, ..
            } => {
                assert_eq!(flow, "登录→首页→设置");
                assert_eq!(style_hint, "");
            }
            _ => panic!("expected PageFlow"),
        }
    }

    #[test]
    fn parse_generate_unchanged() {
        let cli = Cli::parse_from(["fusion-design", "generate", "--prompt", "dashboard"]);
        match cli.command {
            Command::Generate { prompt, page, .. } => {
                assert_eq!(prompt, "dashboard");
                assert_eq!(page, "Home");
            }
            _ => panic!("expected Generate"),
        }
    }

    #[test]
    fn parse_chat_defaults() {
        let cli = Cli::parse_from([
            "fusion-design",
            "chat",
            "--model",
            "mlx-community--Qwen3.5-4B-4bit",
            "--messages-file",
            "/tmp/m.json",
        ]);
        match cli.command {
            Command::Chat {
                model,
                messages_file,
                max_tokens,
                stream,
                json,
                format,
                ..
            } => {
                assert_eq!(model, "mlx-community--Qwen3.5-4B-4bit");
                assert_eq!(messages_file, Some(PathBuf::from("/tmp/m.json")));
                assert_eq!(max_tokens, 4096);
                assert!(stream, "stream defaults to true");
                assert!(json, "json defaults to true");
                // issue #20: format defaults to ndjson
                assert!(
                    matches!(format, ChatStreamFormatArg::Ndjson),
                    "format defaults to ndjson"
                );
            }
            _ => panic!("expected Chat"),
        }
    }

    // issue #20: --format sse parses explicitly (raw OpenAI text/event-stream, aligned with studio)
    #[test]
    fn parse_chat_format_sse() {
        let cli = Cli::parse_from([
            "fusion-design",
            "chat",
            "--model",
            "mlx-community--Qwen3.5-4B-4bit",
            "--messages-file",
            "/tmp/m.json",
            "--format",
            "sse",
        ]);
        match cli.command {
            Command::Chat { format, .. } => {
                assert!(
                    matches!(format, ChatStreamFormatArg::Sse),
                    "--format sse must parse as Sse"
                );
            }
            _ => panic!("expected Chat"),
        }
    }

    // H-A16 regression: the NDJSON three-frame schema is this subcommand's self-consistency
    // contract (delta/chat_done/error).
    // Note: studio goes through gateway TCP using chat_event, not delta; this test guards
    // the CLI pipeline contract, not alignment with studio's schema. Any drift in a type
    // string breaks CLI consumers.
    #[test]
    fn ndjson_delta_frame_contract() {
        // i18n: user-facing (multibyte token fixture)
        let frame = ndjson_frame_delta("你");
        assert_eq!(frame["type"], "delta", "delta frame type must be delta");
        assert_eq!(
            frame["token"], "你",
            "delta frame token passes through verbatim"
        );
    }

    #[test]
    fn ndjson_done_frame_is_chat_done_not_done() {
        let frame = ndjson_frame_done();
        assert_eq!(
            frame["type"], "chat_done",
            "done frame type must be chat_done; regressing to done breaks the CLI pipeline contract"
        );
        assert_eq!(frame["finish_reason"], "stop");
    }

    #[test]
    fn ndjson_error_frame_contract() {
        // i18n: user-facing (error message payload fixture)
        let frame = ndjson_frame_error("连接失败");
        assert_eq!(frame["type"], "error");
        assert_eq!(frame["message"], "连接失败");
    }
}
