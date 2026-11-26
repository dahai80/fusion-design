//! A-3: Lint subcommand handler (split out of main.rs).
//!
//! Design-spec lint: PenDocument → LintResult. Supports --fix atomic write + E-10 backup
//! rotation.
//! Issue #41: --profile selects the rule set (default / desigma admission gate); --gate is
//! for cowork/CI pipeline consumption: exit code 1 when any Error-severity violation
//! exists, 0 on pass.

use std::path::PathBuf;

use crate::common::{read_file_capped, rotate_backup};
use crate::{LintProfileArg, LintRuleArg};

pub fn run(
    input: PathBuf,
    design_system: Option<String>,
    profile: LintProfileArg,
    rules: Option<Vec<LintRuleArg>>,
    fix: bool,
    dry_run: bool,
    gate: bool,
) -> anyhow::Result<()> {
    let json = read_file_capped(&input)?;
    let mut doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;

    // Issue #41: explicit --rules overrides --profile; otherwise the profile
    // picks the rule set (default = all 17 rules, desigma = 4 gate rules).
    let mut linter = match rules {
        Some(r) => fd_design_lint::Linter::with_rules(r.into_iter().map(Into::into).collect()),
        None => fd_design_lint::Linter::with_profile(profile.into()),
    };

    if let Some(ref ds_id) = design_system {
        let mut reg = fd_cli::design::DesignSystemRegistry::new();
        reg.register_builtin();
        if let Some(system) = reg.get(ds_id) {
            linter = linter.with_design_system(system.clone());
        } else {
            tracing::warn!(design_system = %ds_id, "design system not found, linting without tokens");
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
            // L-4 + E-10: atomic write — back up the original file first, then write a
            // temp file and rename, so a failed write never corrupts the original.
            // E-10: backup rotation keeps the latest 3 copies; the old implementation
            // overwrote a single .bak each time, making past fixes unrecoverable.
            let backup = rotate_backup(&input);
            match std::fs::copy(&input, &backup) {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "backup write failed (cross-filesystem rename?), skipping backup as fallback");
                }
            }
            let tmp = input.with_extension("fusiondesign.tmp");
            match std::fs::write(&tmp, &fixed_json) {
                Ok(_) => match std::fs::rename(&tmp, &input) {
                    Ok(_) => {}
                    Err(e) => {
                        // rename fails across filesystems → fall back to read+write to keep data.
                        tracing::warn!(error = %e, "rename failed, falling back to direct write");
                        std::fs::write(&input, &fixed_json)?;
                        let _ = std::fs::remove_file(&tmp);
                    }
                },
                Err(e) => {
                    return Err(anyhow::anyhow!("failed to write temp file: {e}"));
                }
            }
            eprintln!(
                "fixes written: {} (backup: {})",
                input.display(),
                backup.display()
            );
        } else {
            eprintln!("dry-run mode: fixes not written to file");
        }
    }

    // Issue #41: gate semantics for pipeline consumption (cowork desigma gate).
    // Exit 0 = pass, exit 1 = at least one Error-severity violation, exit 2 =
    // runtime failure (anyhow). Result JSON above stays machine-readable.
    if gate {
        if result.stats.errors > 0 {
            eprintln!(
                "gate: FAILED — {} error violation(s) ({} warning, {} info)",
                result.stats.errors, result.stats.warnings, result.stats.infos
            );
            std::process::exit(1);
        }
        eprintln!("gate: PASSED — 0 error violations");
    }

    Ok(())
}
