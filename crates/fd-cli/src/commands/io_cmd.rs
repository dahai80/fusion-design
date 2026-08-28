//! A-3：IO/校验子命令 handler（ExportBatch/CheckFrontend/ParseHtml/Codegen/Diff）。

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
    // E-12/E-24：批量导出旧实现首个格式失败即 `?` 传播，其余格式静默跳过。
    // 改为收集所有失败，末尾汇总打印 + 若有失败退出非零（fail visibly，不阻断已成功项）。
    let mut failures: Vec<String> = Vec::new();
    for fmt in fmt_list {
        match fd_export::Exporter::from_pen_document_with_tokens(&doc, fmt, &out, &reg) {
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

pub async fn check_frontend(dir: PathBuf, backend: String) -> anyhow::Result<()> {
    fd_host_desk::HostBridgeConfig {
        frontend_dir: dir,
        backend_endpoint: backend,
        block_external: true,
    }
    .validate()?;
    println!("前端目录校验通过");
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
    // H-A11：codegen 前解析 token 引用（token:color.accent → 实际 hex）。
    // 旧实现直接把 PenDocument 传 codegen，token:xxx 原样输出——浏览器不认，
    // SwiftUI 侧 DesignTokens.xxx 未定义编译失败。按文档声明的
    // active_design_system 构建注册表并解析，递归覆盖嵌套 children。
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
            tracing::info!("已导出代码到 {p:?}");
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
