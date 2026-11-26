//! A-3: Chat subcommand handler (split out of main.rs).
//!
//! Machine-readable streaming chat: streaming inference interface shared by the
//! fusion-studio subprocess and CLI pipelines.
//! Auth / X-Fusion-Route header / endpoint resolution reuse fd-ai-adapter; callers do not
//! reimplement them.
//!
//! Consumer declaration (issue #17 → issue #20 evolution): issue #17 originally envisioned
//! this subcommand as the studio subprocess entry replacing direct MLX access; early
//! verification assumed studio went through fusion-gateway TCP so there was no studio
//! consumer. Later verification showed studio DesignBridge.sendDesignChat is actually
//! inline URLSession HTTP (self-managed Bearer + SSE parsing + X-Fusion-Route),
//! duplicating adapter protection logic.
//! issue #20 closing direction: studio calls this subcommand;
//! auth/RouteGuard/validate_localhost/false-green(check_generate) reuse the adapter,
//! callers do not reimplement them. For compatibility with studio's existing
//! runFusionDesignStream subprocess parser (takes choices[0].delta.content on the
//! `data: ` prefix, ends at `data: [DONE]`), `--format sse` was added to emit raw OpenAI
//! text/event-stream. `--format ndjson` (default) keeps the existing issue #17 contract
//! (one delta/chat_done/error frame per line).

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

    // system prompt: --system-prompt-file takes precedence over inline --system-prompt
    let sys = match system_prompt_file {
        Some(p) => read_file_capped(&p)?,
        None => system_prompt,
    };
    // Inject RAG context at the tail of the system prompt
    let sys = match rag_context_file {
        Some(p) => {
            let rag = read_file_capped(&p)?;
            if sys.is_empty() {
                rag
            } else {
                // i18n: user-facing (LLM prompt separator)
                format!("{sys}\n\n--- 以下为参考上下文 ---\n{rag}")
            }
        }
        None => sys,
    };

    // messages: --messages-file multi-turn history (JSON array), empty if absent
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
    // Prepend the system message (when given)
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
        anyhow::bail!("chat: no messages (--messages-file missing or empty) and no system prompt");
    }
    // L-8: role guard — bail directly on invalid roles instead of forwarding them to MLX
    // and triggering ambiguous errors.
    for m in &messages {
        if !matches!(m.role.as_str(), "system" | "user" | "assistant") {
            anyhow::bail!(
                "chat: invalid role '{}', only system/user/assistant are supported",
                m.role
            );
        }
    }
    tracing::info!(model = %model, count = messages.len(), "chat: streaming inference started");

    // Framed output: --format selects ndjson (issue #17 contract) or sse (raw OpenAI
    // text/event-stream, aligned with the fusion-studio runFusionDesignStream parser).
    // Subprocess pipes are block-buffered; each frame must flush stdout, otherwise studio's
    // readabilityHandler only receives data when the buffer fills, degrading the streaming
    // experience. The H-A16/P1-8 retrospective item is obsolete: studio consumes via this
    // subcommand.
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
                    tracing::error!(error = %e, "chat: streaming error");
                    stream_failed = Some(e.to_string());
                    break;
                }
            }
        }
        if let Some(e) = stream_failed {
            anyhow::bail!("chat streaming failed: {e}");
        }
    } else {
        anyhow::bail!("chat: only --stream --json streaming output is currently supported (--format ndjson|sse selects framing)");
    }
    Ok(())
}
