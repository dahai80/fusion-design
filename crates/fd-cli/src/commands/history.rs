//! A-3: history snapshot subcommand handlers (Undo/Redo).
//! Ruling #12: this module is a pure ARCH-3 code-location move; it keeps verbatim calls to
//! fd_canvas_core::UndoRedoStack undo()/redo(). FUNC-7 (T16) later rewrites delta-undo
//! behavior in fd-canvas-core.
//! FUNC-7 has landed: the delta stack needs a `current` initial state; when the old
//! snapshot-style history (no `current` field) fails to deserialize → catch + warn and
//! rebuild (using the current .fusiondesign doc as initial).

use std::path::PathBuf;

use crate::common::read_file_capped;

pub async fn undo(input: PathBuf) -> anyhow::Result<()> {
    let history_path = input.with_extension("history.json");
    if !history_path.exists() {
        anyhow::bail!("history file does not exist: {history_path:?}");
    }
    let hist_json = read_file_capped(&history_path)?;
    let mut stack: fd_canvas_core::UndoRedoStack = match serde_json::from_str(&hist_json) {
        Ok(s) => s,
        Err(e) => {
            // FUNC-7: the old snapshot-style history (VecDeque<PenDocument>, no `current`
            // field) is incompatible with the delta stack. Destructive acceptance: drop
            // the old history, rebuild an empty stack from the current .fusiondesign doc,
            // and the user must redo their operations.
            tracing::warn!(error = %e, "snapshot-style history incompatible with delta stack, dropping history and rebuilding");
            let doc_json = read_file_capped(&input).unwrap_or_default();
            let doc: fd_canvas_core::PenDocument = serde_json::from_str(&doc_json)
                .unwrap_or_else(|_| fd_canvas_core::PenDocument::default());
            fd_canvas_core::UndoRedoStack::new(doc)
        }
    };
    match stack.undo() {
        Some(doc) => {
            let out_json = serde_json::to_string_pretty(&doc)?;
            let hist_out = serde_json::to_string_pretty(&stack)?;
            tokio::fs::write(&history_path, &hist_out).await?;
            println!("{out_json}");
            tracing::info!("undo: reverted successfully");
            Ok(())
        }
        None => anyhow::bail!("cannot undo: already at the earliest state"),
    }
}

pub async fn redo(input: PathBuf) -> anyhow::Result<()> {
    let history_path = input.with_extension("history.json");
    if !history_path.exists() {
        anyhow::bail!("history file does not exist: {history_path:?}");
    }
    let hist_json = read_file_capped(&history_path)?;
    let mut stack: fd_canvas_core::UndoRedoStack = match serde_json::from_str(&hist_json) {
        Ok(s) => s,
        Err(e) => {
            // FUNC-7: the old snapshot-style history (VecDeque<PenDocument>, no `current`
            // field) is incompatible with the delta stack. Destructive acceptance: drop
            // the old history, rebuild an empty stack from the current .fusiondesign doc,
            // and the user must redo their operations.
            tracing::warn!(error = %e, "snapshot-style history incompatible with delta stack, dropping history and rebuilding");
            let doc_json = read_file_capped(&input).unwrap_or_default();
            let doc: fd_canvas_core::PenDocument = serde_json::from_str(&doc_json)
                .unwrap_or_else(|_| fd_canvas_core::PenDocument::default());
            fd_canvas_core::UndoRedoStack::new(doc)
        }
    };
    match stack.redo() {
        Some(doc) => {
            let out_json = serde_json::to_string_pretty(&doc)?;
            let hist_out = serde_json::to_string_pretty(&stack)?;
            tokio::fs::write(&history_path, &hist_out).await?;
            println!("{out_json}");
            tracing::info!("redo: reapplied successfully");
            Ok(())
        }
        None => anyhow::bail!("cannot redo: already at the latest state"),
    }
}
