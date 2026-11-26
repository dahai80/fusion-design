//! A-3: Export subcommand handler (split out of main.rs).
//!
//! Exports a single document to the given format, optionally notifying FusionCLI via
//! fd-ecosystem IPC.

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
    // E-11: the IPC format field's old implementation used `{:?}` Debug output, which is
    // uppercase ("Html"/"Svg") and inconsistent with the lowercase expected by file
    // extensions/IPC consumers. Switched to extension() for a stable lowercase contract.
    let format_str = fmt.extension().to_string();
    let files = fd_export::Exporter::from_pen_document_with_tokens(&doc, fmt, &out, &reg)?;
    println!("exported {} page(s) to {out:?}", files.len());
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
        tracing::info!("export: IPC message sent");
    }
    Ok(())
}
