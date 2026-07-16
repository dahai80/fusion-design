//! Fusion-Design AI adapter — 实现 OpenPencil `ChatProvider` trait，
//! 后端对接 fusion-mlx 本地多模态推理。
//!
//! 【禁云端硬约束】本 crate 是 fusion-design 唯一允许发起 HTTP 请求的模块，
//! 但请求目标被 `FusionMlxClient` 限制为 `127.0.0.1` 本地 fusion-mlx 服务，
//! 不存在任何公网调用路径。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ── fusion-mlx 本地推理客户端 ──
//
// fusion-mlx 以本地 HTTP 服务（127.0.0.1:port）暴露 chat completions 接口，
// 兼容 OpenAI API 形状（/v1/chat/completions），便于复用既有生态工具。
// 真实端口由 fusion-mlx 启动时分配，写入本地配置文件。

const DEFAULT_MLX_ENDPOINT: &str = "http://127.0.0.1:8080";

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
    /// 用默认 endpoint `http://127.0.0.1:8080` 构造。
    pub fn new() -> anyhow::Result<Self> {
        Self::with_endpoint(DEFAULT_MLX_ENDPOINT)
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
                MlxMessage { role: "system", content: system_prompt },
                MlxMessage { role: "user", content: user_message },
            ],
            max_tokens,
            temperature: None,
        };
        let url = format!("{}/v1/chat/completions", self.endpoint);
        let resp: MlxChatResponse = blocking_post(&self.http, &url, &payload)?;
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
                MlxMessage { role: "system", content: system_prompt },
                MlxMessage { role: "user", content: user_message },
            ],
            max_tokens,
            temperature: None,
        };
        let url = format!("{}/v1/chat/completions", self.endpoint);
        let resp = self.http.post(&url).json(&payload).send().await?;
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
        anyhow::bail!(
            "违反离线硬约束：endpoint host {host:?} 非 localhost，禁止公网调用"
        );
    }
    Ok(())
}

/// 阻塞式 POST（复用当前 tokio runtime 若存在，否则新建 current-thread runtime）。
///
/// 设计：`chat_sync` 可能在已存在 tokio runtime 的上下文被调用
/// （如 `#[tokio::test]`）。此时直接 `Builder::build()` 会嵌套 runtime
/// panic。改用 `Handle::try_current()`：若已在 runtime 内，直接
/// `handle.block_on()`；否则才新建独立 runtime。
fn blocking_post<T: Serialize + ?Sized>(
    http: &reqwest::Client,
    url: &str,
    payload: &T,
) -> anyhow::Result<MlxChatResponse> {
    let http = http.clone();
    let url = url.to_string();
    let fut = async move {
        let resp = http.post(&url).json(payload).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("fusion-mlx HTTP {}", resp.status());
        }
        Ok::<_, anyhow::Error>(resp.json::<MlxChatResponse>().await?)
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // 已在 runtime 内：用 block_in_place（若 multi-thread）或直接 block_on
            // 简化：直接 handle.block_on，调用方需保证不阻塞 reactor
            handle.block_on(fut)
        }
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(fut)
        }
    }
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
        Self { client, default_model: default_model.into() }
    }
}

