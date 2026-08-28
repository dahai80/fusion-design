//! A-3：历史快照子命令 handler（Undo/Redo）。
//! Ruling #12：本模块仅做 ARCH-3 代码位置迁移，保留 fd_canvas_core::UndoRedoStack
//! undo()/redo() 逐字调用。FUNC-7 (T16) 后续在 fd-canvas-core 重写 delta-undo 行为。
//! FUNC-7 已落地：delta 栈需 current 初始态，旧快照式 history（无 current 字段）
//! 反序列化失败 → catch + warn 重建（用当前 .fusiondesign doc 作 initial）。

use std::path::PathBuf;

use crate::common::read_file_capped;

pub async fn undo(input: PathBuf) -> anyhow::Result<()> {
    let history_path = input.with_extension("history.json");
    if !history_path.exists() {
        anyhow::bail!("历史文件不存在: {history_path:?}");
    }
    let hist_json = read_file_capped(&history_path)?;
    let mut stack: fd_canvas_core::UndoRedoStack = match serde_json::from_str(&hist_json) {
        Ok(s) => s,
        Err(e) => {
            // FUNC-7：旧快照式 history（VecDeque<PenDocument>，无 current 字段）不兼容 delta 栈。
            // 破坏性接受：丢弃旧 history，用当前 .fusiondesign doc 重建空栈，用户须重新操作。
            tracing::warn!(error = %e, "旧快照式 history 不兼容 delta 栈，丢弃历史重建");
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
    let mut stack: fd_canvas_core::UndoRedoStack = match serde_json::from_str(&hist_json) {
        Ok(s) => s,
        Err(e) => {
            // FUNC-7：旧快照式 history（VecDeque<PenDocument>，无 current 字段）不兼容 delta 栈。
            // 破坏性接受：丢弃旧 history，用当前 .fusiondesign doc 重建空栈，用户须重新操作。
            tracing::warn!(error = %e, "旧快照式 history 不兼容 delta 栈，丢弃历史重建");
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
            tracing::info!("redo: 成功重做");
            Ok(())
        }
        None => anyhow::bail!("无法重做：已到最新状态"),
    }
}
