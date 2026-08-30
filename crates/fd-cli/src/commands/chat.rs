//! A-3：Chat 子命令 handler（从 main.rs 拆出）。
//!
//! 机器可读流式 chat：流式推理接口，供 fusion-studio subprocess 与 CLI 管道共用。
//! 鉴权 / X-Fusion-Route header / endpoint 解析复用 fd-ai-adapter，调用方不重实现。
//!
//! 消费方声明（issue #17 → issue #20 演进）：issue #17 原设想此子命令为 studio
//! subprocess 入口取代直连 MLX，早期核实认为 studio 走 fusion-gateway TCP 故无
//! studio 消费方。后核实 studio DesignBridge.sendDesignChat 实为内联 URLSession
//! HTTP（自管 Bearer + SSE 解析 + X-Fusion-Route），重复实现 adapter 保护逻辑。
//! issue #20 闭合方向：studio 改调本子命令，鉴权/RouteGuard/validate_localhost/
//! false-green(check_generate) 复用 adapter，调用方不重实现。为兼容 studio 现有
//! runFusionDesignStream 子进程解析器（按 `data: ` 前缀取 choices[0].delta.content，
//! 遇 `data: [DONE]` 结束），新增 `--format sse` 输出 raw OpenAI text/event-stream。
//! `--format ndjson`（默认）保留 issue #17 既有契约（每行一帧 delta/chat_done/error）。

use std::io::Write;
use std::path::PathBuf;

use crate::common::{
    ndjson_frame_delta, ndjson_frame_done, ndjson_frame_error, read_file_capped, sse_frame_delta,
    sse_frame_done, sse_frame_error,
};
use crate::ChatStreamFormatArg;

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
    format: ChatStreamFormatArg,
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

    // 成帧输出：--format 选 ndjson（issue #17 契约）或 sse（raw OpenAI
    // text/event-stream，对齐 fusion-studio runFusionDesignStream 解析器）。
    // 子进程管道 block-buffered，每帧须 flush stdout，否则 studio readabilityHandler
    // 攒块到缓冲满才收，流式体验退化。H-A16/P1-8 回溯已废：studio 经本子命令消费。
    if stream && json {
        let s = fd_ai_adapter::chat_stream_messages(client, model, messages, max_tokens).await;
        futures::pin_mut!(s);
        let mut stream_failed: Option<String> = None;
        let mut stdout = std::io::stdout();
        while let Some(item) = s.next().await {
            match item {
                Ok(MlxStreamDelta { token, finished }) => {
                    if finished {
                        match format {
                            ChatStreamFormatArg::Ndjson => {
                                println!("{}", ndjson_frame_done());
                            }
                            ChatStreamFormatArg::Sse => {
                                print!("{}", sse_frame_done());
                            }
                        }
                        stdout.flush()?;
                        break;
                    } else if !token.is_empty() {
                        match format {
                            ChatStreamFormatArg::Ndjson => {
                                println!("{}", ndjson_frame_delta(&token));
                            }
                            ChatStreamFormatArg::Sse => {
                                print!("{}", sse_frame_delta(&token));
                            }
                        }
                        stdout.flush()?;
                    }
                }
                Err(e) => {
                    match format {
                        ChatStreamFormatArg::Ndjson => {
                            println!("{}", ndjson_frame_error(&e.to_string()));
                        }
                        ChatStreamFormatArg::Sse => {
                            print!("{}", sse_frame_error(&e.to_string()));
                        }
                    }
                    let _ = stdout.flush();
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
        anyhow::bail!("chat: 当前仅支持 --stream --json 流式输出（--format ndjson|sse 选成帧）");
    }
    Ok(())
}