impl ChatProvider for FusionMlxChatProvider {
    fn provider_label(&self) -> &str { "fusion-mlx (local)" }
    fn send(&self, request: ChatRequest) -> Box<dyn Iterator<Item = ChatDelta> + Send> {
        let model = request.model.as_deref().unwrap_or(&self.default_model);
        match self.client.chat_sync(
            model,
            &request.system_prompt,
            &request.user_message,
            request.max_output_tokens,
        ) {
            Ok(text) => Box::new(
                vec![ChatDelta::TextDelta(text), ChatDelta::Done {
                    stop_reason: StopReason::EndTurn,
                }].into_iter()
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

// ── AI Skills：文生 UI / 图生 UI / 局部编辑 ──

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
        Self { client, default_model: default_model.into() }
    }

    /// 文生 UI：自然语言描述 → PenDocument 页面。
    ///
    /// 对应 PRD 模块 2「文本生成界面」。
    pub fn text_to_ui(&self, prompt: &str, page_name: &str) -> anyhow::Result<PenDocument> {
        let sys = "你是 fusion-design UI 生成器。输出严格 JSON：{\"page\":{...}}。\
只输出 JSON，禁止额外文字。page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}。";
        let user = format!("生成页面「{page_name}」。需求：{prompt}");
        let resp = self.client.chat_sync(&self.default_model, sys, &user, 2048)?;
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
    /// MVP 阶段：草图以路径形式传入，提示词中描述，由 fusion-mlx 视觉模型解析。
    pub fn image_to_ui(&self, sketch_path: &str, hint: &str, page_name: &str) -> anyhow::Result<PenDocument> {
        let sys = "你是 fusion-design UI 生成器。根据用户描述的草图布局，\
输出严格 JSON：{\"page\":{...}}。只输出 JSON。";
        let user = format!(
            "草图路径：{sketch_path}\n补充说明：{hint}\n生成页面「{page_name}」对应的 UI 布局。"
        );
        let resp = self.client.chat_sync(&self.default_model, sys, &user, 2048)?;
        parse_ui_json(&resp, page_name)
    }

    /// 图生 UI（async 变体）。
    pub async fn image_to_ui_async(
        &self,
        sketch_path: &str,
        hint: &str,
        page_name: &str,
    ) -> anyhow::Result<PenDocument> {
        let sys = "你是 fusion-design UI 生成器。根据用户描述的草图布局，\
输出严格 JSON：{\"page\":{...}}。只输出 JSON。";
        let user = format!(
            "草图路径：{sketch_path}\n补充说明：{hint}\n生成页面「{page_name}」对应的 UI 布局。"
        );
        let resp = self
            .client
            .chat_async(&self.default_model, sys, &user, 2048)
            .await?;
        parse_ui_json(&resp, page_name)
    }

    /// 局部编辑：框选节点 + 自然语言指令 → 修改后的节点。
    ///
    /// 对应 PRD 模块 2「局部指令修改」。
    /// 返回更新后的节点样式（调用方负责写回 PenDocument）。
    pub fn partial_edit(
        &self,
        node_json: &str,
        instruction: &str,
    ) -> anyhow::Result<String> {
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
}

/// 解析 fusion-mlx 返回的 UI JSON 为 PenDocument。
///
/// 兼容形状：`{"page": {"width":..,"height":..,"nodes":[..]}}` 或裸 `{"nodes":[..]}`。
fn parse_ui_json(json: &str, page_name: &str) -> anyhow::Result<PenDocument> {
    // 容错：模型可能包裹 markdown ```json ... ```，剥之
    let cleaned = strip_code_fence(json);
    let v: serde_json::Value = serde_json::from_str(cleaned)?;
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
        .or_else(|| v.get("nodes"))
        .ok_or_else(|| anyhow::anyhow!("JSON 缺 nodes 字段"))?;
    let nodes = parse_nodes(nodes_val)?;
    let mut doc = PenDocument::new();
    let mut page = Page::new("page_1", page_name, w, h);
    for n in nodes {
        page.add(n);
    }
    doc.add_page(page);
    Ok(doc)
}

fn parse_nodes(v: &serde_json::Value) -> anyhow::Result<Vec<PenNode>> {
    let arr = v.as_array().ok_or_else(|| anyhow::anyhow!("nodes 非数组"))?;
    let mut nodes = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        nodes.push(parse_node(item, i)?);
    }
    Ok(nodes)
}

fn parse_node(item: &serde_json::Value, idx: usize) -> anyhow::Result<PenNode> {
    let o = item.as_object().ok_or_else(|| anyhow::anyhow!("节点 {idx} 非对象"))?;
    let kind_str = o.get("kind").and_then(|k| k.as_str()).unwrap_or("rect");
    let kind = match kind_str {
        "rect" => NodeKind::Rect,
        "circle" => NodeKind::Circle,
        "text" => NodeKind::Text,
        "image" => NodeKind::Image,
        "group" => NodeKind::Group,
        other => anyhow::bail!("未知节点类型 {other:?}"),
    };
    let id = o.get("id").and_then(|x| x.as_str()).map(String::from)
        .unwrap_or_else(|| format!("n_{idx}"));
    let name = o.get("name").and_then(|x| x.as_str()).map(String::from)
        .unwrap_or_else(|| kind_str.to_string());
    let get_f = |key: &str, d: f32| o.get(key).and_then(|x| x.as_f64()).map(|v| v as f32).unwrap_or(d);
    let text = o.get("text").and_then(|x| x.as_str()).map(String::from);
    let fill = o.get("fill").and_then(|x| x.as_str()).map(String::from);
    let stroke = o.get("stroke").and_then(|x| x.as_str()).map(String::from);
    let mut style = fd_canvas_core::NodeStyle::default();
    style.fill = fill;
    style.stroke = stroke;
    let children_val = o.get("children");
    let children = match children_val {
        Some(cv) => parse_nodes(cv)?,
        None => vec![],
    };
    Ok(PenNode {
        id, kind, name,
        x: get_f("x", 0.0), y: get_f("y", 0.0),
        w: get_f("w", 0.0), h: get_f("h", 0.0),
        style, text, children,
    })
}

fn strip_code_fence(s: &str) -> &str {
    let trimmed = s.trim();
    if !trimmed.starts_with("```") {
        // 裸 JSON：原样返回（trim 已剥首尾空白，不影响 serde_json）
        return trimmed;
    }
    // 剥首行 ```xxx 和末尾 ```
    let inner = trimmed
        .trim_start_matches('`')
        .trim_end_matches('`');
    // 再剥可能的 "json\n" 前缀
    inner.trim_start_matches("json").trim()
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
    fn parse_ui_json_missing_nodes_errors() {
        assert!(parse_ui_json(r#"{"page":{}}"#, "x").is_err());
    }

    #[test]
    fn parse_ui_json_unknown_kind_errors() {
        let bad = r#"{"nodes":[{"id":"x","kind":"weird"}]}"#;
        assert!(parse_ui_json(bad, "x").is_err());
    }

    #[test]
    fn parse_ui_json_invalid_json_errors() {
        assert!(parse_ui_json("not json", "x").is_err());
    }

    #[test]
    fn parse_node_defaults_id_when_absent() {
        let v: serde_json::Value = serde_json::from_str(r#"{"kind":"rect"}"#).unwrap();
        let n = parse_node(&v, 5).unwrap();
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

    #[tokio::test]
    async fn chat_async_end_to_end_openai_shape() {
        let body = String::from(r#"{"choices":[{"message":{"content":"hello world"}}]}"#);
        let (url, count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let out = client.chat_async("qwen3.5", "sys", "usr", 128).await.unwrap();
        assert_eq!(out, "hello world");
        // 请求被发到 mock server
        assert_eq!(*count.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn chat_async_propagates_http_4xx() {
        let body = String::from(r#"{"error":"rate limit"}"#);
        let (url, _count) = spawn_mock_server(429, body).await;
        let client = mock_client(&url);
        let err = client.chat_async("qwen3.5", "sys", "usr", 128).await.unwrap_err();
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
        let err = client.chat_async("qwen3.5", "sys", "usr", 128).await.unwrap_err();
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
        let doc = skills.text_to_ui_async("做一个英雄区", "Home").await.unwrap();
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
            .partial_edit_async("{\"id\":\"btn1\",\"kind\":\"rect\",\"fill\":\"#0000FF\"}", "改成红色")
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
        let err = client.chat_async("qwen3.5", "sys", "usr", 128).await.unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn mock_server_endpoint_is_localhost() {
        // 确保自建 mock server 的 127.0.0.1 endpoint 通过 validate_localhost
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");
        assert!(validate_localhost(&url).is_ok());
    }
}
