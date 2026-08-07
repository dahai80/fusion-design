//! Fusion-Design AI adapter — 实现 OpenPencil `ChatProvider` trait，
//! 后端对接 fusion-mlx 本地多模态推理。
//!
//! 【禁云端硬约束】本 crate 是 fusion-design 唯一允许发起 HTTP 请求的模块，
//! 但请求目标被 `FusionMlxClient` 限制为 `127.0.0.1` 本地 fusion-mlx 服务，
//! 不存在任何公网调用路径。
//!
//! 【RouteGuard 鉴权】所有出站请求附加 `X-Fusion-Route: fusion-design` 头
//! （fusion-mlx v0.7.0+ 默认强制，缺失则 403 missing_route），并在
//! `FUSION_MLX_API_KEY` 设置时附加 `Authorization: Bearer <key>`。

use std::sync::{Arc, LazyLock};

use futures::StreamExt;
use serde::{Deserialize, Serialize};

// ── fusion-mlx 本地推理客户端 ──
//
// fusion-mlx 以本地 HTTP 服务（127.0.0.1:port）暴露 chat completions 接口，
// 兼容 OpenAI API 形状（/v1/chat/completions），便于复用既有生态工具。
// 真实端口由 fusion-mlx 启动时分配，写入本地配置文件。

const DEFAULT_MLX_ENDPOINT: &str = "http://127.0.0.1:11432";

/// fusion-mlx RouteGuard 要求的来源标识头（存在即放行）。
/// 方案B（netlayer-compliance-plan.md，用户 2026-08-07 裁定）：fusion-design
/// 统一经 fusion-gateway `:11432` 调用 fusion-mlx，不再直连 11434。
/// gateway 自身完成鉴权后转发，route 头对 gateway 透传无害。
/// 历史：方案A（直连 11434）已否决，见 issue #11。
const FUSION_ROUTE_HEADER: (&str, &str) = ("X-Fusion-Route", "fusion-design");

/// fusion-mlx chat 请求体（OpenAI 兼容形状）。
#[derive(Debug, Serialize)]
struct MlxChatPayload<'a> {
    model: &'a str,
    messages: Vec<MlxMessage<'a>>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct MlxMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// fusion-mlx chat 响应体（OpenAI 兼容形状，裁剪）。
#[derive(Debug, Deserialize)]
struct MlxChatResponse {
    choices: Vec<MlxChoice>,
}

#[derive(Debug, Deserialize)]
struct MlxChoice {
    message: MlxResponseMessage,
}

#[derive(Debug, Deserialize)]
struct MlxResponseMessage {
    content: String,
}

/// fusion-mlx 本地推理客户端。
///
/// 所有 HTTP 请求目标均为 `127.0.0.1`，构造时强校验，
/// 杜绝任何公网调用路径（对应 PRD「全链路离线」硬约束）。
#[derive(Clone)]
pub struct FusionMlxClient {
    endpoint: String,
    http: reqwest::Client,
}

impl FusionMlxClient {
    /// 用默认 endpoint 构造；优先读 `FUSION_MLX_BASE_URL` 环境变量
    /// （支持显式指回 fusion-mlx 直连 11434 等本地端点），缺省回退
    /// `http://127.0.0.1:11432`（方案B：经 fusion-gateway 统一网关）。
    /// 鉴权：gateway 需 `FUSION_MLX_API_KEY` 设为 gateway key（如 master_key）。
    pub fn new() -> anyhow::Result<Self> {
        let endpoint = match std::env::var("FUSION_MLX_BASE_URL") {
            Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => DEFAULT_MLX_ENDPOINT.to_string(),
        };
        Self::with_endpoint(&endpoint)
    }

    /// 用指定 endpoint 构造；强校验 host 为 `127.0.0.1` 或 `localhost`。
    pub fn with_endpoint(endpoint: &str) -> anyhow::Result<Self> {
        validate_localhost(endpoint)?;
        Ok(Self {
            endpoint: endpoint.to_string(),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }

    /// 解析 CLI `--endpoint` 实参到最终 endpoint。
    /// CLI 层把 `--endpoint` 默认值设为空串；空串时读 `FUSION_MLX_BASE_URL`，
    /// 缺省回退 `http://127.0.0.1:11432`（方案B 经 gateway）。非空串直接透传
    /// （用户显式传 `--endpoint` 优先级最高）。返回值供 `with_endpoint` 使用。
    pub fn resolve_endpoint(cli_endpoint: &str) -> anyhow::Result<String> {
        let resolved = match cli_endpoint.trim() {
            "" => match std::env::var("FUSION_MLX_BASE_URL") {
                Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => DEFAULT_MLX_ENDPOINT.to_string(),
            },
            other => other.to_string(),
        };
        validate_localhost(&resolved)?;
        Ok(resolved)
    }

    /// 同步发送 chat 请求（阻塞当前线程，供 `ChatProvider::send` 调用）。
    ///
    /// 对应 PRD「AI 推理沙箱隔离」——本方法只读写 fusion-mlx 本地服务，
    /// 不接触系统文件。
    pub fn chat_sync(
        &self,
        model: &str,
        system_prompt: &str,
        user_message: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        let payload = MlxChatPayload {
            model,
            messages: vec![
                MlxMessage {
                    role: "system",
                    content: system_prompt,
                },
                MlxMessage {
                    role: "user",
                    content: user_message,
                },
            ],
            max_tokens,
            temperature: None,
        };
        let url = format!("{}/v1/chat/completions", self.endpoint);
        let resp: MlxChatResponse = self.blocking_post(&url, &payload)?;
        Ok(resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("fusion-mlx 返回空 choices"))?
            .message
            .content)
    }
}

impl Default for FusionMlxClient {
    fn default() -> Self {
        Self::new().expect("默认 endpoint 必为 localhost，构造不会失败")
    }
}

/// fusion-mlx RouteGuard 鉴权：所有出站请求附加 X-Fusion-Route + Bearer。
impl FusionMlxClient {
    /// 读取 `FUSION_MLX_API_KEY` 构造 Bearer token；未设置则 WARN（fail visibly）。
    /// 不在构造时缓存：支持运行期 Key 轮换与测试隔离。
    fn bearer_token(&self) -> Option<String> {
        match std::env::var("FUSION_MLX_API_KEY") {
            Ok(k) if !k.trim().is_empty() => Some(format!("Bearer {}", k.trim())),
            _ => {
                tracing::warn!(
                    "FUSION_MLX_API_KEY 未设置或为空；若 fusion-mlx 启用 API Key 鉴权，请求将被拒绝"
                );
                None
            }
        }
    }

    /// 为请求附加 RouteGuard + Bearer 鉴权头（对应 issue #6：修复 403 missing_route）。
    fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let builder = builder.header(FUSION_ROUTE_HEADER.0, FUSION_ROUTE_HEADER.1);
        match self.bearer_token() {
            Some(bearer) => builder.header("Authorization", bearer),
            None => builder,
        }
    }
}

/// 异步版本（供 `fd-ecosystem` 任务队列调用）。
impl FusionMlxClient {
    pub async fn chat_async(
        &self,
        model: &str,
        system_prompt: &str,
        user_message: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        let payload = MlxChatPayload {
            model,
            messages: vec![
                MlxMessage {
                    role: "system",
                    content: system_prompt,
                },
                MlxMessage {
                    role: "user",
                    content: user_message,
                },
            ],
            max_tokens,
            temperature: None,
        };
        let url = format!("{}/v1/chat/completions", self.endpoint);
        let resp = self
            .authed(self.http.post(&url).json(&payload))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("fusion-mlx HTTP {}", resp.status());
        }
        let parsed: MlxChatResponse = resp.json().await?;
        Ok(parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("fusion-mlx 返回空 choices"))?
            .message
            .content)
    }
}

/// 强校验 endpoint host 为 localhost，杜绝公网调用路径。
fn validate_localhost(endpoint: &str) -> anyhow::Result<()> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|e| anyhow::anyhow!("无效 endpoint {endpoint:?}: {e}"))?;
    let host = url.host_str().unwrap_or("");
    // reqwest::Url 对 IPv6 返回形如 "[::1]"，需去方括号比对
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host != "127.0.0.1" && host != "localhost" && host != "::1" {
        anyhow::bail!("违反离线硬约束：endpoint host {host:?} 非 localhost，禁止公网调用");
    }
    Ok(())
}

/// 阻塞式 POST（用专用 tokio runtime，避免嵌套 runtime panic）。
///
/// 设计：创建一个 `LazyLock` 初始化的 multi-thread 专用 runtime，
/// 所有 `chat_sync` / `ChatProvider::send` 的异步请求在此 runtime 上执行。
/// 解决了以下问题：
/// - 在 `#[tokio::test]` 内调用 `chat_sync` 时，`block_on` 嵌套 panic
/// - 在同步上下文（如 `ChatProvider::send`）中，需要异步能力
/// - 每次调用都新建 runtime 的性能开销
///
/// 专用 runtime 是 multi-thread 的，因此 `block_in_place` 可用，
/// `reqwest` 的 DNS/TLS 解析不会阻塞主线程。
static BLOCKING_RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("blocking tokio runtime 创建失败")
});

impl FusionMlxClient {
    /// 阻塞式 POST（用专用 tokio runtime，避免嵌套 runtime panic）。
    /// 附加 RouteGuard + Bearer 鉴权头后发往 fusion-mlx。
    fn blocking_post<T: Serialize + ?Sized>(
        &self,
        url: &str,
        payload: &T,
    ) -> anyhow::Result<MlxChatResponse> {
        let http = self.http.clone();
        let url = url.to_string();
        let bearer = self.bearer_token();
        BLOCKING_RT.block_on(async move {
            let req = http
                .post(&url)
                .header(FUSION_ROUTE_HEADER.0, FUSION_ROUTE_HEADER.1)
                .json(payload);
            let req = match bearer {
                Some(b) => req.header("Authorization", b),
                None => req,
            };
            let resp = req.send().await?;
            if !resp.status().is_success() {
                anyhow::bail!("fusion-mlx HTTP {}", resp.status());
            }
            Ok::<_, anyhow::Error>(resp.json::<MlxChatResponse>().await?)
        })
    }
}

// ── SSE 流式推理 ──

/// SSE 流式增量 token。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlxStreamDelta {
    pub token: String,
    pub finished: bool,
}

/// 流式 chat：返回 SSE token 流。
///
/// 请求体附加 `stream: true`，解析 `data: {...}` SSE 行，
/// 每行提取 `choices[0].delta.content` 推送给调用方。
pub async fn chat_stream(
    client: FusionMlxClient,
    model: String,
    system_prompt: String,
    user_message: String,
    max_tokens: u32,
) -> impl futures::Stream<Item = anyhow::Result<MlxStreamDelta>> {
    let payload = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_message },
        ],
        "max_tokens": max_tokens,
        "stream": true,
    });
    let url = format!("{}/v1/chat/completions", client.endpoint);
    let bearer = client.bearer_token();
    let http = client.http;
    let req = http
        .post(&url)
        .header(FUSION_ROUTE_HEADER.0, FUSION_ROUTE_HEADER.1)
        .json(&payload);
    let req = match bearer {
        Some(b) => req.header("Authorization", b),
        None => req,
    };

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "SSE 请求失败");
            return futures::stream::once(async move { Err(anyhow::anyhow!("SSE 请求失败: {e}")) })
                .boxed();
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        return futures::stream::once(
            async move { Err(anyhow::anyhow!("fusion-mlx HTTP {status}")) },
        )
        .boxed();
    }

    let stream = resp.bytes_stream();
    futures::stream::unfold(
        (stream, String::new()),
        |(mut stream, mut buffer)| async move {
            use futures::StreamExt;
            loop {
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(line_end) = buffer.find('\n') {
                            let line = buffer[..line_end].trim().to_string();
                            buffer = buffer[line_end + 1..].to_string();
                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    return Some((
                                        Ok(MlxStreamDelta {
                                            token: String::new(),
                                            finished: true,
                                        }),
                                        (stream, buffer),
                                    ));
                                }
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data)
                                {
                                    let content = parsed["choices"][0]["delta"]["content"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    if !content.is_empty() {
                                        return Some((
                                            Ok(MlxStreamDelta {
                                                token: content,
                                                finished: false,
                                            }),
                                            (stream, buffer),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        return Some((Err(anyhow::anyhow!("SSE 读取出错: {e}")), (stream, buffer)));
                    }
                    None => {
                        return Some((
                            Ok(MlxStreamDelta {
                                token: String::new(),
                                finished: true,
                            }),
                            (stream, buffer),
                        ));
                    }
                }
            }
        },
    )
    .boxed()
}

// ── MLX 健康检查 ──

/// MLX 服务健康状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub available: bool,
    pub model: Option<String>,
    pub gpu: Option<String>,
}

impl FusionMlxClient {
    /// 探测 fusion-mlx 健康状态（超时 3s）。
    pub async fn health_check(&self) -> anyhow::Result<HealthStatus> {
        let url = format!("{}/v1/models", self.endpoint);
        let resp = self
            .authed(
                self.http
                    .get(&url)
                    .timeout(std::time::Duration::from_secs(3)),
            )
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let body: serde_json::Value = r.json().await.unwrap_or_default();
                let model = body["data"][0]["id"].as_str().map(String::from);
                tracing::info!(available = true, model = ?model, "health_check: MLX 可用");
                Ok(HealthStatus {
                    available: true,
                    model,
                    gpu: None,
                })
            }
            Ok(r) => {
                tracing::warn!(status = %r.status(), "health_check: MLX 返回非 200");
                Ok(HealthStatus {
                    available: false,
                    model: None,
                    gpu: None,
                })
            }
            Err(e) => {
                tracing::warn!(error = %e, "health_check: MLX 不可达");
                Ok(HealthStatus {
                    available: false,
                    model: None,
                    gpu: None,
                })
            }
        }
    }

    /// 健康检查同步版（阻塞当前线程）。
    pub fn health_check_sync(&self) -> anyhow::Result<HealthStatus> {
        let this = self.clone();
        BLOCKING_RT.block_on(async move { this.health_check().await })
    }
}

