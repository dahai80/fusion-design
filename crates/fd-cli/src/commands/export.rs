//! A-3：Export 子命令 handler（从 main.rs 拆出）。
//!
//! 单文档导出到指定格式，可选经 fd-ecosystem IPC 通知 FusionCLI。

use std::path::PathBuf;

use crate::common::{build_registry, read_file_capped};
use crate::ExportFormatArg;

pub async fn run(
    input: PathBuf,
    format: ExportFormatArg,
    out: PathBuf,
    ipc_base: Option<PathBuf>,
) -> anyhow::Result<()> {
    let json = read_file_capped(&input)?;
    let doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;
    let reg = build_registry(&doc);
    let fmt: fd_export::ExportFormat = format.into();
    // E-11：IPC format 字段旧实现 `{:?}` Debug 输出大写（"Html"/"Svg"），
    // 与文件扩展名/IPC 消费方期望的小写不一致。改用 extension() 稳定小写契约。
    let format_str = fmt.extension().to_string();
    let files = fd_export::Exporter::from_pen_document_with_tokens(&doc, fmt, &out, &reg)?;
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
