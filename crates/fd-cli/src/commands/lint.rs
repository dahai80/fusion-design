//! A-3：Lint 子命令 handler（从 main.rs 拆出）。
//!
//! 设计规范检测：PenDocument → LintResult。支持 --fix 原子写 + E-10 备份轮转。

use std::path::PathBuf;

use crate::common::{read_file_capped, rotate_backup};
use crate::LintRuleArg;

pub fn run(
    input: PathBuf,
    design_system: Option<String>,
    rules: Option<Vec<LintRuleArg>>,
    fix: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    let json = read_file_capped(&input)?;
    let mut doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;

    let mut linter = match rules {
        Some(r) => fd_design_lint::Linter::with_rules(r.into_iter().map(Into::into).collect()),
        None => fd_design_lint::Linter::new(),
    };

    if let Some(ref ds_id) = design_system {
        let mut reg = fd_cli::design::DesignSystemRegistry::new();
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
            // L-4 + E-10：原子写——先备份原文件，再写临时文件后 rename，写失败不破坏原文件。
            // E-10：备份轮转保留最近 3 份，旧实现每次覆盖单 .bak，历史修复不可回溯。
            let backup = rotate_backup(&input);
            match std::fs::copy(&input, &backup) {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "备份写入失败（rename 跨文件系统？），降级跳过备份");
                }
            }
            let tmp = input.with_extension("fusiondesign.tmp");
            match std::fs::write(&tmp, &fixed_json) {
                Ok(_) => match std::fs::rename(&tmp, &input) {
                    Ok(_) => {}
                    Err(e) => {
                        // rename 跨文件系统失败 → 回退 read+write 保数据。
                        tracing::warn!(error = %e, "rename 失败，回退直接写");
                        std::fs::write(&input, &fixed_json)?;
                        let _ = std::fs::remove_file(&tmp);
                    }
                },
                Err(e) => {
                    return Err(anyhow::anyhow!("写临时文件失败: {e}"));
                }
            }
            eprintln!(
                "修复已写入: {}（备份: {}）",
                input.display(),
                backup.display()
            );
        } else {
            eprintln!("dry-run 模式: 修复未写入文件");
        }
    }

    Ok(())
}
