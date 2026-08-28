//! A-3：AI 技能子命令 handler（ImageToUi/MultiVariants/SpecDoc/PageFlow/Train）。

use crate::common::{pick_multi_variant_styles, read_file_capped};

pub async fn image_to_ui(
    sketch: std::path::PathBuf,
    hint: String,
    page: String,
    model: String,
    endpoint: String,
    out: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    let doc = skills
        .image_to_ui_async(&sketch.to_string_lossy(), &hint, &page)
        .await?;
    let json = serde_json::to_string_pretty(&doc)?;
    match out {
        Some(p) => {
            tokio::fs::write(&p, &json).await?;
            println!("已生成图生 UI PenDocument JSON 到 {p:?}");
        }
        None => println!("{json}"),
    }
    Ok(())
}

pub async fn multi_variants(
    prompt: String,
    page: String,
    styles: Option<Vec<String>>,
    model: String,
    endpoint: String,
    out: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    let picked = pick_multi_variant_styles(styles);
    let docs = skills
        .multi_variants_async(
            &prompt,
            &page,
            [picked[0].as_str(), picked[1].as_str(), picked[2].as_str()],
        )
        .await?;
    let json = serde_json::to_string_pretty(&docs)?;
    match out {
        Some(p) => {
            tokio::fs::write(&p, &json).await?;
            println!("已生成 3 套多方案 PenDocument JSON 到 {p:?}");
        }
        None => println!("{json}"),
    }
    Ok(())
}

pub async fn spec_doc(
    input: std::path::PathBuf,
    title: String,
    model: String,
    endpoint: String,
    out: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let doc_json = read_file_capped(&input)?;
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    let spec = skills.spec_doc_async(&doc_json, &title).await?;
    let json = serde_json::to_string_pretty(&spec)?;
    match out {
        Some(p) => {
            tokio::fs::write(&p, &json).await?;
            println!("已生成设计规范文档到 {p:?}");
        }
        None => println!("{json}"),
    }
    Ok(())
}

pub async fn page_flow(
    flow: String,
    style_hint: String,
    model: String,
    endpoint: String,
    out: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    let docs = skills.page_flow_async(&flow, &style_hint).await?;
    let json = serde_json::to_string_pretty(&docs)?;
    match out {
        Some(p) => {
            tokio::fs::write(&p, &json).await?;
            println!("已生成 PageFlow 多页面文档到 {p:?}");
        }
        None => println!("{json}"),
    }
    Ok(())
}

pub async fn train(
    dataset: std::path::PathBuf,
    model: String,
    method: String,
    config: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let trainer = fd_ecosystem::TrainerClient::new();
    let status = match method.as_str() {
        "grpo" => trainer.run_rlsl("grpo", &dataset, &model, config.as_deref())?,
        "sft" => trainer.run_sft(&dataset, &model, config.as_deref())?,
        other => anyhow::bail!("不支持的 --method: {other} (仅 sft|grpo)"),
    };
    if !status.success() {
        anyhow::bail!("fusion-trainer 退出码 {}", status.code().unwrap_or(-1));
    }
    Ok(())
}
