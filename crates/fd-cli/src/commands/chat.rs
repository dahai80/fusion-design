//! A-3：Chat 子命令 handler（从 main.rs 拆出）。
//!
//! 机器可读流式 chat：CLI/脚本管道用的流式 NDJSON 推理接口。
//! 鉴权 / X-Fusion-Route header / endpoint 解析复用 fd-ai-adapter，调用方不重实现。
//!
//! 消费方声明（issue #17 诚实回溯）：issue #17 设想此子命令为 fusion-studio
//! subprocess 入口取代直连 MLX，但经核实 studio 实际走 fusion-gateway TCP
//! NDJSON（StreamingBridge.swift，帧 schema 为 chat_event/chat_done/error +
//! session_id/event），**不经 fd-cli chat**。故本子命令当前无 studio 消费方，
//! 供 CLI 管道/脚本/测试消费。NDJSON 帧 schema（delta/chat_done/error）为
//! 本子命令自洽契约，非对齐 studio（studio 用 chat_event 非 delta）。

use std::path::PathBuf;

use crate::common::{ndjson_frame_delta, ndjson_frame_done, ndjson_frame_error, read_file_capped};

#[allow(clippy::too_many_arguments)]
pub async fn run(
    model: String,
    endpoint: String,
    system_prompt: String,
    system_prompt_file: Option<PathBuf>,
    messages_file: Option<PathBuf>,
    rag_context_file: Option<PathBuf>,
    max_tokens: u32,
    stream: bool,
    json: bool,
) -> anyhow::Result<()> {
    use fd_ai_adapter::{MlxChatMessage, MlxStreamDelta};
    use futures::StreamExt;

    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;

    // system prompt：--system-prompt-file 优先于内联 --system-prompt
    let sys = match system_prompt_file {
        Some(p) => read_file_capped(&p)?,
        None => system_prompt,
    };
    // RAG 上下文注入 system prompt 尾部
    let sys = match rag_context_file {
        Some(p) => {
            let rag = read_file_capped(&p)?;
            if sys.is_empty() {
                rag
            } else {
                format!("{sys}\n\n--- 以下为参考上下文 ---\n{rag}")
            }
        }
        None => sys,
    };

    // messages：--messages-file 多轮历史（JSON 数组），缺省则空
    let mut messages: Vec<MlxChatMessage> = match messages_file {
        Some(p) => {
            let raw = read_file_capped(&p)?;
            if raw.trim().is_empty() {
                Vec::new()
            } else {
                serde_json::from_str::<Vec<MlxChatMessage>>(&raw)?
            }
        }
        None => Vec::new(),
    };
    // 前置 system 消息（若给定）
    if !sys.is_empty() {
        messages.insert(
            0,
            MlxChatMessage {
                role: "system".into(),
                content: sys,
            },
        );
    }
    if messages.is_empty() {
        anyhow::bail!("chat: 无 messages（--messages-file 缺失或空）且无 system prompt");
    }
    // L-8：role 护栏——非法 role 直接 bail，不透传给 MLX 引发歧义错误。
    for m in &messages {
        if !matches!(m.role.as_str(), "system" | "user" | "assistant") {
            anyhow::bail!("chat: 非法 role '{}'，仅支持 system/user/assistant", m.role);
        }
    }
    tracing::info!(model = %model, count = messages.len(), "chat: 流式推理开始");

    // NDJSON 成帧输出：delta / chat_done / error（本子命令自洽契约）。
    // H-A16/P1-8 回溯：issue #17 设想 studio 经此契约接 fd-cli，但核实 studio
    // 走 gateway TCP chat_event/chat_done/error，不经 fd-cli。此 schema 供
    // CLI 管道消费，非对齐 studio。
    if stream && json {
        let s = fd_ai_adapter::chat_stream_messages(client, model, messages, max_tokens).await;
        futures::pin_mut!(s);
        let mut stream_failed: Option<String> = None;
        while let Some(item) = s.next().await {
            match item {
                Ok(MlxStreamDelta { token, finished }) => {
                    if finished {
                        println!("{}", ndjson_frame_done());
                        break;
                    } else if !token.is_empty() {
                        println!("{}", ndjson_frame_delta(&token));
                    }
                }
                Err(e) => {
                    println!("{}", ndjson_frame_error(&e.to_string()));
                    tracing::error!(error = %e, "chat: 流式错误");
                    stream_failed = Some(e.to_string());
                    break;
                }
            }
        }
        if let Some(e) = stream_failed {
            anyhow::bail!("chat 流式失败: {e}");
        }
    } else {
        anyhow::bail!("chat: 当前仅支持 --stream --json NDJSON 输出");
    }
    Ok(())
}
