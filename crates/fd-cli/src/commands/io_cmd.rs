//! A-3: IO/validation subcommand handlers (ExportBatch/CheckFrontend/ParseHtml/Codegen/Diff).

use std::path::PathBuf;

use crate::common::{build_registry, read_file_capped, read_stdin_capped};
use crate::CodegenTargetArg;
use crate::ExportFormatArg;

pub async fn export_batch(
    input: PathBuf,
    format: ExportFormatArg,
    formats: Option<Vec<ExportFormatArg>>,
    out: PathBuf,
) -> anyhow::Result<()> {
    let json = read_file_capped(&input)?;
    let doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;
    let reg = build_registry(&doc);
    let fmt_list: Vec<fd_export::ExportFormat> = if let Some(ref fmts) = formats {
        fmts.iter().map(|f| f.clone().into()).collect()
    } else {
        vec![format.into()]
    };
    let mut total = 0;
    // E-12/E-24: the old batch-export implementation propagated the first format failure
    // via `?` and silently skipped the remaining formats. Now collect all failures, print
    // a summary at the end, and exit non-zero if any failed (fail visibly without blocking
    // already-succeeded items).
    let mut failures: Vec<String> = Vec::new();
    for fmt in fmt_list {
        match fd_export::Exporter::from_pen_document_with_tokens(&doc, fmt, &out, &reg) {
            Ok(files) => {
                tracing::info!(format = ?fmt, count = files.len(), "batch export finished");
                total += files.len();
            }
            Err(e) => {
                let fmt_str = fmt.extension();
                tracing::warn!(format = fmt_str, error = %e, "batch export failed for this format");
                failures.push(format!("{fmt_str}: {e}"));
            }
        }
    }
    println!("batch-exported {} page(s) to {out:?}", total);
    if !failures.is_empty() {
        eprintln!("batch export partially failed ({} item(s)):", failures.len());
        for f in &failures {
            eprintln!("  - {f}");
        }
        return Err(anyhow::anyhow!(
            "batch export partially failed: exported {total} page(s), {} format(s) failed",
            failures.len()
        ));
    }
    Ok(())
}

pub async fn check_frontend(dir: PathBuf, backend: String) -> anyhow::Result<()> {
    fd_host_desk::HostBridgeConfig {
        frontend_dir: dir,
        backend_endpoint: backend,
        block_external: true,
    }
    .validate()?;
    println!("frontend directory validation passed");
    Ok(())
}

pub async fn parse_html(input: Option<PathBuf>, page: Option<String>) -> anyhow::Result<()> {
    let html = match input {
        Some(p) => read_file_capped(&p)?,
        None => read_stdin_capped()?,
    };
    let page_name = page.as_deref().unwrap_or("Page");
    let doc = fd_ai_adapter::html_to_pen_document(&html, page_name)?;
    let json = serde_json::to_string_pretty(&doc)?;
    println!("{json}");
    Ok(())
}

pub async fn codegen(
    input: Option<PathBuf>,
    target: CodegenTargetArg,
    component: String,
    out: Option<PathBuf>,
) -> anyhow::Result<()> {
    let json = match input {
        Some(p) => read_file_capped(&p)?,
        None => read_stdin_capped()?,
    };
    let doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;
    // H-A11: resolve token references before codegen (token:color.accent → actual hex).
    // The old implementation passed the PenDocument straight to codegen, emitting
    // token:xxx verbatim — browsers reject it and the SwiftUI side fails to compile with
    // undefined DesignTokens.xxx. Build a registry from the document-declared
    // active_design_system and resolve, recursively overriding nested children.
    let reg = build_registry(&doc);
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
            tracing::info!("code exported to {p:?}");
        }
        None => println!("{code}"),
    }
    Ok(())
}

pub async fn diff(old: PathBuf, new: PathBuf) -> anyhow::Result<()> {
    let old_json = read_file_capped(&old)?;
    let new_json = read_file_capped(&new)?;
    let old_doc: fd_canvas_core::PenDocument = serde_json::from_str(&old_json)?;
    let new_doc: fd_canvas_core::PenDocument = serde_json::from_str(&new_json)?;
    let diff = old_doc.diff(&new_doc);
    let output = serde_json::to_string_pretty(&diff)?;
    println!("{output}");
    Ok(())
}
