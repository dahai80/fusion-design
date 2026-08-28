//! Fusion-Design CLI — 命令行批量生成/导出。
//!
//! 对应 PRD 模块 6「Fusion CLI 联动」扩展。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod commands;
mod common;

#[derive(Parser)]
#[command(
    name = "fusion-design",
    version,
    about = "Fusion-Design 本地 AI 设计工作台 CLI"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// 列出已注册的设计规范
    ListDesignSystems,
    /// 激活一套设计规范
    Activate { id: String },
    /// 导出画布页面到指定格式
    Export {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, value_enum)]
        format: ExportFormatArg,
        #[arg(long)]
        out: PathBuf,
        /// 通过 fd-ecosystem IPC 发送导出结果
        #[arg(long)]
        ipc_base: Option<PathBuf>,
    },
    /// 批量导出多页（输入 JSON 数组）
    ExportBatch {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, value_enum)]
        format: ExportFormatArg,
        /// 多格式组合导出，逗号分隔（如 html,svg,json），优先于 --format
        #[arg(long, value_delimiter = ',')]
        formats: Option<Vec<ExportFormatArg>>,
        #[arg(long)]
        out: PathBuf,
    },
    /// 文生 UI：自然语言描述 → PenDocument JSON
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
        /// 通过 fd-ecosystem IPC 发送结果（而非直接输出）
        #[arg(long)]
        ipc_base: Option<PathBuf>,
        /// 流式输出 SSE token（供 GUI 管道读取）
        #[arg(long)]
        stream: bool,
    },
    /// 机器可读流式 chat：CLI/脚本管道用的流式 NDJSON 推理接口。
    /// 鉴权 / X-Fusion-Route header / endpoint 解析复用 fd-ai-adapter，调用方不重实现。
    ///
    /// 消费方声明（issue #17 诚实回溯）：issue #17 设想此子命令为 fusion-studio
    /// subprocess 入口取代直连 MLX，但经核实 studio 实际走 fusion-gateway TCP
    /// NDJSON（StreamingBridge.swift，帧 schema 为 chat_event/chat_done/error +
    /// session_id/event），**不经 fd-cli chat**。故本子命令当前无 studio 消费方，
    /// 供 CLI 管道/脚本/测试消费。NDJSON 帧 schema（delta/chat_done/error）为
    /// 本子命令自洽契约，非对齐 studio（studio 用 chat_event 非 delta）。
    ///
    /// 流式与 gateway：默认 endpoint 经 fusion-gateway(11432)。gateway 流式转发
    /// 502 bug（fusion-gateway#108：stream=true 连接拒绝）已于 2026-08-25 修复（PR #111
    /// local-first ordering），真流式探针通过（SSE delta 正常产出）。若遇上游回退，
    /// 可 `FUSION_MLX_BASE_URL=http://127.0.0.1:11434` 直连 MLX 绕过 gateway。
    /// 默认 model `Qwen3.5-9B-4bit` 为内置 MLX 常用文本模型（真推理验证通过），
    /// 但 MLX 部署模型列表随环境变，建议显式传 `--model` 本地已加载模型 id
    /// （可用 `check-mlx --endpoint ...` 探测真可用性）以跨部署稳健。
    Chat {
        #[arg(long, default_value = "Qwen3.5-9B-4bit")]
        model: String,
        #[arg(long, default_value = "")]
        endpoint: String,
        /// 内联 system prompt（与 --system-prompt-file 互斥，后者优先）
        #[arg(long, default_value = "")]
        system_prompt: String,
        #[arg(long)]
        system_prompt_file: Option<PathBuf>,
        /// JSON 多轮历史：[{"role":"user|assistant|system","content":".."}]
        #[arg(long)]
        messages_file: Option<PathBuf>,
        /// RAG 上下文，注入到 system prompt 尾部
        #[arg(long)]
        rag_context_file: Option<PathBuf>,
        #[arg(long, default_value = "4096")]
        max_tokens: u32,
        /// 流式 NDJSON 输出（每行一帧 delta/done/error），默认开启
        #[arg(long, default_value = "true")]
        stream: bool,
        /// 输出 NDJSON 成帧（当前唯一格式，保留参数供未来纯文本模式）
        #[arg(long, default_value = "true")]
        json: bool,
    },
    /// 图生 UI：草图/参考图 → PenDocument JSON
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
    /// 多方案对比：一次生成 3 套不同风格设计稿
    MultiVariants {
        #[arg(long)]
        prompt: String,
        #[arg(long, default_value = "Home")]
        page: String,
        /// 三套风格，逗号分隔（缺省用默认三风格）
        #[arg(long, value_delimiter = ',')]
        styles: Option<Vec<String>>,
        #[arg(long, default_value = "Qwen3.5-9B-4bit")]
        model: String,
        #[arg(long, default_value = "")]
        endpoint: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// SpecDoc：AI 自动生成设计规范文档
    SpecDoc {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "设计规范文档")]
        title: String,
        #[arg(long, default_value = "Qwen3.5-9B-4bit")]
        model: String,
        #[arg(long, default_value = "")]
        endpoint: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// PageFlow：按流程描述批量生成多页面
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
    /// 校验前端静态资源目录
    CheckFrontend {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value = "")]
        backend: String,
    },
    /// 校验 fusion-mlx endpoint 是否为 localhost
    CheckMlx {
        #[arg(long, default_value = "")]
        endpoint: String,
        /// 指定推理探针所用模型 id；空则按 FUSION_MLX_MODEL 环境变量，
        /// 再缺省回退到 /v1/models 列表首个（gateway 混列云端/本地模型，
        /// 首个可能未加载，建议显式传本地 mlx 模型 id 以获真可用性判定）。
        #[arg(long, default_value = "")]
        model: String,
    },
    /// HTML → PenDocument JSON 转换
    ParseHtml {
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        page: Option<String>,
    },
    /// 输出当前激活设计规范的 CSS Custom Properties
    TokenCSS {
        #[arg(long, default_value = "apple-hig")]
        design_system: String,
    },
    /// 设计规范检测：PenDocument → LintResult
    Lint {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "apple-hig")]
        design_system: Option<String>,
        #[arg(long, value_enum)]
        rules: Option<Vec<LintRuleArg>>,
        /// 自动修复可修复的违规（Token 引用、空值清理、自动命名）
        #[arg(long)]
        fix: bool,
        /// 仅预览修复，不写入文件
        #[arg(long, requires = "fix")]
        dry_run: bool,
    },
    /// 代码导出：PenDocument → HTML / React+Tailwind / Tailwind-only
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
    /// 撤销：返回上一步 PenDocument 快照
    Undo {
        #[arg(long)]
        input: PathBuf,
    },
    /// 重做：返回下一步 PenDocument 快照
    Redo {
        #[arg(long)]
        input: PathBuf,
    },
    /// 探测 fusion-mlx 健康状态
    Health {
        #[arg(long, default_value = "")]
        endpoint: String,
    },
    /// 比较两个 PenDocument 的差异
    Diff {
        #[arg(long)]
        old: PathBuf,
        #[arg(long)]
        new: PathBuf,
    },
    /// 输出指定主题模式的 CSS Custom Properties
    Theme {
        #[arg(long, default_value = "apple-hig")]
        design_system: String,
        #[arg(long, value_enum, default_value = "light")]
        mode: ThemeModeArg,
    },
    /// 基于设计语料微调模型（子进程调用 fusion-trainer CLI）
    Train {
        #[arg(long)]
        dataset: PathBuf,
        #[arg(long)]
        model: String,
        /// sft（默认）| grpo
        #[arg(long, default_value = "sft")]
        method: String,
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

// Callers: DesignBridge (Swift Process call), CLI direct usage.
// Affected API: Command::Export/ExportBatch now accept PenDocument JSON, Codegen subcommand.
// Data schemas: PenDocument JSON→SVG/HTML/JSON export, DesignSystem→CSS Custom Properties, PenDocument→HTML/React/Tailwind.
// User instruction: "现在开始实施" — Task #16 P3-5 fd-export PNG/SVG/HTML 批量导出

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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[fusion-design] 运行时初始化失败: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = rt.block_on(run(cli)) {
        report_error(&e);
        std::process::exit(1);
    }
}

