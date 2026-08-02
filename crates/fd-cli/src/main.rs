//! Fusion-Design CLI — 命令行批量生成/导出。
//!
//! 对应 PRD 模块 6「Fusion CLI 联动」扩展。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "fusion-design", version, about = "Fusion-Design 本地 AI 设计工作台 CLI")]
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
        #[arg(long, default_value = "qwen3.5")]
        model: String,
        #[arg(long, default_value = "http://127.0.0.1:11434")]
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
    /// 校验前端静态资源目录
    CheckFrontend {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:11434")]
        backend: String,
    },
    /// 校验 fusion-mlx endpoint 是否为 localhost
    CheckMlx {
        #[arg(long, default_value = "http://127.0.0.1:11434")]
        endpoint: String,
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
        #[arg(long, default_value = "http://127.0.0.1:11434")]
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

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
    ).init();

    let cli = Cli::parse();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run(cli))
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    use fd_cli::{design, export, host};

    match cli.command {
        Command::ListDesignSystems => {
            let mut reg = design::DesignSystemRegistry::new();
            reg.register_builtin();
            for id in reg.list() {
                println!("{id}");
            }
            Ok(())
        }
        Command::Activate { id } => {
            let mut reg = design::DesignSystemRegistry::new();
            reg.register_builtin();
            reg.activate(&id)?;
            println!("已激活: {id}");
            Ok(())
        }
        Command::Export { input, format, out, ipc_base } => {
            let json = std::fs::read_to_string(&input)?;
            let doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;
            let format_str = format!("{:?}", format);
            let files = export::Exporter::from_pen_document(&doc, format.into(), &out)?;
            println!("已导出 {} 个页面到 {out:?}", files.len());
            if let Some(base) = ipc_base {
                let link = fd_ecosystem::EcosystemLink::new(&base);
                let msg = fd_ecosystem::LinkMessage {
                    target: fd_ecosystem::EcosystemTarget::FusionCLI,
                    action: "export-done".into(),
                    payload: serde_json::json!({
                        "files": files.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
                        "format": format_str,
                    }),
                };
                link.send(&msg)?;
                tracing::info!("export: IPC 消息已发送");
            }
            Ok(())
        }
        Command::ExportBatch { input, format, formats, out } => {
            let json = std::fs::read_to_string(&input)?;
            let doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;
            let fmt_list: Vec<fd_export::ExportFormat> = if let Some(ref fmts) = formats {
                fmts.iter().map(|f| f.clone().into()).collect()
            } else {
                vec![format.into()]
            };
            let mut total = 0;
            for fmt in fmt_list {
                let files = export::Exporter::from_pen_document(&doc, fmt, &out)?;
                tracing::info!(format = ?fmt, count = files.len(), "批量导出完成");
                total += files.len();
            }
            println!("已批量导出 {} 个页面到 {out:?}", total);
            Ok(())
        }
        Command::Generate { prompt, page, model, endpoint, out, ipc_base, stream } => {
            let client = fd_ai_adapter::FusionMlxClient::with_endpoint(&endpoint)?;
            if stream {
                let model_owned = model.clone();
                let sys = "你是 fusion-design UI 生成器。根据用户描述，\
输出严格 JSON：{\"page\":{...}}。只输出 JSON。";
                let user_msg = format!("描述：{prompt}\n生成页面「{page}」对应的 UI 布局。");
                let s = fd_ai_adapter::chat_stream(
                    client,
                    model_owned,
                    sys.to_string(),
                    user_msg,
                    2048,
                ).await;
                use futures::StreamExt;
                futures::pin_mut!(s);
                while let Some(delta) = s.next().await {
                    match delta {
                        Ok(d) if d.finished => break,
                        Ok(d) => print!("{}", d.token),
                        Err(e) => eprintln!("流式输出错误: {e}"),
                    }
                }
                println!();
                return Ok(());
            }
            let skills = fd_ai_adapter::DesignSkills::new(client, model);
            let doc = skills.text_to_ui_async(&prompt, &page).await?;
            let json = serde_json::to_string_pretty(&doc)?;
            if let Some(base) = ipc_base {
                let link = fd_ecosystem::EcosystemLink::new(&base);
                let msg = fd_ecosystem::LinkMessage {
                    target: fd_ecosystem::EcosystemTarget::FusionCLI,
                    action: "generate-done".into(),
                    payload: serde_json::json!({
                        "page": page,
                        "document": json,
                    }),
                };
                link.send(&msg)?;
                tracing::info!("generate: IPC 消息已发送");
                if let Some(p) = out {
                    std::fs::write(&p, &json)?;
                    println!("已生成 PenDocument JSON 到 {p:?}");
                } else {
                    println!("{json}");
                }
            } else {
                match out {
                    Some(p) => {
                        std::fs::write(&p, &json)?;
                        println!("已生成 PenDocument JSON 到 {p:?}");
                    }
                    None => println!("{json}"),
                }
            }
            Ok(())
        }
        Command::CheckFrontend { dir, backend } => {
            host::HostBridgeConfig {
                frontend_dir: dir,
                backend_endpoint: backend,
                block_external: true,
            }
            .validate()?;
            println!("前端目录校验通过");
            Ok(())
        }
        Command::CheckMlx { endpoint } => {
            fd_ai_adapter::FusionMlxClient::with_endpoint(&endpoint)?;
            println!("fusion-mlx endpoint 校验通过: {endpoint}");
            Ok(())
        }
        Command::ParseHtml { input, page } => {
            let html = match input {
                Some(p) => std::fs::read_to_string(&p)?,
                None => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf
                }
            };
            let page_name = page.as_deref().unwrap_or("Page");
            let doc = fd_ai_adapter::html_to_pen_document(&html, page_name)?;
            let json = serde_json::to_string_pretty(&doc)?;
            println!("{json}");
            Ok(())
        }
        Command::TokenCSS { design_system } => {
            let mut reg = design::DesignSystemRegistry::new();
            reg.register_builtin();
            let system = reg.get(&design_system)
                .ok_or_else(|| anyhow::anyhow!("设计规范 '{design_system}' 未找到，可用: {:?}", reg.list()))?;
            println!("{}", system.to_css_custom_properties());
            Ok(())
        }
        Command::Lint { input, design_system, rules, fix, dry_run } => {
            let json = std::fs::read_to_string(&input)?;
            let mut doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;

            let mut linter = match rules {
                Some(r) => fd_design_lint::Linter::with_rules(r.into_iter().map(Into::into).collect()),
                None => fd_design_lint::Linter::new(),
            };

            if let Some(ref ds_id) = design_system {
                let mut reg = design::DesignSystemRegistry::new();
                reg.register_builtin();
                if let Some(system) = reg.get(ds_id) {
                    linter = linter.with_design_system(system.clone());
                }
            }

            let result = linter.lint(&doc);
            let output = serde_json::to_string_pretty(&result)?;
            println!("{output}");

            if fix {
                let fix_result = linter.auto_fix(&mut doc);
                let fix_output = serde_json::to_string_pretty(&fix_result)?;
                println!("{fix_output}");

                if !dry_run {
                    let fixed_json = serde_json::to_string_pretty(&doc)?;
                    std::fs::write(&input, &fixed_json)?;
                    eprintln!("修复已写入: {}", input.display());
                } else {
                    eprintln!("dry-run 模式: 修复未写入文件");
                }
            }

            Ok(())
        }
        Command::Codegen { input, target, component, out } => {
            let json = match input {
                Some(p) => std::fs::read_to_string(&p)?,
                None => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf
                }
            };
            let doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;
            let code = match target {
                CodegenTargetArg::Html => {
                    use fd_codegen::Codegen;
                    fd_codegen::HtmlCodegen.generate(&doc)
                }
                CodegenTargetArg::ReactTailwind => {
                    use fd_codegen::Codegen;
                    fd_codegen::ReactTailwindCodegen {
                        component_name: component.clone(),
                    }.generate(&doc)
                }
                CodegenTargetArg::TailwindOnly => {
                    use fd_codegen::Codegen;
                    fd_codegen::TailwindOnlyCodegen.generate(&doc)
                }
                CodegenTargetArg::SwiftUi => {
                    use fd_codegen::Codegen;
                    fd_codegen::SwiftUiCodegen {
                        view_name: component.clone(),
                    }.generate(&doc)
                }
            };
            match out {
                Some(p) => {
                    std::fs::write(&p, &code)?;
                    tracing::info!("已导出代码到 {p:?}");
                }
                None => println!("{code}"),
            }
            Ok(())
        }
        Command::Undo { input } => {
            let history_path = input.with_extension("history.json");
            if !history_path.exists() {
                anyhow::bail!("历史文件不存在: {history_path:?}");
            }
            let hist_json = std::fs::read_to_string(&history_path)?;
            let mut stack: fd_canvas_core::UndoRedoStack = serde_json::from_str(&hist_json)
                .map_err(|e| anyhow::anyhow!("历史文件解析失败: {e}"))?;
            match stack.undo() {
                Some(doc) => {
                    let out_json = serde_json::to_string_pretty(&doc)?;
                    let hist_out = serde_json::to_string_pretty(&stack)?;
                    std::fs::write(&history_path, &hist_out)?;
                    println!("{out_json}");
                    tracing::info!("undo: 成功回退");
                    Ok(())
                }
                None => anyhow::bail!("无法撤销：已到最早状态"),
            }
        }
        Command::Redo { input } => {
            let history_path = input.with_extension("history.json");
            if !history_path.exists() {
                anyhow::bail!("历史文件不存在: {history_path:?}");
            }
            let hist_json = std::fs::read_to_string(&history_path)?;
            let mut stack: fd_canvas_core::UndoRedoStack = serde_json::from_str(&hist_json)
                .map_err(|e| anyhow::anyhow!("历史文件解析失败: {e}"))?;
            match stack.redo() {
                Some(doc) => {
                    let out_json = serde_json::to_string_pretty(&doc)?;
                    let hist_out = serde_json::to_string_pretty(&stack)?;
                    std::fs::write(&history_path, &hist_out)?;
                    println!("{out_json}");
                    tracing::info!("redo: 成功重做");
                    Ok(())
                }
                None => anyhow::bail!("无法重做：已到最新状态"),
            }
        }
        Command::Health { endpoint } => {
            let client = fd_ai_adapter::FusionMlxClient::with_endpoint(&endpoint)?;
            let status = client.health_check().await;
            let output = match status {
                Ok(s) => serde_json::to_string_pretty(&s)?,
                Err(e) => serde_json::to_string_pretty(&serde_json::json!({
                    "available": false,
                    "error": e.to_string()
                }))?,
            };
            println!("{output}");
            Ok(())
        }
        Command::Diff { old, new } => {
            let old_json = std::fs::read_to_string(&old)?;
            let new_json = std::fs::read_to_string(&new)?;
            let old_doc: fd_canvas_core::PenDocument = serde_json::from_str(&old_json)?;
            let new_doc: fd_canvas_core::PenDocument = serde_json::from_str(&new_json)?;
            let diff = old_doc.diff(&new_doc);
            let output = serde_json::to_string_pretty(&diff)?;
            println!("{output}");
            Ok(())
        }
        Command::Theme { design_system, mode } => {
            let mut reg = fd_design_system::DesignSystemRegistry::new();
            reg.register_builtin();
            let system = reg.get(&design_system)
                .ok_or_else(|| anyhow::anyhow!("设计规范 '{design_system}' 未找到，可用: {:?}", reg.list()))?;
            let css = system.to_css_custom_properties_for_theme(mode.into());
            println!("{css}");
            Ok(())
        }
    }
}
