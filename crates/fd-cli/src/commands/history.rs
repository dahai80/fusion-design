//! A-3：历史快照子命令 handler（Undo/Redo）。
//! Ruling #12：本模块仅做 ARCH-3 代码位置迁移，保留 fd_canvas_core::UndoRedoStack
//! undo()/redo() 逐字调用。FUNC-7 (T16) 后续在 fd-canvas-core 重写 delta-undo 行为。

use std::path::PathBuf;

use crate::common::read_file_capped;

pub async fn undo(input: PathBuf) -> anyhow::Result<()> {
    let history_path = input.with_extension("history.json");
    if !history_path.exists() {
        anyhow::bail!("历史文件不存在: {history_path:?}");
    }
    let hist_json = read_file_capped(&history_path)?;
    let mut stack: fd_canvas_core::UndoRedoStack =
        serde_json::from_str(&hist_json).map_err(|e| anyhow::anyhow!("历史文件解析失败: {e}"))?;
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

pub async fn redo(input: PathBuf) -> anyhow::Result<()> {
    let history_path = input.with_extension("history.json");
    if !history_path.exists() {
        anyhow::bail!("历史文件不存在: {history_path:?}");
    }
    let hist_json = read_file_capped(&history_path)?;
    let mut stack: fd_canvas_core::UndoRedoStack =
        serde_json::from_str(&hist_json).map_err(|e| anyhow::anyhow!("历史文件解析失败: {e}"))?;
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