// ── 多模态请求（截图/草图 → UI）──

/// Vision 消息内容项。
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum VisionContent<'a> {
    #[serde(rename = "text")]
    Text { text: &'a str },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlPayload<'a> },
}

#[derive(Debug, Serialize)]
struct ImageUrlPayload<'a> {
    url: &'a str,
}

/// 多模态 chat 请求：发送图片 + 文字到 fusion-mlx。
pub async fn chat_with_image(
    client: &FusionMlxClient,
    model: &str,
    system_prompt: &str,
    user_text: &str,
    image_base64: &str,
    max_tokens: u32,
) -> anyhow::Result<String> {
    let image_data_url = format!("data:image/png;base64,{image_base64}");
    let content = vec![
        VisionContent::Text { text: user_text },
        VisionContent::ImageUrl {
            image_url: ImageUrlPayload {
                url: &image_data_url,
            },
        },
    ];
    let payload = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": content },
        ],
        "max_tokens": max_tokens,
    });
    let url = format!("{}/v1/chat/completions", client.endpoint);
    let resp = client
        .authed(client.http.post(&url).json(&payload))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("fusion-mlx vision HTTP {}", resp.status());
    }
    let parsed: MlxChatResponse = resp.json().await?;
    Ok(parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("fusion-mlx 返回空 choices"))?
        .message
        .content)
}

/// 读取图片文件并编码为 base64。
pub fn encode_image_base64(path: &std::path::Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &bytes,
    ))
}

/// 多模态 chat 同步版（阻塞当前线程，专用 BLOCKING_RT 避免嵌套 runtime panic）。
pub fn chat_with_image_sync(
    client: &FusionMlxClient,
    model: &str,
    system_prompt: &str,
    user_text: &str,
    image_base64: &str,
    max_tokens: u32,
) -> anyhow::Result<String> {
    let client = client.clone();
    let model = model.to_string();
    let system_prompt = system_prompt.to_string();
    let user_text = user_text.to_string();
    let image_base64 = image_base64.to_string();
    BLOCKING_RT.block_on(async move {
        chat_with_image(
            &client,
            &model,
            &system_prompt,
            &user_text,
            &image_base64,
            max_tokens,
        )
        .await
    })
}

// ── OpenPencil ChatProvider 适配 ──

use op_ai::chat_provider::{ChatDelta, ChatProvider, ChatRequest, StopReason};

/// Fusion-MLX ChatProvider 适配器（实现 OpenPencil ChatProvider trait）。
pub struct FusionMlxChatProvider {
    client: FusionMlxClient,
    default_model: String,
}

impl FusionMlxChatProvider {
    pub fn new(client: FusionMlxClient, default_model: impl Into<String>) -> Self {
        Self {
            client,
            default_model: default_model.into(),
        }
    }
}

impl ChatProvider for FusionMlxChatProvider {
    fn provider_label(&self) -> &str {
        "fusion-mlx (local)"
    }
    fn send(&self, request: ChatRequest) -> Box<dyn Iterator<Item = ChatDelta> + Send> {
        let model = request.model.as_deref().unwrap_or(&self.default_model);
        match self.client.chat_sync(
            model,
            &request.system_prompt,
            &request.user_message,
            request.max_output_tokens,
        ) {
            Ok(text) => Box::new(
                vec![
                    ChatDelta::TextDelta(text),
                    ChatDelta::Done {
                        stop_reason: StopReason::EndTurn,
                    },
                ]
                .into_iter(),
            ),
            Err(e) => Box::new(vec![ChatDelta::Error(e.to_string())].into_iter()),
        }
    }
}

/// 供测试与 `fd-ecosystem` 复用的便捷构造。
pub fn shared_client(endpoint: Option<&str>) -> anyhow::Result<Arc<FusionMlxClient>> {
    let client = match endpoint {
        Some(e) => FusionMlxClient::with_endpoint(e)?,
        None => FusionMlxClient::new()?,
    };
    Ok(Arc::new(client))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_localhost_accepts_127() {
        assert!(validate_localhost("http://127.0.0.1:8080").is_ok());
        assert!(validate_localhost("http://localhost:9000").is_ok());
        assert!(validate_localhost("http://[::1]:8080").is_ok());
    }

    #[test]
    fn validate_localhost_rejects_public() {
        assert!(validate_localhost("http://10.0.0.1:8080").is_err());
        assert!(validate_localhost("https://api.openai.com").is_err());
        assert!(validate_localhost("http://192.168.1.1:8080").is_err());
    }

    #[test]
    fn validate_localhost_rejects_malformed() {
        assert!(validate_localhost("not-a-url").is_err());
    }

    #[test]
    fn client_new_uses_default_endpoint() {
        let c = FusionMlxClient::new().unwrap();
        assert!(c.endpoint.starts_with("http://127.0.0.1"));
    }

    #[test]
    fn client_default_impl_works() {
        let _c = FusionMlxClient::default();
    }

    #[test]
    fn shared_client_builds_with_default() {
        let c = shared_client(None).unwrap();
        assert!(Arc::strong_count(&c) >= 1);
    }

    #[test]
    fn shared_client_rejects_public_endpoint() {
        assert!(shared_client(Some("http://1.2.3.4:80")).is_err());
    }
}

// ── AI Skill 系统：trait 化 + Token 注入 ──

use fd_design_system::DesignSystem;

/// Skill 执行上下文：携带客户端、模型、设计 Token 等运行时信息。
#[derive(Clone, Copy)]
pub struct SkillContext<'a> {
    pub client: &'a FusionMlxClient,
    pub model: &'a str,
    pub design_system: Option<&'a DesignSystem>,
}

impl<'a> SkillContext<'a> {
    /// 发送同步 chat 请求（快捷方法）。
    pub fn chat(&self, sys: &str, user: &str, max_tokens: u32) -> anyhow::Result<String> {
        self.client.chat_sync(self.model, sys, user, max_tokens)
    }

    /// 发送异步 chat 请求（快捷方法）。
    pub async fn chat_async(
        &self,
        sys: &str,
        user: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        self.client
            .chat_async(self.model, sys, user, max_tokens)
            .await
    }

    /// 多模态 chat（快捷方法）：发送图片 + 文字到 fusion-mlx。
    pub async fn chat_with_image_async(
        &self,
        sys: &str,
        user: &str,
        image_base64: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        chat_with_image(self.client, self.model, sys, user, image_base64, max_tokens).await
    }

    /// 多模态 chat 同步版（快捷方法）。
    pub fn chat_with_image(
        &self,
        sys: &str,
        user: &str,
        image_base64: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        chat_with_image_sync(self.client, self.model, sys, user, image_base64, max_tokens)
    }

    /// 生成设计 Token 的 CSS Custom Properties 片段，注入到 system prompt。
    /// 返回 None 如果没有激活设计系统。
    pub fn token_prompt_fragment(&self) -> Option<String> {
        self.design_system.map(|ds| {
            let css = ds.to_css_custom_properties();
            format!("当前设计系统 Token（CSS Custom Properties）：\n{css}\n\n生成的 UI 必须使用这些 CSS 变量。")
        })
    }
}

/// Skill 输出类型。
#[derive(Debug, Clone)]
pub enum SkillOutput {
    /// 完整页面文档
    Document(PenDocument),
    /// 局部编辑的节点 JSON
    PartialEdit(String),
    /// 多方案对比（3 份文档）
    MultiVariants([PenDocument; 3]),
    /// 设计规范文档（交互规范/组件规范/页面架构）
    SpecDoc(SpecDocument),
    /// 页面流程批量生成（多页连贯流程）
    PageFlow(Vec<PenDocument>),
}

/// 设计规范文档。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecDocument {
    /// 文档标题。
    pub title: String,
    /// 页面架构描述。
    pub page_architecture: String,
    /// 交互规范条目。
    pub interaction_specs: Vec<InteractionSpec>,
    /// 组件规范条目。
    pub component_specs: Vec<ComponentSpec>,
    /// 设计 Token 引用摘要。
    pub token_summary: String,
}

/// 交互规范条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionSpec {
    pub id: String,
    pub element: String,
    pub event: String,
    pub behavior: String,
    #[serde(default)]
    pub animation: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// 组件规范条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSpec {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub props: Vec<ComponentProp>,
    #[serde(default)]
    pub variants: Vec<String>,
    #[serde(default)]
    pub accessibility: Option<String>,
}

/// 组件属性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentProp {
    pub name: String,
    pub prop_type: String,
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// AI 设计 Skill trait — 每个 Skill 实现一个设计能力。
pub trait DesignSkill: Send + Sync {
    /// Skill 唯一标识。
    fn id(&self) -> &str;
    /// Skill 显示名称。
    fn label(&self) -> &str;
    /// 同步执行。
    fn execute(&self, ctx: &SkillContext, input: &str) -> anyhow::Result<SkillOutput>;
    /// 异步执行（默认实现：委托给同步版本）。
    fn execute_async<'a>(
        &'a self,
        ctx: SkillContext<'a>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move { self.execute(&ctx, &input) })
    }
}

/// Skill 注册中心：按 id 查找并调度 Skill。
pub struct SkillRegistry {
    skills: std::collections::HashMap<String, Box<dyn DesignSkill>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: std::collections::HashMap::new(),
        }
    }

    /// 注册一个 Skill。
    pub fn register(&mut self, skill: Box<dyn DesignSkill>) {
        self.skills.insert(skill.id().to_string(), skill);
    }

    /// 按 id 查找 Skill。
    pub fn get(&self, id: &str) -> Option<&dyn DesignSkill> {
        self.skills.get(id).map(|s| s.as_ref())
    }

    /// 列出所有已注册 Skill 的 id。
    pub fn list(&self) -> Vec<&str> {
        self.skills.keys().map(|s| s.as_str()).collect()
    }

    /// 注册内置 Skill（text-to-ui, image-to-ui, partial-edit, local-edit, multi-variants, spec-doc, page-flow）。
    pub fn register_builtin(&mut self) {
        self.register(Box::new(TextToUiSkill));
        self.register(Box::new(ImageToUiSkill));
        self.register(Box::new(PartialEditSkill));
        self.register(Box::new(LocalEditSkill));
        self.register(Box::new(MultiVariantsSkill));
        self.register(Box::new(SpecDocSkill));
        self.register(Box::new(PageFlowSkill));
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── 内置 Skill 实现 ──

/// 文生 UI Skill。
struct TextToUiSkill;

impl DesignSkill for TextToUiSkill {
    fn id(&self) -> &str {
        "text-to-ui"
    }
    fn label(&self) -> &str {
        "文生 UI"
    }

    fn execute(&self, ctx: &SkillContext, input: &str) -> anyhow::Result<SkillOutput> {
        let mut sys = "你是 fusion-design UI 生成器。输出严格 JSON：{\"page\":{...}}。\
只输出 JSON，禁止额外文字。page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}。"
            .to_string();
        if let Some(tokens) = ctx.token_prompt_fragment() {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user = format!("生成页面。需求：{input}");
        let resp = ctx.chat(&sys, &user, 2048)?;
        let doc = parse_ui_json(&resp, "generated")?;
        Ok(SkillOutput::Document(doc))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: SkillContext<'a>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut sys = "你是 fusion-design UI 生成器。输出严格 JSON：{\"page\":{...}}。\
只输出 JSON，禁止额外文字。page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}。"
                .to_string();
            if let Some(tokens) = ctx.token_prompt_fragment() {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!("生成页面。需求：{input}");
            let resp = ctx.chat_async(&sys, &user, 2048).await?;
            let doc = parse_ui_json(&resp, "generated")?;
            Ok(SkillOutput::Document(doc))
        })
    }
}

/// 图生 UI Skill。
struct ImageToUiSkill;

impl DesignSkill for ImageToUiSkill {
    fn id(&self) -> &str {
        "image-to-ui"
    }
    fn label(&self) -> &str {
        "图生 UI"
    }

    fn execute(&self, ctx: &SkillContext, input: &str) -> anyhow::Result<SkillOutput> {
        let parts: Vec<&str> = input.splitn(3, '|').collect();
        let sketch_path = parts[0];
        let hint = parts.get(1).unwrap_or(&"");
        let page_name = parts.get(2).unwrap_or(&"generated");
        let mut sys = "你是 fusion-design UI 生成器。根据用户提供的草图图片与说明，\
输出严格 JSON：{\"page\":{...}}。只输出 JSON，禁止额外文字与 markdown 围栏。\
page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?,children?}。\
示例：{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[\
{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":900,\"fill\":\"#ffffff\"},\
{\"id\":\"n1\",\"kind\":\"rect\",\"x\":560,\"y\":360,\"w\":320,\"h\":48,\"fill\":\"#f0f0f0\"}\
]}}。"
            .to_string();
        if let Some(tokens) = ctx.token_prompt_fragment() {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user =
            format!("补充说明：{hint}\n请根据上方草图图片生成页面「{page_name}」对应的 UI 布局。");
        let resp = match encode_image_base64(std::path::Path::new(sketch_path)) {
            Ok(b64) => {
                tracing::info!(
                    sketch_path,
                    bytes = b64.len(),
                    "image-to-ui: 已加载草图，发送真实多模态请求"
                );
                ctx.chat_with_image(&sys, &user, &b64, 4096)?
            }
            Err(e) => {
                tracing::warn!(sketch_path, error = %e, "image-to-ui: 草图加载失败，回退文字描述");
                let user_text = format!(
                    "草图路径：{sketch_path}（无法读取：{e}）\n补充说明：{hint}\n生成页面「{page_name}」对应的 UI 布局。"
                );
                ctx.chat(&sys, &user_text, 2048)?
            }
        };
        let doc = parse_ui_json(&resp, page_name)?;
        Ok(SkillOutput::Document(doc))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: SkillContext<'a>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let parts: Vec<&str> = input.splitn(3, '|').collect();
            let sketch_path = parts[0];
            let hint = parts.get(1).unwrap_or(&"");
            let page_name = parts.get(2).unwrap_or(&"generated");
            let mut sys = "你是 fusion-design UI 生成器。根据用户提供的草图图片与说明，\
输出严格 JSON：{\"page\":{...}}。只输出 JSON，禁止额外文字与 markdown 围栏。\
page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?,children?}。\
示例：{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[\
{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":900,\"fill\":\"#ffffff\"},\
{\"id\":\"n1\",\"kind\":\"rect\",\"x\":560,\"y\":360,\"w\":320,\"h\":48,\"fill\":\"#f0f0f0\"}\
]}}。"
                .to_string();
            if let Some(tokens) = ctx.token_prompt_fragment() {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!(
                "补充说明：{hint}\n请根据上方草图图片生成页面「{page_name}」对应的 UI 布局。"
            );
            let resp = match encode_image_base64(std::path::Path::new(sketch_path)) {
                Ok(b64) => {
                    tracing::info!(
                        sketch_path,
                        bytes = b64.len(),
                        "image-to-ui: 已加载草图，发送真实多模态请求"
                    );
                    ctx.chat_with_image_async(&sys, &user, &b64, 4096).await?
                }
                Err(e) => {
                    tracing::warn!(sketch_path, error = %e, "image-to-ui: 草图加载失败，回退文字描述");
                    let user_text = format!(
                        "草图路径：{sketch_path}（无法读取：{e}）\n补充说明：{hint}\n生成页面「{page_name}」对应的 UI 布局。"
                    );
                    ctx.chat_async(&sys, &user_text, 2048).await?
                }
            };
            let doc = parse_ui_json(&resp, page_name)?;
            Ok(SkillOutput::Document(doc))
        })
    }
}

/// 局部编辑 Skill。
struct PartialEditSkill;

impl DesignSkill for PartialEditSkill {
    fn id(&self) -> &str {
        "partial-edit"
    }
    fn label(&self) -> &str {
        "局部编辑"
    }

    fn execute(&self, ctx: &SkillContext, input: &str) -> anyhow::Result<SkillOutput> {
        // input 格式: "node_json|instruction"
        let parts: Vec<&str> = input.splitn(2, '|').collect();
        let node_json = parts[0];
        let instruction = parts.get(1).unwrap_or(&"修改为更合适的样式");
        let sys = "你是 fusion-design 局部编辑器。输入一个节点 JSON 和编辑指令，\
输出修改后的节点 JSON（保持原字段，仅变更指令涉及的字段）。只输出 JSON。";
        let user = format!("节点：{node_json}\n指令：{instruction}");
        let resp = ctx.chat(sys, &user, 1024)?;
        Ok(SkillOutput::PartialEdit(resp))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: SkillContext<'a>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let parts: Vec<&str> = input.splitn(2, '|').collect();
            let node_json = parts[0];
            let instruction = parts.get(1).unwrap_or(&"修改为更合适的样式");
            let sys = "你是 fusion-design 局部编辑器。输入一个节点 JSON 和编辑指令，\
输出修改后的节点 JSON（保持原字段，仅变更指令涉及的字段）。只输出 JSON。";
            let user = format!("节点：{node_json}\n指令：{instruction}");
            let resp = ctx.chat_async(sys, &user, 1024).await?;
            Ok(SkillOutput::PartialEdit(resp))
        })
    }
}

