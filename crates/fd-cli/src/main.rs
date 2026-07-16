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
    },
    /// 校验前端静态资源目录
    CheckFrontend {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        backend: String,
    },
    /// 校验 fusion-mlx endpoint 是否为 localhost
    CheckMlx {
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        endpoint: String,
    },
}

#[derive(Clone, clap::ValueEnum)]
pub enum ExportFormatArg {
    Html,
    Svg,
    Json,
}

impl From<ExportFormatArg> for fd_export::ExportFormat {
    fn from(v: ExportFormatArg) -> Self {
        match v {
            ExportFormatArg::Html => Self::Html,
            ExportFormatArg::Svg => Self::Svg,
            ExportFormatArg::Json => Self::Json,
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
        Command::Export { input, format, out } => {
            let json = std::fs::read_to_string(&input)?;
            let page: export::CanvasPage = serde_json::from_str(&json)?;
            export::Exporter::export_page(&page, format.into(), &out)?;
            println!("已导出到 {out:?}");
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
    }
}
