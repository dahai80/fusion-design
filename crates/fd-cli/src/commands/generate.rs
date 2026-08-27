//! A-3：Generate 子命令 handler（从 main.rs 拆出）。
//!
//! 文生 UI：自然语言 → PenDocument JSON。支持流式 SSE token 与 fd-ecosystem IPC。

use std::path::PathBuf;

pub async fn run(
    prompt: String,
    page: String,
    model: String,
    endpoint: String,
    out: Option<PathBuf>,
    ipc_base: Option<PathBuf>,
    stream: bool,
) -> anyhow::Result<()> {
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    if stream {
        let model_owned = model.clone();
        let sys = "你是 fusion-design UI 生成器。根据用户描述，\
输出严格 JSON：{\"page\":{...}}。只输出 JSON。";
        let user_msg = format!("描述：{prompt}\n生成页面「{page}」对应的 UI 布局。");
        let s =
            fd_ai_adapter::chat_stream(client, model_owned, sys.to_string(), user_msg, 2048).await;
        use futures::StreamExt;
        futures::pin_mut!(s);
        let mut stream_failed: Option<String> = None;
        while let Some(delta) = s.next().await {
            match delta {
                Ok(d) if d.finished => break,
                Ok(d) => print!("{}", d.token),
                Err(e) => {
                    eprintln!("流式输出错误: {e}");
                    stream_failed = Some(e.to_string());
                    break;
                }
            }
        }
        println!();
        if let Some(e) = stream_failed {
            anyhow::bail!("generate 流式失败: {e}");
        }
        return Ok(());
    }
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    let doc = skills.text_to_ui_async(&prompt, &page).await?;
    let json = serde_json::to_string_pretty(&doc)?;
    if let Some(base) = ipc_base {
        let link = fd_ecosystem::EcosystemLink::new(&base);
        let msg = fd_ecosystem::LinkMessage {
            target: fd_ecosystem::EcosystemTarget::FusionCLI,
            action: "generate-done".into(),
            payload: serde_json::json!({
                "page": page,
                "document": json,
            }),
        };
        link.send(&msg)?;
        tracing::info!("generate: IPC 消息已发送");
        if let Some(p) = out {
            std::fs::write(&p, &json)?;
            println!("已生成 PenDocument JSON 到 {p:?}");
        } else {
            println!("{json}");
        }
    } else {
        match out {
            Some(p) => {
                std::fs::write(&p, &json)?;
                println!("已生成 PenDocument JSON 到 {p:?}");
            }
            None => println!("{json}"),
        }
    }
    Ok(())
}