/// 把 anyhow 错误分类为可操作的商用级提示，而非裸栈。
fn report_error(e: &anyhow::Error) {
    let msg = format!("{e}");
    // E-9：优先按 thiserror 显式变体 downcast（A-4 落地后导出/画布错误有具名类型），
    // 再退化到子串匹配（HTTP 错误来自 reqwest，无本项目具名类型）。
    let (category, hint): (String, String) = if let Some(ex) =
        e.downcast_ref::<fd_export::ExportError>()
    {
        match ex {
            fd_export::ExportError::UnsupportedFormat(_) => (
                "导出格式不支持".into(),
                "检查 --format 参数，支持 html/svg/png/pdf/json".into(),
            ),
            fd_export::ExportError::BatchPartial { count, .. } => (
                "批量导出部分失败".into(),
                format!(
                    "{} 个页面导出失败，详见日志中失败清单；已导出文件保留",
                    count
                ),
            ),
            fd_export::ExportError::RenderFailed(_) => (
                "渲染失败".into(),
                "检查画布尺寸/元素是否合法；PDF 需系统 CJK 字体".into(),
            ),
            fd_export::ExportError::Io(_) => {
                ("文件 IO 错误".into(), "检查输出目录权限与磁盘空间".into())
            }
            fd_export::ExportError::Serialize(_) => {
                ("序列化错误".into(), "检查文档数据结构完整性".into())
            }
        }
    } else if let Some(c) = e.downcast_ref::<fd_canvas_core::CanvasError>() {
        match c {
            fd_canvas_core::CanvasError::NodeNotFound(_) => (
                "节点未找到".into(),
                "检查操作的目标 node id 是否存在于当前文档".into(),
            ),
            fd_canvas_core::CanvasError::PageNotFound(_) => (
                "页面未找到".into(),
                "检查操作的目标 page id 是否存在于当前文档".into(),
            ),
            fd_canvas_core::CanvasError::ParseError(_) => (
                "文档解析失败".into(),
                "检查 .fusiondesign 文件是否为合法 JSON 且符合 schema".into(),
            ),
            fd_canvas_core::CanvasError::DepthExceeded { .. }
            | fd_canvas_core::CanvasError::NodeTotalExceeded { .. } => (
                "文档超限".into(),
                "输入 .fusiondesign 节点嵌套过深或过多，检查文件是否损坏".into(),
            ),
            fd_canvas_core::CanvasError::SchemaVersion(_) => (
                "文档 schema 版本不支持".into(),
                "文件由更高版本 fusion-design 生成，请升级本程序或用旧版打开".into(),
            ),
        }
    } else if msg.contains("HTTP 401") || msg.contains("Unauthorized") {
        (
            "鉴权失败".into(),
            "检查 FUSION_MLX_API_KEY 是否为 gateway master_key 或 fusion-mlx backend key".into(),
        )
    } else if msg.contains("HTTP 404") || msg.contains("connection refused") {
        (
            "服务不可达".into(),
            "确认 fusion-mlx(11434)/gateway(11432) 已启动；FUSION_MLX_BASE_URL 指向正确端点".into(),
        )
    } else if msg.contains("HTTP 5") || msg.contains("502") {
        (
            "上游服务错误".into(),
            "fusion-mlx/gateway 临时不可用，检查模型是否已加载后重试".into(),
        )
    } else if msg.contains("Empty choices") || msg.contains("空 choices") {
        (
            "模型返回空".into(),
            "模型未产出内容，检查模型名与 max_tokens 设置".into(),
        )
    } else {
        (
            "运行错误".into(),
            "详见上方日志；可设 RUST_LOG=debug 获取更多细节".into(),
        )
    };
    eprintln!("[fusion-design] 失败：{category}");
    eprintln!("  原因：{msg}");
    eprintln!("  建议：{hint}");
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    use fd_cli::{export, host};

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
        } => {
            // TODO A-3: 拆到 src/commands/
            let json = common::read_file_capped(&input)?;
            let doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;
            let reg = common::build_registry(&doc);
            let fmt_list: Vec<fd_export::ExportFormat> = if let Some(ref fmts) = formats {
                fmts.iter().map(|f| f.clone().into()).collect()
            } else {
                vec![format.into()]
            };
            let mut total = 0;
            // E-12/E-24：批量导出旧实现首个格式失败即 `?` 传播，其余格式静默跳过。
            // 改为收集所有失败，末尾汇总打印 + 若有失败退出非零（fail visibly，不阻断已成功项）。
            let mut failures: Vec<String> = Vec::new();
            for fmt in fmt_list {
                match export::Exporter::from_pen_document_with_tokens(&doc, fmt, &out, &reg) {
                    Ok(files) => {
                        tracing::info!(format = ?fmt, count = files.len(), "批量导出完成");
                        total += files.len();
                    }
                    Err(e) => {
                        let fmt_str = fmt.extension();
                        tracing::warn!(format = fmt_str, error = %e, "批量导出该格式失败");
                        failures.push(format!("{fmt_str}: {e}"));
                    }
                }
            }
            println!("已批量导出 {} 个页面到 {out:?}", total);
            if !failures.is_empty() {
                eprintln!("批量导出部分失败 ({} 项):", failures.len());
                for f in &failures {
                    eprintln!("  - {f}");
                }
                return Err(anyhow::anyhow!(
                    "批量导出部分失败: 成功导出 {total} 页，{} 项格式失败",
                    failures.len()
                ));
            }
            Ok(())
        }
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
        } => {
            // TODO A-3: 拆到 src/commands/
            let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
                fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
            )?;
            let skills = fd_ai_adapter::DesignSkills::new(client, model);
            let doc = skills
                .image_to_ui_async(&sketch.to_string_lossy(), &hint, &page)
                .await?;
            let json = serde_json::to_string_pretty(&doc)?;
            match out {
                Some(p) => {
                    tokio::fs::write(&p, &json).await?;
                    println!("已生成图生 UI PenDocument JSON 到 {p:?}");
                }
                None => println!("{json}"),
            }
            Ok(())
        }
        Command::MultiVariants {
            prompt,
            page,
            styles,
            model,
            endpoint,
            out,
        } => {
            // TODO A-3: 拆到 src/commands/
            let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
                fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
            )?;
            let skills = fd_ai_adapter::DesignSkills::new(client, model);
            let picked = common::pick_multi_variant_styles(styles);
            let docs = skills
                .multi_variants_async(
                    &prompt,
                    &page,
                    [picked[0].as_str(), picked[1].as_str(), picked[2].as_str()],
                )
                .await?;
            let json = serde_json::to_string_pretty(&docs)?;
            match out {
                Some(p) => {
                    tokio::fs::write(&p, &json).await?;
                    println!("已生成 3 套多方案 PenDocument JSON 到 {p:?}");
                }
                None => println!("{json}"),
            }
            Ok(())
        }
        Command::SpecDoc {
            input,
            title,
            model,
            endpoint,
            out,
        } => {
            // TODO A-3: 拆到 src/commands/
            let doc_json = common::read_file_capped(&input)?;
            let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
                fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
            )?;
            let skills = fd_ai_adapter::DesignSkills::new(client, model);
            let spec = skills.spec_doc_async(&doc_json, &title).await?;
            let json = serde_json::to_string_pretty(&spec)?;
            match out {
                Some(p) => {
                    tokio::fs::write(&p, &json).await?;
                    println!("已生成设计规范文档到 {p:?}");
                }
                None => println!("{json}"),
            }
            Ok(())
        }
        Command::PageFlow {
            flow,
            style_hint,
            model,
            endpoint,
            out,
        } => {
            // TODO A-3: 拆到 src/commands/
            let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
                fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
            )?;
            let skills = fd_ai_adapter::DesignSkills::new(client, model);
            let docs = skills.page_flow_async(&flow, &style_hint).await?;
            let json = serde_json::to_string_pretty(&docs)?;
            match out {
                Some(p) => {
                    tokio::fs::write(&p, &json).await?;
                    println!("已生成 PageFlow 多页面文档到 {p:?}");
                }
                None => println!("{json}"),
            }
            Ok(())
        }
        Command::CheckFrontend { dir, backend } => {
            // TODO A-3: 拆到 src/commands/
            host::HostBridgeConfig {
                frontend_dir: dir,
                backend_endpoint: backend,
                block_external: true,
            }
            .validate()?;
            println!("前端目录校验通过");
            Ok(())
        }
        Command::CheckMlx { endpoint, model } => commands::check_mlx::run(endpoint, model).await,
        Command::ParseHtml { input, page } => {
            // TODO A-3: 拆到 src/commands/
            let html = match input {
                Some(p) => common::read_file_capped(&p)?,
                None => common::read_stdin_capped()?,
            };
            let page_name = page.as_deref().unwrap_or("Page");
            let doc = fd_ai_adapter::html_to_pen_document(&html, page_name)?;
            let json = serde_json::to_string_pretty(&doc)?;
            println!("{json}");
            Ok(())
        }
        Command::TokenCSS { design_system } => {
            commands::design_system::token_css(design_system).await
        }
        Command::Lint {
            input,
            design_system,
            rules,
            fix,
            dry_run,
        } => commands::lint::run(input, design_system, rules, fix, dry_run),
        Command::Codegen {
            input,
            target,
            component,
            out,
        } => {
            // TODO A-3: 拆到 src/commands/
            let json = match input {
                Some(p) => common::read_file_capped(&p)?,
                None => common::read_stdin_capped()?,
            };
            let doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;
            // H-A11：codegen 前解析 token 引用（token:color.accent → 实际 hex）。
            // 旧实现直接把 PenDocument 传 codegen，token:xxx 原样输出——浏览器不认，
            // SwiftUI 侧 DesignTokens.xxx 未定义编译失败。按文档声明的
            // active_design_system 构建注册表并解析，递归覆盖嵌套 children。
            let reg = common::build_registry(&doc);
            let doc = fd_codegen::resolve_tokens(&doc, &reg);
            let code = match target {
                CodegenTargetArg::Html => {
                    use fd_codegen::Codegen;
                    fd_codegen::HtmlCodegen.generate(&doc)
                }
                CodegenTargetArg::ReactTailwind => {
                    use fd_codegen::Codegen;
                    fd_codegen::ReactTailwindCodegen {
                        component_name: component.clone(),
                    }
                    .generate(&doc)
                }
                CodegenTargetArg::TailwindOnly => {
                    use fd_codegen::Codegen;
                    fd_codegen::TailwindOnlyCodegen.generate(&doc)
                }
                CodegenTargetArg::SwiftUi => {
                    use fd_codegen::Codegen;
                    fd_codegen::SwiftUiCodegen {
                        view_name: component.clone(),
                    }
                    .generate(&doc)
                }
            };
            match out {
                Some(p) => {
                    tokio::fs::write(&p, &code).await?;
                    tracing::info!("已导出代码到 {p:?}");
                }
                None => println!("{code}"),
            }
            Ok(())
        }
        Command::Undo { input } => {
            // TODO A-3: 拆到 src/commands/
            let history_path = input.with_extension("history.json");
            if !history_path.exists() {
                anyhow::bail!("历史文件不存在: {history_path:?}");
            }
            let hist_json = common::read_file_capped(&history_path)?;
            let mut stack: fd_canvas_core::UndoRedoStack = serde_json::from_str(&hist_json)
                .map_err(|e| anyhow::anyhow!("历史文件解析失败: {e}"))?;
            match stack.undo() {
                Some(doc) => {
                    let out_json = serde_json::to_string_pretty(&doc)?;
                    let hist_out = serde_json::to_string_pretty(&stack)?;
                    tokio::fs::write(&history_path, &hist_out).await?;
                    println!("{out_json}");
                    tracing::info!("undo: 成功回退");
                    Ok(())
                }
                None => anyhow::bail!("无法撤销：已到最早状态"),
            }
        }
        Command::Redo { input } => {
            // TODO A-3: 拆到 src/commands/
            let history_path = input.with_extension("history.json");
            if !history_path.exists() {
                anyhow::bail!("历史文件不存在: {history_path:?}");
            }
            let hist_json = common::read_file_capped(&history_path)?;
            let mut stack: fd_canvas_core::UndoRedoStack = serde_json::from_str(&hist_json)
                .map_err(|e| anyhow::anyhow!("历史文件解析失败: {e}"))?;
            match stack.redo() {
                Some(doc) => {
                    let out_json = serde_json::to_string_pretty(&doc)?;
                    let hist_out = serde_json::to_string_pretty(&stack)?;
                    tokio::fs::write(&history_path, &hist_out).await?;
                    println!("{out_json}");
                    tracing::info!("redo: 成功重做");
                    Ok(())
                }
                None => anyhow::bail!("无法重做：已到最新状态"),
            }
        }
        Command::Health { endpoint } => commands::health::run(endpoint).await,
        Command::Diff { old, new } => {
            // TODO A-3: 拆到 src/commands/
            let old_json = common::read_file_capped(&old)?;
            let new_json = common::read_file_capped(&new)?;
            let old_doc: fd_canvas_core::PenDocument = serde_json::from_str(&old_json)?;
            let new_doc: fd_canvas_core::PenDocument = serde_json::from_str(&new_json)?;
            let diff = old_doc.diff(&new_doc);
            let output = serde_json::to_string_pretty(&diff)?;
            println!("{output}");
            Ok(())
        }
        Command::Theme {
            design_system,
            mode,
        } => commands::design_system::theme(design_system, mode).await,
        Command::Train {
            dataset,
            model,
            method,
            config,
        } => {
            // TODO A-3: 拆到 src/commands/
            let trainer = fd_ecosystem::TrainerClient::new();
            let status = match method.as_str() {
                "grpo" => trainer.run_rlsl("grpo", &dataset, &model, config.as_deref())?,
                "sft" => trainer.run_sft(&dataset, &model, config.as_deref())?,
                other => anyhow::bail!("不支持的 --method: {other} (仅 sft|grpo)"),
            };
            if !status.success() {
                anyhow::bail!("fusion-trainer 退出码 {}", status.code().unwrap_or(-1));
            }
            Ok(())
        }
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
                // --endpoint 默认空串，运行时由 resolve_endpoint 解析（env/缺省回退）
                assert_eq!(endpoint, "");
                assert!(out.is_none());
            }
            _ => panic!("应为 ImageToUi"),
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
                let styles = styles.expect("应提供 styles");
                assert_eq!(styles, vec!["neon", "glass", "flat"]);
            }
            _ => panic!("应为 MultiVariants"),
        }
    }

    #[test]
    fn pick_multi_variant_styles_preserves_full_user_input() {
        // E-27/P3：>=3 用户 styles 全量保留，不补齐。
        let picked =
            pick_multi_variant_styles(Some(vec!["neon".into(), "glass".into(), "flat".into()]));
        assert_eq!(picked, ["neon", "glass", "flat"]);
    }

    #[test]
    fn pick_multi_variant_styles_fills_under3_with_defaults() {
        // E-27/P3：1 个用户 style 须保留，不足用默认补齐至 3，不得丢弃用户输入。
        let picked = pick_multi_variant_styles(Some(vec!["neon".into()]));
        assert_eq!(picked[0], "neon", "用户提供的 style 须保留");
        assert_eq!(picked.len(), 3);
        // 补齐项须来自默认（极简风/卡片风/深色风），不重复 neon
        let defaults = ["极简风", "卡片风", "深色风"];
        assert!(
            picked[1..].iter().all(|s| defaults.contains(&s.as_str())),
            "补齐项须为默认: {picked:?}"
        );
    }

    #[test]
    fn pick_multi_variant_styles_none_uses_all_defaults() {
        // E-27/P3：未提供 styles 用全部默认三种。
        let picked = pick_multi_variant_styles(None);
        assert_eq!(picked, ["极简风", "卡片风", "深色风"]);
    }

    #[test]
    fn pick_multi_variant_styles_empty_uses_all_defaults() {
        // E-27/P3：空 styles 数组视为未提供，用全部默认。
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
                assert_eq!(title, "设计规范文档");
                assert_eq!(endpoint, "");
            }
            _ => panic!("应为 SpecDoc"),
        }
    }

    #[test]
    fn parse_page_flow_defaults() {
        let cli = Cli::parse_from(["fusion-design", "page-flow", "--flow", "登录→首页→设置"]);
        match cli.command {
            Command::PageFlow {
                flow, style_hint, ..
            } => {
                assert_eq!(flow, "登录→首页→设置");
                assert_eq!(style_hint, "");
            }
            _ => panic!("应为 PageFlow"),
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
            _ => panic!("应为 Generate"),
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
                ..
            } => {
                assert_eq!(model, "mlx-community--Qwen3.5-4B-4bit");
                assert_eq!(messages_file, Some(PathBuf::from("/tmp/m.json")));
                assert_eq!(max_tokens, 4096);
                assert!(stream, "stream 默认 true");
                assert!(json, "json 默认 true");
            }
            _ => panic!("应为 Chat"),
        }
    }

    // H-A16 回归：NDJSON 三帧 schema 为本子命令自洽契约（delta/chat_done/error）。
    // 注：studio 走 gateway TCP 用 chat_event 非 delta，此测试守护 CLI 管道契约，
    // 非对齐 studio schema。任一 type 字符串漂移即破坏 CLI 消费方。
    #[test]
    fn ndjson_delta_frame_contract() {
        let frame = ndjson_frame_delta("你");
        assert_eq!(frame["type"], "delta", "delta 帧 type 必须为 delta");
        assert_eq!(frame["token"], "你", "delta 帧 token 透传原文");
    }

    #[test]
    fn ndjson_done_frame_is_chat_done_not_done() {
        let frame = ndjson_frame_done();
        assert_eq!(
            frame["type"], "chat_done",
            "done 帧 type 必须为 chat_done，回退 done 即破坏 CLI 管道契约"
        );
        assert_eq!(frame["finish_reason"], "stop");
    }

    #[test]
    fn ndjson_error_frame_contract() {
        let frame = ndjson_frame_error("连接失败");
        assert_eq!(frame["type"], "error");
        assert_eq!(frame["message"], "连接失败");
    }
}
