//! Fusion-Design 生态联动 — 对接 Fusion Code/Simulation/KB/CLI。
//!
//! 对应 PRD 模块 6「生态联动能力」。通过本地文件 IPC（约定目录下的
//! JSON 消息文件）+ 未来 MCP 协议（op-mcp）打通全系生态。
//!
//! 【离线硬约束】所有联动走本地文件系统或 127.0.0.1，无公网调用。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 生态联动目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EcosystemTarget {
    /// Fusion Code（正向导出代码、反向同步样式）
    FusionCode,
    /// Fusion-Simulation（生成机器人仿真控制面板）
    FusionSimulation,
    /// Fusion-KB（保存/检索设计模板）
    FusionKB,
    /// Fusion CLI（命令行批量生成/导出）
    FusionCLI,
}

impl EcosystemTarget {
    /// 约定的 IPC 目录名。
    pub fn ipc_dir(self) -> &'static str {
        match self {
            Self::FusionCode => "fusion-code",
            Self::FusionSimulation => "fusion-simulation",
            Self::FusionKB => "fusion-kb",
            Self::FusionCLI => "fusion-cli",
        }
    }
}

/// 联动消息体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkMessage {
    pub target: EcosystemTarget,
    pub action: String,
    pub payload: serde_json::Value,
}

/// 生态联动客户端（基于本地文件 IPC）。
#[derive(Debug, Clone)]
pub struct EcosystemLink {
    base_dir: PathBuf,
}

impl EcosystemLink {
    /// 用指定基目录构造（所有 IPC 文件落在 `<base>/<target>/`）。
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self { base_dir: base_dir.into() }
    }

    /// 发送一条联动消息（写入目标目录的 JSON 文件）。
    pub fn send(&self, msg: &LinkMessage) -> anyhow::Result<PathBuf> {
        let dir = self.base_dir.join(msg.target.ipc_dir());
        std::fs::create_dir_all(&dir)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let file = dir.join(format!("{stamp}.json"));
        let json = serde_json::to_string_pretty(msg)?;
        std::fs::write(&file, json)?;
        tracing::info!(?file, target = msg.target.ipc_dir(), "联动消息已发送");
        Ok(file)
    }

    /// 列出某目标的全部待处理消息文件。
    pub fn list(&self, target: EcosystemTarget) -> anyhow::Result<Vec<PathBuf>> {
        let dir = self.base_dir.join(target.ipc_dir());
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        files.sort();
        Ok(files)
    }

    /// 读取并删除一条消息（消费式）。
    pub fn consume(&self, file: &Path) -> anyhow::Result<LinkMessage> {
        let json = std::fs::read_to_string(file)?;
        let msg: LinkMessage = serde_json::from_str(&json)?;
        std::fs::remove_file(file)?;
        Ok(msg)
    }

    /// 异步发送（供任务队列调用）。
    pub async fn send_async(&self, msg: LinkMessage) -> anyhow::Result<PathBuf> {
        let base = self.base_dir.clone();
        tokio::task::spawn_blocking(move || {
            EcosystemLink::new(base).send(&msg)
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn send_and_list_and_consume() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let msg = LinkMessage {
            target: EcosystemTarget::FusionCode,
            action: "export-code".into(),
            payload: serde_json::json!({"page": "login"}),
        };
        let file = link.send(&msg).unwrap();
        let listed = link.list(EcosystemTarget::FusionCode).unwrap();
        assert_eq!(listed, vec![file.clone()]);
        let consumed = link.consume(&file).unwrap();
        assert_eq!(consumed.action, "export-code");
        // consume 后文件删除
        assert!(link.list(EcosystemTarget::FusionCode).unwrap().is_empty());
    }

    #[test]
    fn list_empty_when_dir_absent() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        assert!(link.list(EcosystemTarget::FusionKB).unwrap().is_empty());
    }

    #[test]
    fn ipc_dir_names() {
        assert_eq!(EcosystemTarget::FusionCode.ipc_dir(), "fusion-code");
        assert_eq!(EcosystemTarget::FusionSimulation.ipc_dir(), "fusion-simulation");
    }

    #[tokio::test]
    async fn send_async_works() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let msg = LinkMessage {
            target: EcosystemTarget::FusionCLI,
            action: "batch-export".into(),
            payload: serde_json::json!({}),
        };
        let file = link.send_async(msg).await.unwrap();
        assert!(file.exists());
    }
}
