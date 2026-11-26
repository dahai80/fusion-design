//! A-3: Generate subcommand handler (split out of main.rs).
//!
//! Text-to-UI: natural language → PenDocument JSON. Supports streaming SSE tokens and
//! fd-ecosystem IPC.

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
        let sys = "You are the fusion-design UI generator. Based on the user's description, \
output strict JSON: {\"page\":{...}}. Output JSON only.";
        let user_msg =
            format!("Description: {prompt}\nGenerate the UI layout for page \"{page}\".");
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
                    eprintln!("streaming output error: {e}");
                    stream_failed = Some(e.to_string());
                    break;
                }
            }
        }
        println!();
        if let Some(e) = stream_failed {
            anyhow::bail!("generate streaming failed: {e}");
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
        tracing::info!("generate: IPC message sent");
        if let Some(p) = out {
            std::fs::write(&p, &json)?;
            println!("PenDocument JSON written to {p:?}");
        } else {
            println!("{json}");
        }
    } else {
        match out {
            Some(p) => {
                std::fs::write(&p, &json)?;
                println!("PenDocument JSON written to {p:?}");
            }
            None => println!("{json}"),
        }
    }
    Ok(())
}