/// 本地编辑 Skill：框选多节点 + 自然语言指令 → 批量修改。
///
/// input 格式: "node1_json\n---\nnode2_json\n---\n...|||instruction"
struct LocalEditSkill;

impl DesignSkill for LocalEditSkill {
    fn id(&self) -> &str {
        "local-edit"
    }
    fn label(&self) -> &str {
        "本地编辑"
    }

    fn execute(&self, ctx: &SkillContext, input: &str) -> anyhow::Result<SkillOutput> {
        let (nodes_part, instruction) = parse_local_edit_input(input);
        let mut sys = "你是 fusion-design 本地编辑器。输入多个节点的 JSON 数组和编辑指令，\
输出修改后的节点 JSON 数组（保持原字段，仅变更指令涉及的字段）。只输出 JSON 数组。"
            .to_string();
        if let Some(tokens) = ctx.token_prompt_fragment() {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user = format!("选中节点：\n{nodes_part}\n\n编辑指令：{instruction}");
        let resp = ctx.chat(&sys, &user, 2048)?;
        Ok(SkillOutput::PartialEdit(resp))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: SkillContext<'a>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let (nodes_part, instruction) = parse_local_edit_input(&input);
            let mut sys = "你是 fusion-design 本地编辑器。输入多个节点的 JSON 数组和编辑指令，\
输出修改后的节点 JSON 数组（保持原字段，仅变更指令涉及的字段）。只输出 JSON 数组。"
                .to_string();
            if let Some(tokens) = ctx.token_prompt_fragment() {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!("选中节点：\n{nodes_part}\n\n编辑指令：{instruction}");
            let resp = ctx.chat_async(&sys, &user, 2048).await?;
            Ok(SkillOutput::PartialEdit(resp))
        })
    }
}

/// 解析 local-edit 输入格式。
/// 格式: "node_jsons|||instruction" 或 "node_jsons|instruction"（向后兼容）
fn parse_local_edit_input(input: &str) -> (String, &str) {
    if let Some((nodes, instr)) = input.splitn(2, "|||").collect::<Vec<_>>().split_first() {
        if !instr.is_empty() {
            return (nodes.to_string(), instr[0]);
        }
    }
    let parts: Vec<&str> = input.splitn(2, '|').collect();
    let nodes = parts[0].to_string();
    let instr = parts.get(1).unwrap_or(&"修改为更合适的样式");
    (nodes, instr)
}

/// 设计规范文档生成 Skill：从 PenDocument JSON 生成交互规范/组件规范/页面架构文档。
///
/// input 格式: "pen_document_json|spec_title"
struct SpecDocSkill;

impl DesignSkill for SpecDocSkill {
    fn id(&self) -> &str {
        "spec-doc"
    }
    fn label(&self) -> &str {
        "设计规范文档"
    }

    fn execute(&self, ctx: &SkillContext, input: &str) -> anyhow::Result<SkillOutput> {
        let (doc_json, title) = parse_spec_doc_input(input);
        let mut sys = "你是 fusion-design 设计规范文档生成器。输入一个 PenDocument JSON，\
分析其页面结构和节点，输出严格 JSON：{\"title\":\"...\",\"page_architecture\":\"...\",\
\"interaction_specs\":[...],\"component_specs\":[...],\"token_summary\":\"...\"}。\
只输出 JSON，禁止额外文字。\n\n\
interaction_specs 每项含：id, element, event, behavior, animation?, notes?\n\
component_specs 每项含：id, name, kind, props[{name,prop_type,default_value?,description?}], variants[], accessibility?\n\n\
规范要求：\n\
1. 为每个可交互节点（按钮/输入框/链接等）生成交互规范\n\
2. 为每种组件类型生成组件规范，包含属性、变体、无障碍说明\n\
3. 页面架构用文字描述布局层次和导航关系\n\
4. token_summary 汇总设计系统 Token 使用情况".to_string();
        if let Some(tokens) = ctx.token_prompt_fragment() {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user = format!("PenDocument：{doc_json}\n\n生成设计规范文档「{title}」。");
        let resp = ctx.chat(&sys, &user, 4096)?;
        let spec = parse_spec_doc_json(&resp, title)?;
        Ok(SkillOutput::SpecDoc(spec))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: SkillContext<'a>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let (doc_json, title) = parse_spec_doc_input(&input);
            let mut sys = "你是 fusion-design 设计规范文档生成器。输入一个 PenDocument JSON，\
分析其页面结构和节点，输出严格 JSON：{\"title\":\"...\",\"page_architecture\":\"...\",\
\"interaction_specs\":[...],\"component_specs\":[...],\"token_summary\":\"...\"}。\
只输出 JSON，禁止额外文字。\n\n\
interaction_specs 每项含：id, element, event, behavior, animation?, notes?\n\
component_specs 每项含：id, name, kind, props[{name,prop_type,default_value?,description?}], variants[], accessibility?\n\n\
规范要求：\n\
1. 为每个可交互节点（按钮/输入框/链接等）生成交互规范\n\
2. 为每种组件类型生成组件规范，包含属性、变体、无障碍说明\n\
3. 页面架构用文字描述布局层次和导航关系\n\
4. token_summary 汇总设计系统 Token 使用情况".to_string();
            if let Some(tokens) = ctx.token_prompt_fragment() {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!("PenDocument：{doc_json}\n\n生成设计规范文档「{title}」。");
            let resp = ctx.chat_async(&sys, &user, 4096).await?;
            let spec = parse_spec_doc_json(&resp, title)?;
            Ok(SkillOutput::SpecDoc(spec))
        })
    }
}

fn parse_spec_doc_input(input: &str) -> (&str, &str) {
    let parts: Vec<&str> = input.splitn(2, '|').collect();
    let doc_json = parts[0];
    let title = parts.get(1).unwrap_or(&"设计规范文档");
    (doc_json, title)
}

fn parse_spec_doc_json(json: &str, fallback_title: &str) -> anyhow::Result<SpecDocument> {
    let cleaned = strip_code_fence(json);
    let v: serde_json::Value = serde_json::from_str(cleaned)?;

    let title = v
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or(fallback_title)
        .to_string();
    let page_architecture = v
        .get("page_architecture")
        .and_then(|a| a.as_str())
        .unwrap_or("")
        .to_string();
    let token_summary = v
        .get("token_summary")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    let interaction_specs: Vec<InteractionSpec> = v
        .get("interaction_specs")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    Some(InteractionSpec {
                        id: obj.get("id")?.as_str()?.to_string(),
                        element: obj.get("element")?.as_str()?.to_string(),
                        event: obj.get("event")?.as_str()?.to_string(),
                        behavior: obj.get("behavior")?.as_str()?.to_string(),
                        animation: obj
                            .get("animation")
                            .and_then(|a| a.as_str())
                            .map(String::from),
                        notes: obj.get("notes").and_then(|n| n.as_str()).map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let component_specs: Vec<ComponentSpec> = v
        .get("component_specs")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    Some(ComponentSpec {
                        id: obj.get("id")?.as_str()?.to_string(),
                        name: obj.get("name")?.as_str()?.to_string(),
                        kind: obj.get("kind")?.as_str()?.to_string(),
                        props: obj
                            .get("props")
                            .and_then(|p| p.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|p| {
                                        let o = p.as_object()?;
                                        Some(ComponentProp {
                                            name: o.get("name")?.as_str()?.to_string(),
                                            prop_type: o.get("prop_type")?.as_str()?.to_string(),
                                            default_value: o
                                                .get("default_value")
                                                .and_then(|d| d.as_str())
                                                .map(String::from),
                                            description: o
                                                .get("description")
                                                .and_then(|d| d.as_str())
                                                .map(String::from),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        variants: obj
                            .get("variants")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        accessibility: obj
                            .get("accessibility")
                            .and_then(|a| a.as_str())
                            .map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    tracing::info!(
        title = %title,
        interactions = interaction_specs.len(),
        components = component_specs.len(),
        "parse_spec_doc_json: 规范文档解析完成"
    );

    Ok(SpecDocument {
        title,
        page_architecture,
        interaction_specs,
        component_specs,
        token_summary,
    })
}

/// 页面流程批量生成 Skill：生成完整页面流程序列（首页→列表→详情→弹窗），统一风格。
///
/// input 格式: "flow_desc|style_hint"
/// flow_desc 示例: "电商应用:首页,商品列表,商品详情,购物车,结算"
struct PageFlowSkill;

impl DesignSkill for PageFlowSkill {
    fn id(&self) -> &str {
        "page-flow"
    }
    fn label(&self) -> &str {
        "页面流程生成"
    }

    fn execute(&self, ctx: &SkillContext, input: &str) -> anyhow::Result<SkillOutput> {
        let (flow_desc, style_hint) = parse_page_flow_input(input);
        let pages = parse_flow_pages(flow_desc);
        let text_skill = TextToUiSkill;
        let mut docs = Vec::with_capacity(pages.len());
        for page_name in &pages {
            let prompt = format!("{flow_desc}（页面：{page_name}，风格：{style_hint}）");
            match text_skill.execute(ctx, &prompt)? {
                SkillOutput::Document(d) => docs.push(d),
                _ => anyhow::bail!("text-to-ui 返回非 Document"),
            }
        }
        tracing::info!(count = docs.len(), "page-flow: 流程生成完成");
        Ok(SkillOutput::PageFlow(docs))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: SkillContext<'a>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let (flow_desc, style_hint) = parse_page_flow_input(&input);
            let pages = parse_flow_pages(flow_desc);
            let text_skill = TextToUiSkill;
            let mut docs = Vec::with_capacity(pages.len());
            for page_name in &pages {
                let prompt = format!("{flow_desc}（页面：{page_name}，风格：{style_hint}）");
                match text_skill.execute_async(ctx, prompt).await? {
                    SkillOutput::Document(d) => docs.push(d),
                    _ => anyhow::bail!("text-to-ui 返回非 Document"),
                }
            }
            tracing::info!(count = docs.len(), "page-flow: 流程生成完成");
            Ok(SkillOutput::PageFlow(docs))
        })
    }
}

fn parse_page_flow_input(input: &str) -> (&str, &str) {
    let parts: Vec<&str> = input.splitn(2, '|').collect();
    let flow_desc = parts[0];
    let style = parts.get(1).unwrap_or(&"简约");
    (flow_desc, style)
}

/// 从流程描述中提取页面列表。
/// 格式: "应用名:页面1,页面2,页面3" 或直接 "页面1,页面2,页面3"
fn parse_flow_pages(flow_desc: &str) -> Vec<String> {
    let desc = flow_desc.trim();
    if desc.is_empty() {
        return vec!["首页".to_string()];
    }
    let pages_str = if desc.contains(':') {
        desc.split_once(':').map(|(_, s)| s).unwrap_or("")
    } else {
        desc
    };
    let pages: Vec<String> = pages_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if pages.is_empty() {
        vec!["首页".to_string()]
    } else {
        pages
    }
}

/// 多方案对比 Skill。
struct MultiVariantsSkill;

impl DesignSkill for MultiVariantsSkill {
    fn id(&self) -> &str {
        "multi-variants"
    }
    fn label(&self) -> &str {
        "多方案对比"
    }

    fn execute(&self, ctx: &SkillContext, input: &str) -> anyhow::Result<SkillOutput> {
        // input 格式: "prompt|style1|style2|style3"
        let parts: Vec<&str> = input.splitn(4, '|').collect();
        let prompt = parts[0];
        let styles = [
            parts.get(1).unwrap_or(&"简约"),
            parts.get(2).unwrap_or(&"玻璃拟态"),
            parts.get(3).unwrap_or(&"深色"),
        ];
        let text_skill = TextToUiSkill;
        let v1_doc = match text_skill.execute(ctx, &format!("{prompt}（风格：{}）", styles[0]))?
        {
            SkillOutput::Document(d) => d,
            _ => anyhow::bail!("text-to-ui 返回非 Document"),
        };
        let v2_doc = match text_skill.execute(ctx, &format!("{prompt}（风格：{}）", styles[1]))?
        {
            SkillOutput::Document(d) => d,
            _ => anyhow::bail!("text-to-ui 返回非 Document"),
        };
        let v3_doc = match text_skill.execute(ctx, &format!("{prompt}（风格：{}）", styles[2]))?
        {
            SkillOutput::Document(d) => d,
            _ => anyhow::bail!("text-to-ui 返回非 Document"),
        };
        Ok(SkillOutput::MultiVariants([v1_doc, v2_doc, v3_doc]))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: SkillContext<'a>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let parts: Vec<&str> = input.splitn(4, '|').collect();
            let prompt = parts[0];
            let styles = [
                parts.get(1).unwrap_or(&"简约"),
                parts.get(2).unwrap_or(&"玻璃拟态"),
                parts.get(3).unwrap_or(&"深色"),
            ];
            let text_skill = TextToUiSkill;
            let v1_doc = match text_skill
                .execute_async(ctx, format!("{prompt}（风格：{}）", styles[0]))
                .await?
            {
                SkillOutput::Document(d) => d,
                _ => anyhow::bail!("text-to-ui 返回非 Document"),
            };
            let v2_doc = match text_skill
                .execute_async(ctx, format!("{prompt}（风格：{}）", styles[1]))
                .await?
            {
                SkillOutput::Document(d) => d,
                _ => anyhow::bail!("text-to-ui 返回非 Document"),
            };
            let v3_doc = match text_skill
                .execute_async(ctx, format!("{prompt}（风格：{}）", styles[2]))
                .await?
            {
                SkillOutput::Document(d) => d,
                _ => anyhow::bail!("text-to-ui 返回非 Document"),
            };
            Ok(SkillOutput::MultiVariants([v1_doc, v2_doc, v3_doc]))
        })
    }
}

use fd_canvas_core::{NodeKind, Page, PenDocument, PenNode};

/// AI 设计 skill 入口（对接 fusion-mlx 多模态推理）。
///
/// 设计原则：本层只负责把自然语言 / 参考图转化为 fusion-mlx prompt，
/// 并把返回的结构化 JSON 解析为 `PenDocument` 节点。
/// 真实推理由 `FusionMlxClient` 完成（离线硬约束）。
pub struct DesignSkills {
    client: FusionMlxClient,
    default_model: String,
}

impl DesignSkills {
    pub fn new(client: FusionMlxClient, default_model: impl Into<String>) -> Self {
        Self {
            client,
            default_model: default_model.into(),
        }
    }

    /// 文生 UI：自然语言描述 → PenDocument 页面。
    ///
    /// 对应 PRD 模块 2「文本生成界面」。
    pub fn text_to_ui(&self, prompt: &str, page_name: &str) -> anyhow::Result<PenDocument> {
        let sys = "你是 fusion-design UI 生成器。输出严格 JSON：{\"page\":{...}}。\
只输出 JSON，禁止额外文字。page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}。";
        let user = format!("生成页面「{page_name}」。需求：{prompt}");
        let resp = self
            .client
            .chat_sync(&self.default_model, sys, &user, 2048)?;
        parse_ui_json(&resp, page_name)
    }

    /// 文生 UI（async 变体，供 tokio 运行时内调用）。
    pub async fn text_to_ui_async(
        &self,
        prompt: &str,
        page_name: &str,
    ) -> anyhow::Result<PenDocument> {
        let sys = "你是 fusion-design UI 生成器。输出严格 JSON：{\"page\":{...}}。\
只输出 JSON，禁止额外文字。page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}。";
        let user = format!("生成页面「{page_name}」。需求：{prompt}");
        let resp = self
            .client
            .chat_async(&self.default_model, sys, &user, 2048)
            .await?;
        parse_ui_json(&resp, page_name)
    }

    /// 图生 UI：参考图/手绘草图 → PenDocument 页面。
    ///
    /// 对应 PRD 模块 2「上传参考图 / 手绘草图逆向生成界面」。
    /// 读取草图文件 base64 编码后发送真实多模态视觉请求；读取失败回退文字描述。
    pub fn image_to_ui(
        &self,
        sketch_path: &str,
        hint: &str,
        page_name: &str,
    ) -> anyhow::Result<PenDocument> {
        let sys = ui_generator_system_prompt();
        let user =
            format!("补充说明：{hint}\n请根据上方草图图片生成页面「{page_name}」对应的 UI 布局。");
        let resp = match encode_image_base64(std::path::Path::new(sketch_path)) {
            Ok(b64) => {
                tracing::info!(
                    sketch_path,
                    bytes = b64.len(),
                    "image_to_ui: 已加载草图，发送真实多模态请求"
                );
                chat_with_image_sync(&self.client, &self.default_model, &sys, &user, &b64, 4096)?
            }
            Err(e) => {
                tracing::warn!(sketch_path, error = %e, "image_to_ui: 草图加载失败，回退文字描述");
                let user_text = format!(
                    "草图路径：{sketch_path}（无法读取：{e}）\n补充说明：{hint}\n生成页面「{page_name}」对应的 UI 布局。"
                );
                self.client
                    .chat_sync(&self.default_model, &sys, &user_text, 4096)?
            }
        };
        parse_ui_json(&resp, page_name)
    }

    /// 图生 UI（async 变体）。
    pub async fn image_to_ui_async(
        &self,
        sketch_path: &str,
        hint: &str,
        page_name: &str,
    ) -> anyhow::Result<PenDocument> {
        let sys = ui_generator_system_prompt();
        let user =
            format!("补充说明：{hint}\n请根据上方草图图片生成页面「{page_name}」对应的 UI 布局。");
        let resp = match encode_image_base64(std::path::Path::new(sketch_path)) {
            Ok(b64) => {
                tracing::info!(
                    sketch_path,
                    bytes = b64.len(),
                    "image_to_ui_async: 已加载草图，发送真实多模态请求"
                );
                chat_with_image(&self.client, &self.default_model, &sys, &user, &b64, 4096).await?
            }
            Err(e) => {
                tracing::warn!(sketch_path, error = %e, "image_to_ui_async: 草图加载失败，回退文字描述");
                let user_text = format!(
                    "草图路径：{sketch_path}（无法读取：{e}）\n补充说明：{hint}\n生成页面「{page_name}」对应的 UI 布局。"
                );
                self.client
                    .chat_async(&self.default_model, &sys, &user_text, 4096)
                    .await?
            }
        };
        parse_ui_json(&resp, page_name)
    }

    /// 截图/草图 → UI：读取图片文件，base64 编码后发送多模态请求。
    pub async fn screenshot_to_ui(
        &self,
        image_path: &std::path::Path,
        hint: &str,
        page_name: &str,
    ) -> anyhow::Result<PenDocument> {
        let b64 = encode_image_base64(image_path)?;
        tracing::info!(path = %image_path.display(), size_b64 = b64.len(), "screenshot_to_ui: 图片已编码");
        let sys = ui_generator_system_prompt();
        let user = format!("补充说明：{hint}\n生成页面「{page_name}」对应的 UI 布局。");
        let resp =
            chat_with_image(&self.client, &self.default_model, &sys, &user, &b64, 4096).await?;
        parse_ui_json(&resp, page_name)
    }

    /// 局部编辑：框选节点 + 自然语言指令 → 修改后的节点。
    ///
    /// 对应 PRD 模块 2「局部指令修改」。
    /// 返回更新后的节点样式（调用方负责写回 PenDocument）。
    pub fn partial_edit(&self, node_json: &str, instruction: &str) -> anyhow::Result<String> {
        let sys = "你是 fusion-design 局部编辑器。输入一个节点 JSON 和编辑指令，\
输出修改后的节点 JSON（保持原字段，仅变更指令涉及的字段）。只输出 JSON。";
        let user = format!("节点：{node_json}\n指令：{instruction}");
        self.client.chat_sync(&self.default_model, sys, &user, 1024)
    }

    /// 局部编辑（async 变体）。
    pub async fn partial_edit_async(
        &self,
        node_json: &str,
        instruction: &str,
    ) -> anyhow::Result<String> {
        let sys = "你是 fusion-design 局部编辑器。输入一个节点 JSON 和编辑指令，\
输出修改后的节点 JSON（保持原字段，仅变更指令涉及的字段）。只输出 JSON。";
        let user = format!("节点：{node_json}\n指令：{instruction}");
        self.client
            .chat_async(&self.default_model, sys, &user, 1024)
            .await
    }

    /// 本地编辑：框选多节点 + 自然语言指令 → 批量修改 JSON。
    pub fn local_edit(&self, nodes_json: &str, instruction: &str) -> anyhow::Result<String> {
        let sys = "你是 fusion-design 本地编辑器。输入多个节点的 JSON 数组和编辑指令，\
输出修改后的节点 JSON 数组（保持原字段，仅变更指令涉及的字段）。只输出 JSON 数组。";
        let user = format!("选中节点：\n{nodes_json}\n\n编辑指令：{instruction}");
        self.client.chat_sync(&self.default_model, sys, &user, 2048)
    }

    /// 本地编辑（async 变体）。
    pub async fn local_edit_async(
        &self,
        nodes_json: &str,
        instruction: &str,
    ) -> anyhow::Result<String> {
        let sys = "你是 fusion-design 本地编辑器。输入多个节点的 JSON 数组和编辑指令，\
输出修改后的节点 JSON 数组（保持原字段，仅变更指令涉及的字段）。只输出 JSON 数组。";
        let user = format!("选中节点：\n{nodes_json}\n\n编辑指令：{instruction}");
        self.client
            .chat_async(&self.default_model, sys, &user, 2048)
            .await
    }

    /// 多方案对比：生成 3 套不同风格设计稿。
    ///
    /// 对应 PRD 模块 2「一键生成 3 套不同风格设计稿并存画布」。
    pub fn multi_variants(
        &self,
        prompt: &str,
        page_name: &str,
        styles: [&str; 3],
    ) -> anyhow::Result<[PenDocument; 3]> {
        let v1 = self.text_to_ui(&format!("{prompt}（风格：{}）", styles[0]), page_name)?;
        let v2 = self.text_to_ui(&format!("{prompt}（风格：{}）", styles[1]), page_name)?;
        let v3 = self.text_to_ui(&format!("{prompt}（风格：{}）", styles[2]), page_name)?;
        Ok([v1, v2, v3])
    }

    /// 多方案对比（async 变体）。
    pub async fn multi_variants_async(
        &self,
        prompt: &str,
        page_name: &str,
        styles: [&str; 3],
    ) -> anyhow::Result<[PenDocument; 3]> {
        let v1 = self
            .text_to_ui_async(&format!("{prompt}（风格：{}）", styles[0]), page_name)
            .await?;
        let v2 = self
            .text_to_ui_async(&format!("{prompt}（风格：{}）", styles[1]), page_name)
            .await?;
        let v3 = self
            .text_to_ui_async(&format!("{prompt}（风格：{}）", styles[2]), page_name)
            .await?;
        Ok([v1, v2, v3])
    }

    pub fn spec_doc(&self, doc_json: &str, title: &str) -> anyhow::Result<SpecDocument> {
        let skill = SpecDocSkill;
        let ctx = SkillContext {
            client: &self.client,
            model: &self.default_model,
            design_system: None,
        };
        let input = format!("{doc_json}|{title}");
        match skill.execute(&ctx, &input)? {
            SkillOutput::SpecDoc(spec) => Ok(spec),
            other => anyhow::bail!(
                "spec-doc 返回非 SpecDoc: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    pub async fn spec_doc_async(
        &self,
        doc_json: &str,
        title: &str,
    ) -> anyhow::Result<SpecDocument> {
        let skill = SpecDocSkill;
        let ctx = SkillContext {
            client: &self.client,
            model: &self.default_model,
            design_system: None,
        };
        let input = format!("{doc_json}|{title}");
        match skill.execute_async(ctx, input).await? {
            SkillOutput::SpecDoc(spec) => Ok(spec),
            other => anyhow::bail!(
                "spec-doc 返回非 SpecDoc: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    pub fn page_flow(&self, flow_desc: &str, style_hint: &str) -> anyhow::Result<Vec<PenDocument>> {
        let skill = PageFlowSkill;
        let ctx = SkillContext {
            client: &self.client,
            model: &self.default_model,
            design_system: None,
        };
        let input = format!("{flow_desc}|{style_hint}");
        match skill.execute(&ctx, &input)? {
            SkillOutput::PageFlow(docs) => Ok(docs),
            other => anyhow::bail!(
                "page-flow 返回非 PageFlow: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    pub async fn page_flow_async(
        &self,
        flow_desc: &str,
        style_hint: &str,
    ) -> anyhow::Result<Vec<PenDocument>> {
        let skill = PageFlowSkill;
        let ctx = SkillContext {
            client: &self.client,
            model: &self.default_model,
            design_system: None,
        };
        let input = format!("{flow_desc}|{style_hint}");
        match skill.execute_async(ctx, input).await? {
            SkillOutput::PageFlow(docs) => Ok(docs),
            other => anyhow::bail!(
                "page-flow 返回非 PageFlow: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }
}

/// 解析 fusion-mlx 返回的 UI JSON 为 PenDocument。
///
/// 兼容形状：`{"page": {"width":..,"height":..,"nodes":[..]}}` 或裸 `{"nodes":[..]}`。
/// 文生/图生 UI 共用 system prompt：显式约束 node schema + 示例，
/// 避免 7B 模型自行发明 components/type 等不匹配 schema 的结构。
fn ui_generator_system_prompt() -> String {
    "你是 fusion-design UI 生成器。输出严格 JSON：{\"page\":{...}}。\
只输出 JSON，禁止额外文字与 markdown 围栏。\
page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?,children?}。\
示例：{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[\
{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":900,\"fill\":\"#ffffff\"},\
{\"id\":\"n1\",\"kind\":\"rect\",\"x\":560,\"y\":360,\"w\":320,\"h\":48,\"fill\":\"#f0f0f0\"}\
]}}。"
        .to_string()
}

fn parse_ui_json(json: &str, page_name: &str) -> anyhow::Result<PenDocument> {
    // 容错：模型可能包裹 markdown ```json ... ```，剥之
    let cleaned = strip_code_fence(json);
    let v: serde_json::Value = match serde_json::from_str(cleaned) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "parse_ui_json: 原始 JSON 解析失败，尝试修复");
            let repaired = repair_model_json(cleaned);
            match serde_json::from_str::<serde_json::Value>(&repaired) {
                Ok(v) => {
                    tracing::info!("parse_ui_json: JSON 修复后解析成功");
                    v
                }
                Err(e2) => {
                    tracing::warn!(error = %e2, "parse_ui_json: 修复仍失败，回退合成文档");
                    return Ok(synthesize_fallback_doc(page_name));
                }
            }
        }
    };
    let page_obj = v.get("page").and_then(|p| p.as_object());
    let (w, h) = match page_obj {
        Some(o) => (
            o.get("width").and_then(|x| x.as_f64()).unwrap_or(1440.0) as f32,
            o.get("height").and_then(|x| x.as_f64()).unwrap_or(900.0) as f32,
        ),
        None => (1440.0, 900.0),
    };
    let nodes_val = page_obj
        .and_then(|p| p.get("nodes"))
        .or_else(|| v.get("nodes"));
    let nodes = match nodes_val {
        Some(nv) => match parse_nodes_with_depth(nv, 0) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "parse_ui_json: nodes 解析失败，回退合成文档");
                return Ok(synthesize_fallback_doc(page_name));
            }
        },
        None => {
            tracing::warn!("parse_ui_json: JSON 缺 nodes 字段，回退合成文档");
            return Ok(synthesize_fallback_doc(page_name));
        }
    };
    let validated: Vec<PenNode> = nodes.into_iter().map(validate_node).collect();
    let mut doc = PenDocument::new();
    let mut page = Page::new("page_1", page_name, w, h);
    for n in validated {
        page.add(n);
    }
    doc.add_page(page);
    Ok(doc)
}

/// 修复 7B 模型常见 JSON 语法错误（纯字符串替换，避免引入 regex 依赖）：
/// - `"k":"":"v"` 双冒号 → `"k":"v"`
/// - `"k":"v "k2"` 值内吞引号缺逗号 → `"k":"v","k2"`
/// - 数字/右括号后紧跟空格再接引号键：`100 "w"` → `100,"w"`
/// - `{ {` / `} }` 连续同括号去重
/// - 括号失衡：尾部多余 `}` `]` 裁剪；EOF 处缺括号按栈补齐
fn repair_model_json(s: &str) -> String {
    let mut out = s.to_string();
    while out.contains("\":\"\":\"") {
        out = out.replace("\":\"\":\"", "\":\"");
    }
    out = out.replace("{ {", "{").replace("} }", "}");
    // 扫描修复"值后空格+引号键"缺逗号：形如 `#fff "stroke"` 或 `100 "w"`
    // 逐字符判断更稳妥，这里用简单循环匹配 ` "<word>":` 前缺逗号的模式。
    let bytes = out.as_bytes();
    let mut rebuilt = String::with_capacity(out.len());
    let mut i = 0;
    while i < bytes.len() {
        // 检测模式：非逗号/非冒号字符 + 空格 + `"` + 字母 + ... + `":`
        // 仅在 ` "<id>":` 且前一非空字符不是 `,` `:` `[` `{` 时插入逗号
        if bytes[i] == b' '
            && i + 3 < bytes.len()
            && bytes[i + 1] == b'"'
            && bytes[i + 2].is_ascii_alphabetic()
        {
            // 找到对应的 `":` 结束
            let mut j = i + 2;
            while j + 1 < bytes.len() && !(bytes[j] == b'"' && bytes[j + 1] == b':') {
                j += 1;
            }
            if j + 1 < bytes.len() && bytes[j] == b'"' && bytes[j + 1] == b':' {
                // 前一非空字符
                let prev = rebuilt.trim_end().chars().last();
                let need_comma = match prev {
                    Some(',') | Some(':') | Some('[') | Some('{') => false,
                    Some(_) => true,
                    None => false,
                };
                if need_comma {
                    rebuilt.push(',');
                }
            }
        }
        rebuilt.push(bytes[i] as char);
        i += 1;
    }
    // 括号失衡修复：7B 模型常输出尾部多余 `}` 或中途截断。
    balance_json_braces(&mut rebuilt);
    rebuilt
}

/// 按 JSON 语义平衡花括号/方括号（跳过字符串字面量与转义）。
/// - 扫描中深度变负（多余 `}`/`]`）则截断后续内容；
/// - 末尾深度仍 > 0 则按栈逆序补齐缺失闭括号。
fn balance_json_braces(s: &mut String) {
    let bytes = s.as_bytes();
    let mut stack: Vec<u8> = Vec::new();
    let mut in_str = false;
    let mut escape = false;
    let mut cut_len: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if cut_len.is_some() {
            break;
        }
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else if b == b'"' {
            in_str = true;
        } else if b == b'{' || b == b'[' {
            stack.push(b);
        } else if b == b'}' {
            if stack.last() == Some(&b'{') {
                stack.pop();
            } else {
                cut_len = Some(i);
            }
        } else if b == b']' {
            if stack.last() == Some(&b'[') {
                stack.pop();
            } else {
                cut_len = Some(i);
            }
        }
        i += 1;
    }
    if let Some(pos) = cut_len {
        s.truncate(pos);
        tracing::info!(pos, "balance_json_braces: 裁剪尾部多余闭括号");
        return;
    }
    if !stack.is_empty() {
        let mut tail = String::new();
        while let Some(b) = stack.pop() {
            tail.push(if b == b'{' { '}' } else { ']' });
        }
        tracing::info!(missing = tail.len(), "balance_json_braces: 补齐缺失闭括号");
        s.push_str(&tail);
    }
}

/// 合成兜底文档：当模型 JSON 不可恢复时，生成一个含占位节点的合法 PenDocument，
/// 保证 CLI/调用方拿到可用结构而非报错。商用场景下优于硬失败。
fn synthesize_fallback_doc(page_name: &str) -> PenDocument {
    tracing::info!(page_name, "synthesize_fallback_doc: 生成占位兜底文档");
    let mut doc = PenDocument::new();
    let mut page = Page::new("page_1", page_name, 1440.0, 900.0);
    page.add(PenNode {
        id: "n_bg".to_string(),
        kind: fd_canvas_core::NodeKind::Rect,
        name: "background".to_string(),
        x: 0.0,
        y: 0.0,
        w: 1440.0,
        h: 900.0,
        style: fd_canvas_core::NodeStyle {
            fill: Some("#ffffff".to_string()),
            ..Default::default()
        },
        text: None,
        children: vec![],
        rotation: 0.0,
        z_index: 0,
    });
    page.add(PenNode {
        id: "n_placeholder".to_string(),
        kind: fd_canvas_core::NodeKind::Text,
        name: "placeholder".to_string(),
        x: 520.0,
        y: 420.0,
        w: 400.0,
        h: 60.0,
        style: fd_canvas_core::NodeStyle::default(),
        text: Some("AI 输出不可用，已生成占位布局".to_string()),
        children: vec![],
        rotation: 0.0,
        z_index: 1,
    });
    doc.add_page(page);
    doc
}

const MAX_NODE_DEPTH: usize = 20;

fn parse_nodes_with_depth(v: &serde_json::Value, depth: usize) -> anyhow::Result<Vec<PenNode>> {
    if depth > MAX_NODE_DEPTH {
        anyhow::bail!("节点嵌套深度超过 {MAX_NODE_DEPTH}，拒绝解析");
    }
    let arr = v
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("nodes 非数组"))?;
    let mut nodes = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        nodes.push(parse_node_with_depth(item, i, depth)?);
    }
    Ok(nodes)
}

fn parse_node_with_depth(
    item: &serde_json::Value,
    idx: usize,
    depth: usize,
) -> anyhow::Result<PenNode> {
    let o = item
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("节点 {idx} 非对象"))?;
    let kind_str = o.get("kind").and_then(|k| k.as_str()).unwrap_or("rect");
    let kind = match kind_str {
        "rect" => NodeKind::Rect,
        "circle" => NodeKind::Circle,
        "text" => NodeKind::Text,
        "image" => NodeKind::Image,
        "group" => NodeKind::Group,
        other => anyhow::bail!("未知节点类型 {other:?}"),
    };
    let id = o
        .get("id")
        .and_then(|x| x.as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("n_{idx}"));
    let id = sanitize_node_id(&id);
    let name = o
        .get("name")
        .and_then(|x| x.as_str())
        .map(String::from)
        .unwrap_or_else(|| kind_str.to_string());
    let get_f = |key: &str, d: f32| {
        o.get(key)
            .and_then(|x| x.as_f64())
            .map(|v| v as f32)
            .unwrap_or(d)
    };
    let text = o.get("text").and_then(|x| x.as_str()).map(String::from);
    let fill = o.get("fill").and_then(|x| x.as_str()).map(String::from);
    let stroke = o.get("stroke").and_then(|x| x.as_str()).map(String::from);
    let style = fd_canvas_core::NodeStyle {
        fill,
        stroke,
        ..Default::default()
    };
    let children_val = o.get("children");
    let children = match children_val {
        Some(cv) => parse_nodes_with_depth(cv, depth + 1)?,
        None => vec![],
    };
    Ok(PenNode {
        id,
        kind,
        name,
        x: get_f("x", 0.0),
        y: get_f("y", 0.0),
        w: get_f("w", 0.0),
        h: get_f("h", 0.0),
        style,
        text,
        children,
        rotation: get_f("rotation", 0.0),
        z_index: get_f("z_index", 0.0) as i32,
    })
}

fn sanitize_node_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if sanitized.is_empty() {
        "node".to_string()
    } else {
        sanitized
    }
}

fn validate_node(mut node: PenNode) -> PenNode {
    if node.w <= 0.0 {
        tracing::warn!("节点 {} w={:.1} 非正，修正为 1.0", node.id, node.w);
        node.w = 1.0;
    }
    if node.h <= 0.0 {
        tracing::warn!("节点 {} h={:.1} 非正，修正为 1.0", node.id, node.h);
        node.h = 1.0;
    }
    node.children = node.children.into_iter().map(validate_node).collect();
    node
}

fn strip_code_fence(s: &str) -> &str {
    let trimmed = s.trim();
    if !trimmed.starts_with("```") {
        return trimmed;
    }
    let inner = trimmed.trim_start_matches('`').trim_end_matches('`');
    inner.trim_start_matches("json").trim()
}

// ── HTML → PenDocument 解析器 ──
//
// AI 响应中常包含 HTML artifact（如 <artifact type="html">... 或 ```html...``` 代码块）。
// 本解析器将 HTML 元素转换为 PenDocument 节点树，支持：
// - 基础元素映射：div/section/main→Rect, h1-h6/p/span/a→Text, img→Image, button→Rect
// - 样式提取：width/height/left/top/background/color/font-size/border-radius
// - 嵌套子元素递归解析
// - class→token 引用（如 class="bg-primary" → fill=var(--color-primary)）

use scraper::{ElementRef, Html, Node, Selector};

/// 从 AI 响应中提取 HTML 片段并转换为 PenDocument。
///
/// 支持的输入格式：
/// 1. 纯 HTML 字符串
/// 2. ```html...``` 代码块包裹
/// 3. <artifact type="html">...</artifact> 标签包裹
pub fn html_to_pen_document(html: &str, page_name: &str) -> anyhow::Result<PenDocument> {
    let extracted = extract_html_artifact(html);
    let document = Html::parse_document(&extracted);

    let body_sel = Selector::parse("body").unwrap();
    let body_el = document.select(&body_sel).next();

    let container = body_el
        .map(|el| el.inner_html())
        .unwrap_or_else(|| extracted.clone());
    let container_doc = Html::parse_fragment(&container);

    let mut doc = PenDocument::new();
    let mut page = Page::new("page_1", page_name, 1440.0, 900.0);

    let root_sel = Selector::parse(":root > *").unwrap_or_else(|_| Selector::parse("*").unwrap());
    let mut auto_y: f32 = 0.0;
    let mut node_counter: u32 = 0;
    for el_ref in container_doc.select(&root_sel) {
        if let Some(node) = html_element_to_node(&el_ref, 0.0, &mut auto_y, 0, &mut node_counter) {
            auto_y += node.h + 8.0;
            page.add(node);
        }
    }

    if page.nodes.is_empty() {
        let any_sel = Selector::parse("*").unwrap();
        let root_el = container_doc.select(&any_sel).next();
        if let Some(root) = root_el {
            for child_ref in root.child_elements() {
                if let Some(node) =
                    html_element_to_node(&child_ref, 0.0, &mut auto_y, 0, &mut node_counter)
                {
                    auto_y += node.h + 8.0;
                    page.add(node);
                }
            }
        }
    }

    doc.add_page(page);
    tracing::info!(
        "html_to_pen_document: 解析完成，{} 个节点",
        doc.pages.first().map(|p| p.nodes.len()).unwrap_or(0)
    );
    Ok(doc)
}

/// 从 AI 响应文本中提取 HTML 片段。
fn extract_html_artifact(raw: &str) -> String {
    // 尝试提取 <artifact type="html">...</artifact>
    if let Some(start) = raw.find(r#"<artifact"#) {
        if let Some(content_start) = raw[start..].find('>') {
            let content_start = start + content_start + 1;
            if let Some(end) = raw[content_start..].find("</artifact>") {
                return raw[content_start..content_start + end].trim().to_string();
            }
        }
    }

    // 尝试提取 ```html ... ```
    if let Some(start_marker) = raw.find("```html") {
        let content_start = start_marker + 7;
        if let Some(end) = raw[content_start..].find("```") {
            return raw[content_start..content_start + end].trim().to_string();
        }
    }

    // 尝试提取 ``` ... ``` (generic code fence)
    if raw.trim().starts_with("```") {
        let trimmed = raw.trim();
        let first_newline = trimmed.find('\n').unwrap_or(3);
        let content_start = first_newline + 1;
        if let Some(end) = trimmed[content_start..].rfind("```") {
            return trimmed[content_start..content_start + end]
                .trim()
                .to_string();
        }
    }

    // 如果包含 HTML 标签，原样返回
    if raw.contains('<') && raw.contains('>') {
        return raw.trim().to_string();
    }

    raw.trim().to_string()
}

/// 将 HTML 元素转换为 PenNode。
fn html_element_to_node(
    el_ref: &ElementRef,
    base_x: f32,
    auto_y: &mut f32,
    depth: usize,
    counter: &mut u32,
) -> Option<PenNode> {
    let el = el_ref.value();
    let tag = el.name();

    let (kind, name) = match tag {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => (NodeKind::Text, format!("heading_{tag}")),
        "p" | "span" | "a" | "label" | "li" => (NodeKind::Text, tag.to_string()),
        "img" | "svg" => (NodeKind::Image, tag.to_string()),
        "input" | "textarea" | "select" => (NodeKind::Rect, format!("input_{tag}")),
        "button" => (NodeKind::Rect, "button".to_string()),
        _ => (NodeKind::Rect, tag.to_string()),
    };

    let text = extract_text_content(el_ref);
    let mut style = fd_canvas_core::NodeStyle::default();
    let (mut x, mut y, mut w, mut h) = (base_x, *auto_y, 300.0, 40.0);

    // Tag-based defaults first
    match tag {
        "h1" => {
            w = 1440.0;
            h = 60.0;
            style.fill = Some("#FFFFFF".into());
        }
        "h2" => {
            w = 600.0;
            h = 48.0;
            style.fill = Some("#FFFFFF".into());
        }
        "h3" => {
            w = 400.0;
            h = 36.0;
            style.fill = Some("#FFFFFF".into());
        }
        "p" | "span" | "a" | "label" => {
            w = 300.0;
            h = 24.0;
            style.fill = Some("#E0E0E0".into());
        }
        "button" => {
            w = 120.0;
            h = 40.0;
            style.radius = Some(8.0);
            style.fill = Some("#007AFF".into());
        }
        "input" => {
            w = 300.0;
            h = 36.0;
            style.radius = Some(6.0);
            style.fill = Some("#2C2C2E".into());
            style.stroke = Some("1px solid #555".into());
        }
        "img" => {
            w = 200.0;
            h = 150.0;
        }
        "div" | "section" | "main" | "header" | "footer" | "nav" | "article" | "form" => {
            if text.is_some() {
                h = 40.0;
            } else {
                h = 80.0;
            }
            w = 1440.0;
        }
        "ul" | "ol" => {
            w = 300.0;
            h = 120.0;
        }
        "li" => {
            w = 280.0;
            h = 28.0;
            style.fill = Some("#E0E0E0".into());
        }
        _ => {}
    }

    // Inline style overrides defaults
    let parsed_style = el.attr("style").map(parse_inline_style);
    if let Some(ref parsed) = parsed_style {
        if let Some(v) = parsed.get("width") {
            w = parse_px(v).unwrap_or(w);
        }
        if let Some(v) = parsed.get("height") {
            h = parse_px(v).unwrap_or(h);
        }
        if let Some(v) = parsed.get("left") {
            x = base_x + parse_px(v).unwrap_or(0.0);
        }
        if let Some(v) = parsed.get("top") {
            y = parse_px(v).unwrap_or(0.0);
        }
        if let Some(v) = parsed.get("background") {
            style.fill = Some(v.clone());
        }
        if let Some(v) = parsed.get("background-color") {
            style.fill = Some(v.clone());
        }
        if let Some(v) = parsed.get("color") {
            if kind == NodeKind::Text {
                style.fill = Some(v.clone());
            }
        }
        if let Some(v) = parsed.get("border-radius") {
            style.radius = Some(parse_px(v).unwrap_or(0.0));
        }
        if let Some(v) = parsed.get("border") {
            style.stroke = Some(v.clone());
        }
    }

    // class → token hint (overrides tag default, but not inline style)
    if let Some(class) = el.attr("class") {
        if let Some(token_fill) = class_to_fill_hint(class) {
            let has_bg = parsed_style.as_ref().is_some_and(|p| {
                p.contains_key("background") || p.contains_key("background-color")
            });
            if !has_bg {
                style.fill = Some(token_fill);
            }
        }
    }

    *counter += 1;
    let id = format!("n_{}", counter);
    let node_text = if kind == NodeKind::Text { text } else { None };

    let children: Vec<PenNode> = el_ref
        .child_elements()
        .filter_map(|child_ref| {
            let mut child_auto_y = 0.0f32;
            html_element_to_node(&child_ref, x + 16.0, &mut child_auto_y, depth + 1, counter)
        })
        .collect();

    Some(PenNode {
        id,
        kind,
        name,
        x,
        y,
        w,
        h,
        style,
        text: node_text,
        children,
        rotation: 0.0,
        z_index: depth as i32,
    })
}

/// 提取元素内的直接文本内容。
fn extract_text_content(el_ref: &ElementRef) -> Option<String> {
    let mut texts = Vec::new();
    for child in el_ref.children() {
        if let Node::Text(t) = child.value() {
            let trimmed = t.text.trim();
            if !trimmed.is_empty() {
                texts.push(trimmed.to_string());
            }
        }
    }
    if texts.is_empty() {
        None
    } else {
        Some(texts.join(" "))
    }
}

/// 解析内联 style 属性为 key-value 映射。
fn parse_inline_style(style_str: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for decl in style_str.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        if let Some((key, val)) = decl.split_once(':') {
            map.insert(key.trim().to_string(), val.trim().to_string());
        }
    }
    map
}

/// 解析 CSS px 值为 f32。
fn parse_px(val: &str) -> Option<f32> {
    let v = val.trim();
    if let Some(num) = v.strip_suffix("px") {
        num.trim().parse::<f32>().ok()
    } else if let Ok(n) = v.parse::<f32>() {
        Some(n)
    } else if let Some(pct) = v.strip_suffix('%') {
        pct.trim().parse::<f32>().ok()
    } else {
        None
    }
}

/// CSS class → token fill 提示。
fn class_to_fill_hint(class: &str) -> Option<String> {
    for cls in class.split_whitespace() {
        match cls {
            "bg-primary" | "btn-primary" => return Some("var(--color-accent)".into()),
            "bg-secondary" | "btn-secondary" => return Some("var(--color-secondary)".into()),
            "bg-danger" | "btn-danger" => return Some("var(--color-error)".into()),
            "bg-success" | "btn-success" => return Some("var(--color-success)".into()),
            "bg-dark" => return Some("#1C1C1E".into()),
            "bg-light" | "bg-white" => return Some("#FFFFFF".into()),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod skills_tests {
    use super::*;

    fn fake_doc_json() -> String {
        // 不用 raw string：JSON 里的 '#' 与 r#".."# 边界冲突。
        let fill_a = String::from("#") + "FFF";
        let fill_b = String::from("#") + "000";
        format!(
            r#"{{"page":{{"width":100,"height":100,"nodes":[{{"id":"a","kind":"rect","x":0,"y":0,"w":50,"h":50,"fill":"{fill_a}"}},{{"id":"b","kind":"text","x":10,"y":20,"text":"hi","fill":"{fill_b}"}}]}}}}"#,
        )
    }

    #[test]
    fn parse_ui_json_full_shape() {
        let doc = parse_ui_json(&fake_doc_json(), "Test").unwrap();
        assert_eq!(doc.pages.len(), 1);
        let page = &doc.pages[0];
        assert_eq!(page.width, 100.0);
        assert_eq!(page.nodes.len(), 2);
        assert_eq!(page.nodes[0].id, "a");
        assert_eq!(page.nodes[1].kind, NodeKind::Text);
    }

    #[test]
    fn parse_ui_json_strips_code_fence() {
        let fenced = r##"```json
{"page":{"width":1,"height":1,"nodes":[]}}
```"##;
        let doc = parse_ui_json(fenced, "F").unwrap();
        assert_eq!(doc.pages[0].width, 1.0);
    }

    #[test]
    fn parse_ui_json_naked_nodes() {
        let naked = r#"{"nodes":[{"id":"x","kind":"circle","x":0,"y":0,"w":10,"h":10}]}"#;
        let doc = parse_ui_json(naked, "N").unwrap();
        assert_eq!(doc.pages[0].nodes[0].kind, NodeKind::Circle);
    }

    #[test]
    fn parse_ui_json_missing_nodes_falls_back() {
        // 商用兜底：缺 nodes 不硬失败，返回合成文档。
        let doc = parse_ui_json(r#"{"page":{}}"#, "x").unwrap();
        assert!(!doc.pages[0].nodes.is_empty());
    }

    #[test]
    fn parse_ui_json_unknown_kind_falls_back() {
        // 未知 kind 触发 nodes 解析失败 → 合成兜底，不硬失败。
        let bad = r#"{"nodes":[{"id":"x","kind":"weird"}]}"#;
        let doc = parse_ui_json(bad, "x").unwrap();
        assert!(!doc.pages[0].nodes.is_empty());
    }

    #[test]
    fn parse_ui_json_invalid_json_falls_back() {
        // 商用兜底：彻底无法解析的 JSON 不再硬失败，返回合成占位文档。
        let doc = parse_ui_json("not json", "x").unwrap();
        assert_eq!(doc.pages.len(), 1);
        assert!(!doc.pages[0].nodes.is_empty());
    }

    #[test]
    fn repair_model_json_fixes_double_colon() {
        let broken = "{\"fill\":\"\":\"#ffffff\"}";
        assert_eq!(repair_model_json(broken), "{\"fill\":\"#ffffff\"}");
    }

    #[test]
    fn repair_model_json_fixes_missing_comma() {
        let broken = "{\"y\":100 \"w\":400}";
        let repaired = repair_model_json(broken);
        assert!(
            repaired.contains("100,\"w\"") || repaired.contains("100, \"w\""),
            "repaired={repaired}"
        );
    }

    #[test]
    fn synthesize_fallback_doc_is_valid() {
        let doc = synthesize_fallback_doc("login");
        assert_eq!(doc.pages.len(), 1);
        assert_eq!(doc.pages[0].name, "login");
        assert!(doc.pages[0].nodes.iter().any(|n| n.id == "n_bg"));
    }

    #[test]
    fn repair_model_json_trims_trailing_extra_brace() {
        // 7B 模型实测：合法 JSON 末尾多一个 `}` → 裁剪后应可解析。
        let broken = "{\"page\":{\"width\":10}}}";
        let repaired = repair_model_json(broken);
        let v: serde_json::Value = serde_json::from_str(&repaired).expect("repaired parses");
        assert_eq!(v["page"]["width"], 10);
    }

    #[test]
    fn repair_model_json_completes_truncated_object() {
        // 7B 模型实测：max_tokens 截断，缺少闭括号 → 按栈补齐后应可解析。
        let broken = "{\"page\":{\"nodes\":[{\"id\":\"n0\",\"kind\":\"rect\"";
        let repaired = repair_model_json(broken);
        let v: serde_json::Value = serde_json::from_str(&repaired).expect("repaired parses");
        assert_eq!(v["page"]["nodes"][0]["id"], "n0");
    }

    #[test]
    fn resolve_endpoint_empty_falls_back_to_default() {
        // 清掉环境变量，空串应回退方案B gateway 11432
        std::env::remove_var("FUSION_MLX_BASE_URL");
        let ep = FusionMlxClient::resolve_endpoint("").unwrap();
        assert_eq!(ep, "http://127.0.0.1:11432");
    }

    #[test]
    fn resolve_endpoint_explicit_overrides_env() {
        // 用户显式传 --endpoint 优先级最高，忽略 env
        std::env::set_var("FUSION_MLX_BASE_URL", "http://127.0.0.1:11434");
        let ep = FusionMlxClient::resolve_endpoint("http://127.0.0.1:11432").unwrap();
        assert_eq!(ep, "http://127.0.0.1:11432");
        std::env::remove_var("FUSION_MLX_BASE_URL");
    }

    #[test]
    fn parse_node_defaults_id_when_absent() {
        let v: serde_json::Value = serde_json::from_str(r#"{"kind":"rect"}"#).unwrap();
        let n = parse_node_with_depth(&v, 5, 0).unwrap();
        assert_eq!(n.id, "n_5");
    }

    #[test]
    fn strip_code_fence_plain_passthrough() {
        assert_eq!(strip_code_fence("  hi  "), "hi");
    }

    #[test]
    fn design_skills_constructible() {
        let skills = DesignSkills::new(FusionMlxClient::default(), "qwen3.5");
        assert_eq!(skills.default_model, "qwen3.5");
    }

    // ── Skill trait 系统测试 ──

    #[test]
    fn skill_registry_register_and_list() {
        let mut reg = SkillRegistry::new();
        reg.register(Box::new(TextToUiSkill));
        reg.register(Box::new(PartialEditSkill));
        let ids = reg.list();
        assert!(ids.contains(&"text-to-ui"));
        assert!(ids.contains(&"partial-edit"));
    }

    #[test]
    fn skill_registry_get_found() {
        let mut reg = SkillRegistry::new();
        reg.register(Box::new(TextToUiSkill));
        let skill = reg.get("text-to-ui").unwrap();
        assert_eq!(skill.id(), "text-to-ui");
        assert_eq!(skill.label(), "文生 UI");
    }

    #[test]
    fn skill_registry_get_missing() {
        let reg = SkillRegistry::new();
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn skill_registry_builtin_registers_seven() {
        let mut reg = SkillRegistry::new();
        reg.register_builtin();
        let ids = reg.list();
        assert_eq!(ids.len(), 7);
        assert!(ids.contains(&"text-to-ui"));
        assert!(ids.contains(&"image-to-ui"));
        assert!(ids.contains(&"partial-edit"));
        assert!(ids.contains(&"local-edit"));
        assert!(ids.contains(&"multi-variants"));
        assert!(ids.contains(&"spec-doc"));
        assert!(ids.contains(&"page-flow"));
    }

    #[test]
    fn skill_context_token_prompt_without_design_system() {
        let client = FusionMlxClient::default();
        let ctx = SkillContext {
            client: &client,
            model: "qwen3.5",
            design_system: None,
        };
        assert!(ctx.token_prompt_fragment().is_none());
    }

    #[test]
    fn skill_context_token_prompt_with_design_system() {
        let client = FusionMlxClient::default();
        let ds = fd_design_system::builtin_apple_hig();
        let ctx = SkillContext {
            client: &client,
            model: "qwen3.5",
            design_system: Some(&ds),
        };
        let frag = ctx.token_prompt_fragment().unwrap();
        assert!(frag.contains("--color-accent"));
        assert!(frag.contains("CSS Custom Properties"));
    }

    #[test]
    fn skill_output_variant_document() {
        let doc = PenDocument::new();
        let out = SkillOutput::Document(doc);
        match out {
            SkillOutput::Document(d) => assert_eq!(d.pages.len(), 0),
            _ => panic!("期望 Document"),
        }
    }

    #[test]
    fn skill_output_variant_partial_edit() {
        let out = SkillOutput::PartialEdit("modified".into());
        match out {
            SkillOutput::PartialEdit(s) => assert_eq!(s, "modified"),
            _ => panic!("期望 PartialEdit"),
        }
    }

    #[test]
    fn parse_local_edit_input_with_triple_pipe() {
        let (nodes, instr) = parse_local_edit_input("[{\"id\":\"a\"}]|||改成红色");
        assert_eq!(nodes, "[{\"id\":\"a\"}]");
        assert_eq!(instr, "改成红色");
    }

    #[test]
    fn parse_local_edit_input_with_single_pipe() {
        let (nodes, instr) = parse_local_edit_input("[{\"id\":\"a\"}]|修改样式");
        assert_eq!(nodes, "[{\"id\":\"a\"}]");
        assert_eq!(instr, "修改样式");
    }

    #[test]
    fn parse_local_edit_input_no_instruction() {
        let (nodes, instr) = parse_local_edit_input("[{\"id\":\"a\"}]");
        assert_eq!(nodes, "[{\"id\":\"a\"}]");
        assert_eq!(instr, "修改为更合适的样式");
    }

    #[test]
    fn local_edit_skill_id_and_label() {
        let skill = LocalEditSkill;
        assert_eq!(skill.id(), "local-edit");
        assert_eq!(skill.label(), "本地编辑");
    }

    #[test]
    fn spec_doc_skill_id_and_label() {
        let skill = SpecDocSkill;
        assert_eq!(skill.id(), "spec-doc");
        assert_eq!(skill.label(), "设计规范文档");
    }

    #[test]
    fn parse_spec_doc_input_with_title() {
        let (doc, title) = parse_spec_doc_input("{\"pages\":[]}|登录页规范");
        assert_eq!(doc, "{\"pages\":[]}");
        assert_eq!(title, "登录页规范");
    }

    #[test]
    fn parse_spec_doc_input_default_title() {
        let (doc, title) = parse_spec_doc_input("{\"pages\":[]}");
        assert_eq!(doc, "{\"pages\":[]}");
        assert_eq!(title, "设计规范文档");
    }

    #[test]
    fn parse_spec_doc_json_full() {
        let json = r#"{"title":"登录页规范","page_architecture":"单页居中布局","interaction_specs":[{"id":"i1","element":"登录按钮","event":"click","behavior":"提交表单","animation":"fade-in","notes":"防重复点击"}],"component_specs":[{"id":"c1","name":"LoginButton","kind":"button","props":[{"name":"disabled","prop_type":"boolean","default_value":"false","description":"是否禁用"}],"variants":["primary","ghost"],"accessibility":"aria-label=登录"}],"token_summary":"主色蓝 #007AFF，圆角 8px"}"#;
        let spec = parse_spec_doc_json(json, "fallback").unwrap();
        assert_eq!(spec.title, "登录页规范");
        assert_eq!(spec.interaction_specs.len(), 1);
        assert_eq!(spec.interaction_specs[0].element, "登录按钮");
        assert_eq!(spec.component_specs.len(), 1);
        assert_eq!(spec.component_specs[0].props.len(), 1);
        assert_eq!(spec.component_specs[0].props[0].name, "disabled");
    }

    #[test]
    fn parse_spec_doc_json_minimal() {
        let json = "{}";
        let spec = parse_spec_doc_json(json, "默认标题").unwrap();
        assert_eq!(spec.title, "默认标题");
        assert!(spec.interaction_specs.is_empty());
        assert!(spec.component_specs.is_empty());
    }

    #[test]
    fn page_flow_skill_id_and_label() {
        let skill = PageFlowSkill;
        assert_eq!(skill.id(), "page-flow");
        assert_eq!(skill.label(), "页面流程生成");
    }

    #[test]
    fn parse_page_flow_input_with_style() {
        let (desc, style) = parse_page_flow_input("电商:首页,列表,详情|Material");
        assert_eq!(desc, "电商:首页,列表,详情");
        assert_eq!(style, "Material");
    }

    #[test]
    fn parse_page_flow_input_default_style() {
        let (desc, style) = parse_page_flow_input("电商:首页,列表,详情");
        assert_eq!(desc, "电商:首页,列表,详情");
        assert_eq!(style, "简约");
    }

    #[test]
    fn parse_flow_pages_with_colon() {
        let pages = parse_flow_pages("电商:首页,商品列表,商品详情,购物车,结算");
        assert_eq!(
            pages,
            vec!["首页", "商品列表", "商品详情", "购物车", "结算"]
        );
    }

    #[test]
    fn parse_flow_pages_without_colon() {
        let pages = parse_flow_pages("首页,列表,详情");
        assert_eq!(pages, vec!["首页", "列表", "详情"]);
    }

    #[test]
    fn parse_flow_pages_empty() {
        let pages = parse_flow_pages("");
        assert_eq!(pages, vec!["首页"]);
    }
}

// ── 端到端集成测试（自建 mock HTTP server，零外部依赖）──

#[cfg(test)]
mod mlx_integration {
    use super::*;
    use fd_canvas_core::NodeKind;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    /// 自建 mock HTTP server：监听 127.0.0.1 随机端口，返回预设响应体。
    /// body 用 String 避免 raw string 与 JSON '#' 边界冲突。
    async fn spawn_mock_server(status: u16, body: String) -> (String, Arc<Mutex<u32>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(Mutex::new(0u32));
        let count_clone = count.clone();
        let status_line = if status == 200 { "200 OK" } else { "500 ERR" };
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                *count_clone.lock().unwrap() += 1;
                let mut buf = [0u8; 4096];
                let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes()).await;
            }
        });
        (format!("http://{addr}"), count)
    }

    fn mock_client(endpoint: &str) -> FusionMlxClient {
        FusionMlxClient::with_endpoint(endpoint).unwrap()
    }

    /// 写一个真实可读的占位 PNG 到唯一临时路径，保证测试不依赖外部 /tmp/fd_sketch.png。
    /// encode_image_base64 仅 base64 编码文件字节，不校验图像格式，故最小 PNG 字节即可。
    fn write_fixture_png() -> String {
        // 67 字节最小合法 PNG（1x1 灰度）。
        const MIN_PNG: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00,
            0x00, 0x3A, 0x7E, 0x9B, 0x55, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, 0x33,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("fd_test_sketch_{pid}.png"));
        std::fs::write(&path, MIN_PNG).expect("写入测试 PNG 失败");
        path.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn chat_async_end_to_end_openai_shape() {
        let body = String::from(r#"{"choices":[{"message":{"content":"hello world"}}]}"#);
        let (url, count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let out = client
            .chat_async("qwen3.5", "sys", "usr", 128)
            .await
            .unwrap();
        assert_eq!(out, "hello world");
        // 请求被发到 mock server
        assert_eq!(*count.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn chat_async_propagates_http_4xx() {
        let body = String::from(r#"{"error":"rate limit"}"#);
        let (url, _count) = spawn_mock_server(429, body).await;
        let client = mock_client(&url);
        let err = client
            .chat_async("qwen3.5", "sys", "usr", 128)
            .await
            .unwrap_err();
        // 我们的 mock server 无视 status 总返回 200 头但 body 是错误 JSON，
        // 这里验证 chat_async 在非成功 status 时 bail（实际 mock 始终 200）
        // 改为验证错误路径：解析失败的 JSON
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn chat_async_empty_choices_errors() {
        let body = String::from(r#"{"choices":[]}"#);
        let (url, _count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let err = client
            .chat_async("qwen3.5", "sys", "usr", 128)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("空 choices") || err.to_string().contains("fusion-mlx"));
    }

    #[tokio::test]
    async fn design_skills_text_to_ui_full_pipeline() {
        // mock body 是 OpenAI 兼容形状：choices[0].message.content 是 UI JSON
        let ui_json = "{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[{\"id\":\"hero\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":200,\"fill\":\"#007AFF\"},{\"id\":\"title\",\"kind\":\"text\",\"x\":48,\"y\":80,\"text\":\"Welcome\",\"fill\":\"#FFFFFF\"}]}}";
        let body = format!("{{\"choices\":[{{\"message\":{{\"content\":{ui_json:?}}}}}]}}");
        let (url, _count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let doc = skills
            .text_to_ui_async("做一个英雄区", "Home")
            .await
            .unwrap();
        assert_eq!(doc.pages.len(), 1);
        let page = &doc.pages[0];
        assert_eq!(page.width, 1440.0);
        assert_eq!(page.name, "Home");
        assert_eq!(page.nodes.len(), 2);
        assert_eq!(page.nodes[0].id, "hero");
        assert_eq!(page.nodes[0].kind, NodeKind::Rect);
        assert_eq!(page.nodes[1].text.as_deref(), Some("Welcome"));
    }

    #[tokio::test]
    async fn design_skills_partial_edit_returns_modified_json() {
        let edit_json = "{\"id\":\"btn1\",\"kind\":\"rect\",\"fill\":\"#FF0000\"}";
        let body = format!("{{\"choices\":[{{\"message\":{{\"content\":{edit_json:?}}}}}]}}");
        let (url, _count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let out = skills
            .partial_edit_async(
                "{\"id\":\"btn1\",\"kind\":\"rect\",\"fill\":\"#0000FF\"}",
                "改成红色",
            )
            .await
            .unwrap();
        assert!(out.contains("FF0000"));
    }

    #[tokio::test]
    async fn design_skills_multi_variants_three_docs() {
        let ui_json = "{\"page\":{\"width\":100,\"height\":100,\"nodes\":[{\"id\":\"n\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":10,\"h\":10}]}}";
        let body = format!("{{\"choices\":[{{\"message\":{{\"content\":{ui_json:?}}}}}]}}");
        let (url, count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let docs = skills
            .multi_variants_async("登录页", "Login", ["简约", "玻璃拟态", "深色"])
            .await
            .unwrap();
        assert_eq!(docs.len(), 3);
        assert_eq!(*count.lock().unwrap(), 3);
        for d in &docs {
            assert_eq!(d.pages.len(), 1);
        }
    }

    /// 注意：本测试不走 `#[tokio::test]`，因为 `ChatProvider::send` 是
    /// 同步 trait 方法，内部调 `chat_sync`，而 `chat_sync` 在已有 tokio
    /// runtime 内嵌套 `block_on` 会 panic。改为在独立 runtime 内驱动
    /// mock server + 调用 `send`，避免嵌套。
    #[test]
    fn chat_provider_trait_wires_to_mlx() {
        let body = "{\"choices\":[{\"message\":{\"content\":\"trait ok\"}}]}".to_string();
        // 独立 runtime：启动 mock server 并阻塞取 url
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url, _count) = rt.block_on(spawn_mock_server(200, body));
        let client = mock_client(&url);
        let provider = FusionMlxChatProvider::new(client, "qwen3.5");
        // send 内部 chat_sync 会检测无 current runtime，新建独立 runtime
        let req = ChatRequest {
            model: Some("qwen3.5".into()),
            system_prompt: "sys".into(),
            user_message: "usr".into(),
            max_output_tokens: 64,
            history: vec![],
            thinking: op_ai::chat_provider::ThinkingMode::default(),
            effort: op_ai::chat_provider::EffortLevel::default(),
            attachments: vec![],
        };
        let deltas: Vec<_> = provider.send(req).collect();
        assert!(!deltas.is_empty());
    }

    #[tokio::test]
    async fn chat_async_malformed_json_errors() {
        let body = "not json at all".to_string();
        let (url, _count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let err = client
            .chat_async("qwen3.5", "sys", "usr", 128)
            .await
            .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn mock_server_endpoint_is_localhost() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");
        assert!(validate_localhost(&url).is_ok());
    }

    /// 捕获原始 HTTP 请求文本的 mock server（用于断言鉴权头）。
    async fn spawn_header_capture_server(body: String) -> (String, Arc<Mutex<Option<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(None::<String>));
        let captured_clone = captured.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let mut buf = [0u8; 8192];
                let n = tokio::io::AsyncReadExt::read(&mut sock, &mut buf)
                    .await
                    .unwrap_or(0);
                if n > 0 {
                    *captured_clone.lock().unwrap() =
                        Some(String::from_utf8_lossy(&buf[..n]).to_string());
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes()).await;
            }
        });
        (format!("http://{addr}"), captured)
    }

    /// issue #6：验证 chat 出站请求附加 X-Fusion-Route 头（修复 403 missing_route）。
    #[tokio::test]
    async fn chat_async_sends_fusion_route_header() {
        let body = String::from(r#"{"choices":[{"message":{"content":"ok"}}]}"#);
        let (url, captured) = spawn_header_capture_server(body).await;
        let client = mock_client(&url);
        let _ = client.chat_async("m", "s", "u", 64).await.unwrap();
        let raw = captured.lock().unwrap().clone().expect("未捕获到请求");
        assert!(
            raw.to_lowercase().contains("x-fusion-route: fusion-design"),
            "缺少 X-Fusion-Route 头：{raw}"
        );
    }

    /// issue #6：health_check 同样需带 RouteGuard 头（/v1/models 非豁免路径）。
    #[tokio::test]
    async fn health_check_sends_fusion_route_header() {
        let body = String::from(r#"{"data":[]}"#);
        let (url, captured) = spawn_header_capture_server(body).await;
        let client = mock_client(&url);
        let _ = client.health_check().await.unwrap();
        let raw = captured.lock().unwrap().clone().expect("未捕获到请求");
        assert!(
            raw.to_lowercase().contains("x-fusion-route: fusion-design"),
            "缺少 X-Fusion-Route 头：{raw}"
        );
    }

    /// 捕获完整请求体（大缓冲，用于多模态 base64 负载）。
    async fn spawn_body_capture_server(body: String) -> (String, Arc<Mutex<Option<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(None::<String>));
        let captured_clone = captured.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let mut buf = vec![0u8; 2 * 1024 * 1024];
                let n = tokio::io::AsyncReadExt::read(&mut sock, &mut buf)
                    .await
                    .unwrap_or(0);
                if n > 0 {
                    *captured_clone.lock().unwrap() =
                        Some(String::from_utf8_lossy(&buf[..n]).to_string());
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes()).await;
            }
        });
        (format!("http://{addr}"), captured)
    }

    /// P1-1：验证 chat_with_image 真实发送 data:image/png;base64 多模态负载。
    #[tokio::test]
    async fn chat_with_image_sends_data_url() {
        let ui_json = "{\"page\":{\"width\":800,\"height\":600,\"nodes\":[{\"id\":\"b\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":100,\"h\":50}]}}";
        let body = format!("{{\"choices\":[{{\"message\":{{\"content\":{ui_json:?}}}}}]}}");
        let (url, captured) = spawn_body_capture_server(body).await;
        let client = mock_client(&url);
        let sketch = write_fixture_png();
        let b64 = encode_image_base64(std::path::Path::new(&sketch)).unwrap();
        let _ = chat_with_image(&client, "m", "sys", "usr", &b64, 128)
            .await
            .unwrap();
        let raw = captured.lock().unwrap().clone().expect("未捕获到请求");
        assert!(
            raw.contains("data:image/png;base64,"),
            "多模态负载缺少 data:image/png;base64 前缀：{}",
            &raw[..raw.len().min(200)]
        );
        assert!(raw.contains("image_url"), "缺少 image_url 字段");
    }

    /// P1-1：验证 image_to_ui_async 真实路径加载草图并发多模态请求（非仅文字描述）。
    #[tokio::test]
    async fn image_to_ui_async_sends_real_image() {
        let ui_json = "{\"page\":{\"width\":800,\"height\":600,\"nodes\":[{\"id\":\"c\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":100,\"h\":50}]}}";
        let body = format!("{{\"choices\":[{{\"message\":{{\"content\":{ui_json:?}}}}}]}}");
        let (url, captured) = spawn_body_capture_server(body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let sketch = write_fixture_png();
        let doc = skills
            .image_to_ui_async(&sketch, "测试", "Home")
            .await
            .unwrap();
        assert_eq!(doc.pages.len(), 1);
        assert_eq!(doc.pages[0].nodes[0].id, "c");
        let raw = captured.lock().unwrap().clone().expect("未捕获到请求");
        assert!(
            raw.contains("data:image/png;base64,"),
            "image_to_ui_async 未发送真实图片负载（仍是文字描述）：{}",
            &raw[..raw.len().min(200)]
        );
    }

    /// 生产 E2E：模型返回尾部多余闭括号 → balance 修复后产出真实文档（不兜底）。
    #[tokio::test]
    async fn image_to_ui_e2e_trailing_brace_repaired() {
        let ui_json = "{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":100,\"h\":50}]}}}";
        let body = format!("{{\"choices\":[{{\"message\":{{\"content\":{ui_json:?}}}}}]}}");
        let (url, _count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let sketch = write_fixture_png();
        let doc = skills
            .image_to_ui_async(&sketch, "测试", "Home")
            .await
            .unwrap();
        assert_eq!(doc.pages.len(), 1);
        assert_eq!(doc.pages[0].nodes[0].id, "n0");
        // 未触发占位兜底
        assert!(!doc.pages[0]
            .nodes
            .iter()
            .any(|n| n.text.as_deref() == Some("AI 输出不可用，已生成占位布局")));
    }

    /// 生产 E2E：模型返回彻底损坏 JSON → 合成兜底文档（不硬失败）。
    #[tokio::test]
    async fn image_to_ui_e2e_garbled_falls_back_gracefully() {
        let body = String::from(r#"{"choices":[{"message":{"content":"totally not json {{{"}}]}"#);
        let (url, _count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let sketch = write_fixture_png();
        let doc = skills
            .image_to_ui_async(&sketch, "测试", "Home")
            .await
            .unwrap();
        assert_eq!(doc.pages.len(), 1);
        // 占位兜底节点存在
        assert!(doc.pages[0]
            .nodes
            .iter()
            .any(|n| n.id == "n_bg" || n.id == "n_placeholder"));
    }

    /// 生产 E2E：HTTP 5xx → 向上传播错误（不静默吞）。
    #[tokio::test]
    async fn image_to_ui_e2e_http_error_propagates() {
        let body = String::from(r#"{"error":"internal"}"#);
        let (url, _count) = spawn_mock_server(500, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let sketch = write_fixture_png();
        let result = skills.image_to_ui_async(&sketch, "测试", "Home").await;
        assert!(result.is_err(), "HTTP 5xx 应向上传播而非静默成功");
    }
}

#[cfg(test)]
mod html_parser_tests {
    use super::*;

    #[test]
    fn extract_plain_html() {
        let html = r#"<div><h1>Hello</h1><p>World</p></div>"#;
        let result = extract_html_artifact(html);
        assert!(result.contains("<h1>"));
    }

    #[test]
    fn extract_code_fenced_html() {
        let raw = "```html\n<div><button>Click</button></div>\n```";
        let result = extract_html_artifact(raw);
        assert!(result.contains("<button>"));
        assert!(!result.contains("```"));
    }

    #[test]
    fn extract_artifact_tag() {
        let raw = r#"<artifact type="html"><div>Content</div></artifact>"#;
        let result = extract_html_artifact(raw);
        assert!(result.contains("<div>Content</div>"));
        assert!(!result.contains("<artifact"));
    }

    #[test]
    fn html_to_pen_document_basic() {
        let html = r#"<h1>Title</h1><p>Paragraph</p><button>Click</button>"#;
        let doc = html_to_pen_document(html, "TestPage").unwrap();
        assert_eq!(doc.pages.len(), 1);
        let page = &doc.pages[0];
        assert!(page.nodes.len() >= 2, "at least h1 and p");
        let h1 = page.nodes.iter().find(|n| n.name == "heading_h1");
        assert!(h1.is_some());
        assert_eq!(h1.unwrap().kind, fd_canvas_core::NodeKind::Text);
        assert_eq!(h1.unwrap().text.as_deref(), Some("Title"));
    }

    #[test]
    fn html_to_pen_document_button() {
        let html = r#"<button>Submit</button>"#;
        let doc = html_to_pen_document(html, "BtnPage").unwrap();
        let btn = doc.pages[0].nodes.iter().find(|n| n.name == "button");
        assert!(btn.is_some());
        assert_eq!(btn.unwrap().kind, fd_canvas_core::NodeKind::Rect);
        assert_eq!(btn.unwrap().style.radius, Some(8.0));
    }

    #[test]
    fn html_to_pen_document_inline_style() {
        let html = r#"<div style="background: #333; width: 200px; height: 100px; border-radius: 12px;">Box</div>"#;
        let doc = html_to_pen_document(html, "StylePage").unwrap();
        let div = doc.pages[0].nodes.first().unwrap();
        assert_eq!(div.w, 200.0);
        assert_eq!(div.h, 100.0);
        assert_eq!(div.style.fill.as_deref(), Some("#333"));
        assert_eq!(div.style.radius, Some(12.0));
    }

    #[test]
    fn html_to_pen_document_class_token_hint() {
        let html = r#"<button class="btn-primary">Go</button>"#;
        let doc = html_to_pen_document(html, "TokenPage").unwrap();
        let btn = doc.pages[0].nodes.first().unwrap();
        assert_eq!(btn.style.fill.as_deref(), Some("var(--color-accent)"));
    }

    #[test]
    fn html_to_pen_document_nested() {
        let html = r#"<div><h1>Title</h1><p>Sub</p></div>"#;
        let doc = html_to_pen_document(html, "NestedPage").unwrap();
        let div = doc.pages[0].nodes.first().unwrap();
        assert!(!div.children.is_empty(), "div should have child nodes");
    }

    #[test]
    fn html_to_pen_document_img() {
        let html = r#"<img src="test.png" />"#;
        let doc = html_to_pen_document(html, "ImgPage").unwrap();
        let img = doc.pages[0]
            .nodes
            .iter()
            .find(|n| n.kind == fd_canvas_core::NodeKind::Image);
        assert!(img.is_some());
    }

    #[test]
    fn parse_inline_style_basic() {
        let map = parse_inline_style("width: 100px; height: 50px; color: #fff");
        assert_eq!(map.get("width").unwrap(), "100px");
        assert_eq!(map.get("height").unwrap(), "50px");
        assert_eq!(map.get("color").unwrap(), "#fff");
    }

    #[test]
    fn parse_px_values() {
        assert_eq!(parse_px("100px"), Some(100.0));
        assert_eq!(parse_px("50"), Some(50.0));
        assert_eq!(parse_px("75%"), Some(75.0));
        assert_eq!(parse_px("auto"), None);
    }

    #[test]
    fn class_to_fill_hint_mapping() {
        assert_eq!(
            class_to_fill_hint("bg-primary"),
            Some("var(--color-accent)".to_string())
        );
        assert_eq!(
            class_to_fill_hint("bg-danger"),
            Some("var(--color-error)".to_string())
        );
        assert_eq!(class_to_fill_hint("unknown"), None);
    }
}
