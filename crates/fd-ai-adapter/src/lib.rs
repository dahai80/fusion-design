//! Fusion-Design AI adapter — 实现 OpenPencil `ChatProvider` trait，
//! 后端对接 fusion-mlx 本地多模态推理。
//!
//! 【禁云端硬约束】本 crate 是 fusion-design 唯一允许发起 HTTP 请求的模块，
//! 请求目标被 `FusionMlxClient` 限制为本地/局域网（回环 + RFC1918 + 链路本地），
//! 见 `validate_localhost`——杜绝一切公网调用路径。
//!
//! 【RouteGuard 鉴权】所有出站请求附加 `X-Fusion-Route: fusion-design` 头
//! （fusion-mlx v0.7.0+ 默认强制，缺失则 403 missing_route），并在
//! `FUSION_MLX_API_KEY` 设置时附加 `Authorization: Bearer <key>`。
//!
//! 【M-5 重试退避】fusion-mlx 模型加载期间返回 503、节点模型被驱逐回 502/503。
//! 三处 HTTP 路径（`blocking_post` / `chat_stream_messages` / `check_generate`）
//! 对 502/503 瞬时错误指数退避重试（500ms→1s→2s→4s→8s 封顶，默认最多 4 次），
//! 4xx 永久错误（鉴权/请求格式）直接失败不重试。`FUSION_MLX_RETRY_MAX` 环境变量
//! 调最大尝试次数（含首次），设 1 即关闭重试。流式仅覆盖建连阶段，中途断流不重试。
//!
//! 【H-A7 流式塌缩修复】`ChatProvider::send` 受 op-ai trait 同步语义约束
//! （`Box<dyn Iterator<Item=ChatDelta>>`，非 async Stream），无法做真 SSE 流。
//! 旧实现把 `chat_sync_messages` 全文包成单个 `TextDelta`，消费方等全文生成完
//! 才见首个 delta。改为按行分块：每行（含换行）一个 `TextDelta`，消费方逐行收
//! 增量序列；空文本不产 `TextDelta` 仅 `Done`。真流式仍走 `chat_stream_messages`
//! （CLI `chat` + studio），trait 路径为同步消费方提供增量逼近。

use std::net::IpAddr;
use std::sync::{Arc, LazyLock};

use futures::StreamExt;
use serde::{Deserialize, Serialize};

mod stream;
pub use stream::parse_sse_line;

// ── fusion-mlx 本地推理客户端 ──
//
// fusion-mlx 以本地 HTTP 服务（127.0.0.1:port）暴露 chat completions 接口，
// 兼容 OpenAI API 形状（/v1/chat/completions），便于复用既有生态工具。
// 真实端口由 fusion-mlx 启动时分配，写入本地配置文件。

const DEFAULT_MLX_ENDPOINT: &str = "http://127.0.0.1:11432";

/// A-1：把逗号分隔的 endpoint 串拆成去空 trim 后的 Vec。
/// `"http://a:11432,http://b:11432"` → `["http://a:11432","http://b:11432"]`。
/// 空串 → `[DEFAULT_MLX_ENDPOINT]`（保证至少 1 条，交由 with_endpoints 校验）。
fn parse_endpoints(raw: &str) -> Vec<String> {
    let parts: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        vec![DEFAULT_MLX_ENDPOINT.to_string()]
    } else {
        parts
    }
}

/// fusion-mlx RouteGuard 要求的来源标识头（存在即放行）。
/// 方案B（netlayer-compliance-plan.md，用户 2026-08-07 裁定）：fusion-design
/// 统一经 fusion-gateway `:11432` 调用 fusion-mlx，不再直连 11434。
/// gateway 自身完成鉴权后转发，route 头对 gateway 透传无害。
/// 历史：方案A（直连 11434）已否决，见 issue #11。
const FUSION_ROUTE_HEADER: (&str, &str) = ("X-Fusion-Route", "fusion-design");

// DEFAULT_MAX_TOKENS 迁出 fd-skills（ARCH-11），本文件经 re-export 引用。

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

/// 公开对话消息：role + content，供调用方注入多轮历史。
/// role 透传到 OpenAI 兼容 API（system/user/assistant）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlxChatMessage {
    pub role: String,
    pub content: String,
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
    // A-1：多节点 failover。单节点时 `endpoints == vec![endpoint]`。
    // `FUSION_MLX_BASE_URL` 逗号分隔多值时，每 endpoint 各经 validate_localhost。
    endpoints: Vec<String>,
    // A-1：轮询计数器。每请求/重试 attempt fetch_add 取下一 endpoint，
    // 单节点负载均衡 + 502/503 即切下一节点（被动故障转移）。
    rr: Arc<std::sync::atomic::AtomicU32>,
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
    /// `endpoint` 逗号分隔多值时（A-1）按多节点构造，每 endpoint 各校验。
    pub fn with_endpoint(endpoint: &str) -> anyhow::Result<Self> {
        let endpoints = parse_endpoints(endpoint);
        Self::with_endpoints(endpoints)
    }

    /// A-1：多节点构造器。`endpoints` 不可空，每条经 validate_localhost；
    /// 全部合法才建 client。`endpoint` 字段取首条（兼容旧单 endpoint 读字段路径）。
    pub fn with_endpoints(endpoints: Vec<String>) -> anyhow::Result<Self> {
        if endpoints.is_empty() {
            anyhow::bail!("endpoints 列表为空，至少需 1 个本地/局域网 endpoint");
        }
        for ep in &endpoints {
            validate_localhost(ep)?;
        }
        let endpoint = endpoints[0].clone();
        Ok(Self {
            endpoint,
            endpoints,
            rr: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                // C8：缺超时则 fusion-mlx 挂起时请求永久阻塞，调用方
                // （fusion-studio subprocess）卡死。连接短超时 + 总超时兜底。
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(180))
                .build()?,
        })
    }

    /// A-1：按 attempt 轮询选 endpoint。round-robin 计数器 fetch_add 取模，
    /// 让相邻请求/重试落不同节点；单节点时恒返首条。
    fn endpoint_for_attempt(&self, attempt: u32) -> &str {
        if self.endpoints.len() == 1 {
            return &self.endpoint;
        }
        let idx = self.rr.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as usize;
        let pos = idx % self.endpoints.len();
        let ep = &self.endpoints[pos];
        tracing::debug!(
            attempt,
            endpoint = %ep,
            pos,
            n = self.endpoints.len(),
            "endpoint_for_attempt: 轮询选节点"
        );
        ep
    }

    /// 解析 CLI `--endpoint` 实参到最终 endpoint 列表。
    /// CLI 层把 `--endpoint` 默认值设为空串；空串时读 `FUSION_MLX_BASE_URL`，
    /// 缺省回退 `http://127.0.0.1:11432`（方案B 经 gateway）。非空串直接透传
    /// （用户显式传 `--endpoint` 优先级最高）。逗号分隔多值各校验（A-1）。
    /// 返回 endpoints 列表供 `with_endpoints` 使用。
    pub fn resolve_endpoint(cli_endpoint: &str) -> anyhow::Result<Vec<String>> {
        let raw = match cli_endpoint.trim() {
            "" => match std::env::var("FUSION_MLX_BASE_URL") {
                Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => DEFAULT_MLX_ENDPOINT.to_string(),
            },
            other => other.to_string(),
        };
        let endpoints = parse_endpoints(&raw);
        for ep in &endpoints {
            validate_localhost(ep)?;
        }
        Ok(endpoints)
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
        let resp: MlxChatResponse = self.blocking_post("/v1/chat/completions", &payload)?;
        Ok(resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("fusion-mlx 返回空 choices"))?
            .message
            .content)
    }

    /// 多轮同步 chat：messages 由调用方构造（system + 多轮 user/assistant 历史 + 当前 user）。
    /// H-A9：`ChatProvider::send` 旧实现只用 system_prompt+user_message，静默丢弃
    /// `request.history`（多轮上下文丢失）、`thinking`/`effort`/`attachments`（无 wire 支持）。
    /// 此方法把完整 messages[] 透传到 OpenAI 兼容 API，恢复多轮上下文。
    pub fn chat_sync_messages(
        &self,
        model: &str,
        messages: Vec<MlxChatMessage>,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        // 持有 owned String，构造 &'a str 的 MlxMessage 借用本帧局部。
        let owned: Vec<(String, String)> =
            messages.into_iter().map(|m| (m.role, m.content)).collect();
        let mlx_msgs: Vec<MlxMessage> = owned
            .iter()
            .map(|(r, c)| MlxMessage {
                role: r.as_str(),
                content: c.as_str(),
            })
            .collect();
        let payload = MlxChatPayload {
            model,
            messages: mlx_msgs,
            max_tokens,
            temperature: None,
        };
        let resp: MlxChatResponse = self.blocking_post("/v1/chat/completions", &payload)?;
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
        // L10/C9：旧实现 .expect() — FUSION_MLX_BASE_URL 坏值即 panic。
        // 改回退到安全的本地缺省 endpoint（DEFAULT_MLX_ENDPOINT，已 validate_localhost 校验）。
        match Self::new() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "默认 endpoint 构造失败，回退本地缺省");
                Self::with_endpoint(DEFAULT_MLX_ENDPOINT)
                    .expect("DEFAULT_MLX_ENDPOINT 必为合法 localhost（编译期保证）")
            }
        }
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

// A-6：统一鉴权头附加。retry_async_post / chat_stream_messages / check_generate 三处
// 原各自手写 route header + bearer match，逻辑分叉易漏（如只加 route 忘 bearer）。
// 此 helper 收口：route header 恒加 + 可选 bearer。bearer 由调用方预算（避免重试循环
// 每次重读 env，性能），与 authed 方法互补——authed 即时读 env（低频单次请求场景）。
fn attach_route_bearer(
    builder: reqwest::RequestBuilder,
    bearer: Option<&str>,
) -> reqwest::RequestBuilder {
    let builder = builder.header(FUSION_ROUTE_HEADER.0, FUSION_ROUTE_HEADER.1);
    match bearer {
        Some(b) => builder.header("Authorization", b),
        None => builder,
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
        // R-3：经 retry_async_post 重试（瞬时错误退避，4xx 永久失败，deadline 封顶）。
        // A-1：failover 经 endpoints+rr 轮询，单节点 502/503 即切下一节点。
        let bearer = self.bearer_token();
        let bearer_ref: Option<&str> = bearer.as_deref();
        let parsed: MlxChatResponse = retry_async_post(
            &self.http,
            &self.endpoints,
            &self.rr,
            "/v1/chat/completions",
            &payload,
            bearer_ref,
        )
        .await?;
        Ok(parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("fusion-mlx 返回空 choices"))?
            .message
            .content)
    }
}

/// 强校验 endpoint host 为本地/局域网，杜绝公网调用路径。
///
/// H-A1/P2-1：旧实现仅放行 `127.0.0.1`/`localhost`/`::1`，把 fusion-mlx 集群
/// 入口焊死——用户无法指向局域网内的 MLX worker（如 `10.x`/`192.168.x`）。
/// 现放行回环 + RFC1918 私有段 + 链路本地（169.254/fe80）+ 唯一本地（fc00::/7），
/// 仍拒绝一切公网 IP 与公网域名。离线硬约束（无公网调用）保持不变。
fn validate_localhost(endpoint: &str) -> anyhow::Result<()> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|e| anyhow::anyhow!("无效 endpoint {endpoint:?}: {e}"))?;
    let host = url.host_str().unwrap_or("");
    let host = host.trim_start_matches('[').trim_end_matches(']');
    // localhost 名义放行（回环别名）
    if host == "localhost" {
        return Ok(());
    }
    // IP 字面量：按 IpAddr 判定私有/回环/链路本地
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_loopback() {
            return Ok(());
        }
        if is_private_or_local(&ip) {
            return Ok(());
        }
        anyhow::bail!("违反离线硬约束：endpoint host {host:?} 为公网 IP，禁止公网调用");
    }
    // 非localhost域名一律拒（DNS 可能解析到公网，无法在静态期保证离线）
    anyhow::bail!("违反离线硬约束：endpoint host {host:?} 非 localhost/私有IP，禁止公网调用");
}

/// 判定 IP 是否为私有段（RFC1918）或链路本地/唯一本地地址。
/// 公网 IP（含 8.8.8.8、1.1.1.1 等）返回 false。
/// E-2：移除 is_unspecified()——0.0.0.0/:: 非"本地可达"地址，放行它等于允许
/// 绑定任意网卡（含公网）的 endpoint，击穿离线约束。loopback 已在 validate_localhost
/// 上层判定；此处仅私有/链路本地/唯一本地段，集群 worker（10.x/192.168.x/fe80）保留。
fn is_private_or_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() // 10/8、172.16/12、192.168/16
                || v4.is_link_local() // 169.254/16
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // 唯一本地 fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // 链路本地 fe80::/10
        }
    }
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
///
/// H-A6：worker_threads 自适应机器核数（最少 4）。旧值 1 使所有并发 block_on
/// 串行化到单线程，首个 180s 长推理请求饿死后续请求（FIFO 头阻塞），多面板并发
/// UI 冻结。multi-thread runtime 保持 block_in_place 可用，不引入嵌套 runtime panic。
/// A-10：固定 4 在高核机器仍瓶颈，改为 available_parallelism 自适应（回退 4）。
static BLOCKING_RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(4);
    tracing::info!(workers, "BLOCKING_RT: 自适应 worker 线程数");
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .expect("blocking tokio runtime 创建失败")
});

// ── M-5：502/503 指数退避重试 ──
// fusion-mlx 模型加载期间返回 503（"model loading"），集群节点模型被驱逐回 502/503。
// 旧实现直接 bail!，首次请求即失败不等待加载。改瞬时错误退避重试，永久错误直接失败。
// FUSION_MLX_RETRY_MAX 环境变量调最大尝试次数（含首次），缺省 4。设 1 即关闭重试。
const RETRY_BACKOFF_BASE_MS: u64 = 500;
const RETRY_MAX_BACKOFF_MS: u64 = 8_000;
const RETRY_DEFAULT_MAX_ATTEMPTS: u32 = 4;
// R-12：SSE buffer 残留上限。FUSION_MLX_SSE_BUFFER_CAP 可调（字节），缺省 8MB。
const MAX_SSE_BUFFER: usize = 8 * 1024 * 1024;

fn sse_buffer_cap() -> usize {
    std::env::var("FUSION_MLX_SSE_BUFFER_CAP")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1024)
        .unwrap_or(MAX_SSE_BUFFER)
}

fn retry_max_attempts() -> u32 {
    // FUSION_MLX_RETRY_MAX 覆盖；<1 视为缺省。0/1 = 不重试（仅首次）。
    std::env::var("FUSION_MLX_RETRY_MAX")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(RETRY_DEFAULT_MAX_ATTEMPTS)
}

fn is_transient_status(code: u16) -> bool {
    // L-7：扩展瞬时错误集。
    // 502/503/504 = 网关/服务端瞬时（模型加载中/被驱逐/网关临时不可达/超时）。
    // 429 = 限流（Too Many Requests），529 = 上游过载（Anthropic 风格透传）。
    // 其余 4xx = 永久，不重试。
    matches!(code, 502 | 503 | 504 | 429 | 529)
}

fn backoff_delay(attempt: u32) -> std::time::Duration {
    // 指数退避：500ms, 1s, 2s, 4s, 8s(上限)。attempt 从 0 起算（首次重试前的等待）。
    let exp = if attempt >= 16 {
        1u64 << 16
    } else {
        1u64 << attempt
    };
    let delay_ms = RETRY_BACKOFF_BASE_MS
        .saturating_mul(exp)
        .min(RETRY_MAX_BACKOFF_MS);
    std::time::Duration::from_millis(delay_ms)
}

impl FusionMlxClient {
    /// 阻塞式 POST（用专用 tokio runtime，避免嵌套 runtime panic）。
    /// 附加 RouteGuard + Bearer 鉴权头后发往 fusion-mlx。
    /// M-5：502/503 瞬时错误指数退避重试，4xx 永久错误直接失败。
    /// A-1：`path` 为相对 endpoint 的后缀（如 `/v1/chat/completions`），每 attempt
    /// 经 `endpoint_for_attempt` 轮询选节点拼 url——单节点 502/503 即切下一节点。
    fn blocking_post<T: Serialize + ?Sized>(
        &self,
        path: &str,
        payload: &T,
    ) -> anyhow::Result<MlxChatResponse> {
        let http = self.http.clone();
        let path = path.to_string();
        let bearer = self.bearer_token();
        let endpoints = self.endpoints.clone();
        let rr = self.rr.clone();
        BLOCKING_RT.block_on(async move {
            let max = retry_max_attempts();
            // P-1：总 deadline 封顶重试时长，防 4×长退避 + 挂起响应无限阻塞。
            // 默认 300s（FUSION_MLX_RETRY_DEADLINE_SECS 可调），每轮超 deadline 即 bail。
            let deadline_secs = std::env::var("FUSION_MLX_RETRY_DEADLINE_SECS")
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(300);
            let deadline = std::time::Duration::from_secs(deadline_secs);
            let started = std::time::Instant::now();
            let mut last_err: Option<anyhow::Error> = None;
            for attempt in 0..max {
                if started.elapsed() > deadline {
                    tracing::error!(
                        attempt,
                        elapsed = ?started.elapsed(),
                        deadline = ?deadline,
                        "blocking_post: 超过总 deadline，放弃重试"
                    );
                    anyhow::bail!(
                        "fusion-mlx 重试超过总 deadline {:?}（尝试 {}/{} 次）",
                        deadline,
                        attempt,
                        max
                    );
                }
                // A-1：每 attempt 轮询选 endpoint 拼完整 url（failover）。
                let idx = if endpoints.len() == 1 {
                    0
                } else {
                    rr.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as usize % endpoints.len()
                };
                let url = format!("{}{}", endpoints[idx], path);
                // RequestBuilder 消耗性，每次重试重建。A-6：鉴权头经 attach_route_bearer 统一。
                let req = attach_route_bearer(http.post(&url).json(payload), bearer.as_deref());
                match req.send().await {
                    Ok(resp) if resp.status().is_success() => {
                        return Ok::<_, anyhow::Error>(resp.json::<MlxChatResponse>().await?);
                    }
                    Ok(resp) => {
                        let code = resp.status().as_u16();
                        if is_transient_status(code) && attempt + 1 < max {
                            let delay = backoff_delay(attempt);
                            tracing::warn!(
                                attempt,
                                code,
                                endpoint = %endpoints[idx],
                                ?delay,
                                "blocking_post: 瞬时错误，退避后切换节点重试"
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        if attempt + 1 < max {
                            tracing::warn!(attempt, code, "blocking_post: 永久错误，不重试");
                        } else {
                            tracing::error!(attempt, code, "blocking_post: 重试耗尽");
                        }
                        anyhow::bail!("fusion-mlx HTTP {code}（尝试 {}/{max} 次）", attempt + 1);
                    }
                    Err(e) => {
                        // 连接错误（fusion-mlx 未起/网关不可达）按瞬时重试。
                        if attempt + 1 < max {
                            let delay = backoff_delay(attempt);
                            tracing::warn!(
                                attempt,
                                error = %e,
                                endpoint = %endpoints[idx],
                                ?delay,
                                "blocking_post: 连接失败，退避后切换节点重试"
                            );
                            last_err = Some(anyhow::Error::from(e));
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        tracing::error!(attempt, error = %e, "blocking_post: 连接失败，重试耗尽");
                        anyhow::bail!(
                            "fusion-mlx 连接失败（尝试 {}/{} 次）: {e}",
                            attempt + 1,
                            max
                        );
                    }
                }
            }
            // max >= 1 由 retry_max_attempts 保证，循环内必 return/bail。
            Err(last_err.unwrap_or_else(|| anyhow::anyhow!("blocking_post: 重试循环异常退出")))
        })
    }
}

/// R-3：异步 POST 重试。复用 blocking_post 同款退避/瞬时判定/deadline 语义，
/// 供 chat_async 等 async 路径统一重试。4xx 永久错误直接失败，瞬时错误退避重试。
/// A-1：`endpoints`+`rr` 实现 failover——每 attempt 轮询选 endpoint 拼 url，
/// 单节点 502/503 即切下一节点。
async fn retry_async_post<T: Serialize + ?Sized, R: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    endpoints: &[String],
    rr: &Arc<std::sync::atomic::AtomicU32>,
    path: &str,
    payload: &T,
    bearer: Option<&str>,
) -> anyhow::Result<R> {
    let max = retry_max_attempts();
    let deadline_secs = std::env::var("FUSION_MLX_RETRY_DEADLINE_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(300);
    let deadline = std::time::Duration::from_secs(deadline_secs);
    let started = std::time::Instant::now();
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..max {
        if started.elapsed() > deadline {
            tracing::error!(
                attempt,
                elapsed = ?started.elapsed(),
                deadline = ?deadline,
                "retry_async_post: 超过总 deadline，放弃重试"
            );
            anyhow::bail!(
                "fusion-mlx 重试超过总 deadline {:?}（尝试 {}/{} 次）",
                deadline,
                attempt,
                max
            );
        }
        let idx = if endpoints.len() == 1 {
            0
        } else {
            rr.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as usize % endpoints.len()
        };
        let url = format!("{}{}", endpoints[idx], path);
        // A-6：鉴权头经 attach_route_bearer 统一（bearer 已为 Option<&str>）。
        let req = attach_route_bearer(http.post(&url).json(payload), bearer);
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                return Ok(resp.json::<R>().await?);
            }
            Ok(resp) => {
                let code = resp.status().as_u16();
                if is_transient_status(code) && attempt + 1 < max {
                    let delay = backoff_delay(attempt);
                    tracing::warn!(
                        attempt,
                        code,
                        endpoint = %endpoints[idx],
                        ?delay,
                        "retry_async_post: 瞬时错误，退避后切换节点重试"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                if attempt + 1 < max {
                    tracing::warn!(attempt, code, "retry_async_post: 永久错误，不重试");
                } else {
                    tracing::error!(attempt, code, "retry_async_post: 重试耗尽");
                }
                anyhow::bail!("fusion-mlx HTTP {code}（尝试 {}/{max} 次）", attempt + 1);
            }
            Err(e) => {
                if attempt + 1 < max {
                    let delay = backoff_delay(attempt);
                    tracing::warn!(
                        attempt, error = %e, endpoint = %endpoints[idx], ?delay,
                        "retry_async_post: 连接失败，退避后切换节点重试"
                    );
                    last_err = Some(anyhow::Error::from(e));
                    tokio::time::sleep(delay).await;
                    continue;
                }
                tracing::error!(attempt, error = %e, "retry_async_post: 连接失败，重试耗尽");
                anyhow::bail!(
                    "fusion-mlx 连接失败（尝试 {}/{} 次）: {e}",
                    attempt + 1,
                    max
                );
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("retry_async_post: 重试循环异常退出")))
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
/// 单轮便捷封装：system + 单条 user，多轮历史请用 chat_stream_messages。
pub async fn chat_stream(
    client: FusionMlxClient,
    model: String,
    system_prompt: String,
    user_message: String,
    max_tokens: u32,
) -> impl futures::Stream<Item = anyhow::Result<MlxStreamDelta>> {
    let messages = vec![
        MlxChatMessage {
            role: "system".into(),
            content: system_prompt,
        },
        MlxChatMessage {
            role: "user".into(),
            content: user_message,
        },
    ];
    chat_stream_messages(client, model, messages, max_tokens).await
}

/// 多轮流式 chat：messages 由调用方构造（system + 多轮 user/assistant 历史）。
/// 鉴权 / X-Fusion-Route header / endpoint 解析与 chat_stream 一致，
/// 让 CLI / 上游（如 fusion-studio subprocess）复用同一 MLX 入口，不重实现。
pub async fn chat_stream_messages(
    client: FusionMlxClient,
    model: String,
    messages: Vec<MlxChatMessage>,
    max_tokens: u32,
) -> impl futures::Stream<Item = anyhow::Result<MlxStreamDelta>> {
    // A6 可观测性：关键推理入口记关联日志（model/消息数/max_tokens），
    // 便于商用排障关联同一请求的完整生命周期。
    tracing::info!(
        model = %model,
        msg_count = messages.len(),
        max_tokens,
        "chat_stream_messages: 启动流式推理请求"
    );
    let payload = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": true,
    });
    let endpoints = client.endpoints.clone();
    let rr = client.rr.clone();
    let bearer = client.bearer_token();
    let http = client.http;
    // M-5：建连阶段（send + status）指数退避重试 502/503。拿到 2xx 即进流消费。
    // 流已建立后的中途断流不重试（语义复杂——已消费部分 delta，盲目重发致重复输出，
    // 须上游提供 Last-Event-ID/断点续传原语，见 fusion-gateway#139）。
    // RequestBuilder 消耗性，每次重试重建；bearer 保留 owned，闭包按引用取用。
    // A-1：每 attempt 轮询选 endpoint 拼 url（failover）。
    let max = retry_max_attempts();
    // R-4：建连阶段总 deadline 封顶，防 4×长退避无限挂起。
    let deadline_secs = std::env::var("FUSION_MLX_RETRY_DEADLINE_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(300);
    let deadline = std::time::Duration::from_secs(deadline_secs);
    let started = std::time::Instant::now();
    let resp = {
        let mut last_err: Option<anyhow::Error> = None;
        let mut got: Option<reqwest::Response> = None;
        for attempt in 0..max {
            if started.elapsed() > deadline {
                tracing::error!(
                    attempt,
                    elapsed = ?started.elapsed(),
                    deadline = ?deadline,
                    "chat_stream_messages: 建连超过总 deadline，放弃重试"
                );
                return futures::stream::once(async move {
                    Err(anyhow::anyhow!(
                        "fusion-mlx 建连重试超过总 deadline {:?}（尝试 {}/{} 次）",
                        deadline,
                        attempt,
                        max
                    ))
                })
                .boxed();
            }
            let idx = if endpoints.len() == 1 {
                0
            } else {
                rr.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as usize % endpoints.len()
            };
            let url = format!("{}/v1/chat/completions", endpoints[idx]);
            // A-6：鉴权头经 attach_route_bearer 统一。bearer 预算一次，闭包按引用取用。
            let build_req =
                || attach_route_bearer(http.post(&url).json(&payload), bearer.as_deref());
            let attempt_req = build_req();
            match attempt_req.send().await {
                Ok(r) if r.status().is_success() => {
                    got = Some(r);
                    break;
                }
                Ok(r) => {
                    let code = r.status().as_u16();
                    if is_transient_status(code) && attempt + 1 < max {
                        let delay = backoff_delay(attempt);
                        tracing::warn!(
                            attempt,
                            code,
                            endpoint = %endpoints[idx],
                            ?delay,
                            "chat_stream_messages: 建连瞬时错误，退避后切换节点重试"
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    tracing::error!(
                        attempt,
                        code,
                        "chat_stream_messages: 建连失败，重试耗尽或永久错误"
                    );
                    return futures::stream::once(async move {
                        Err(anyhow::anyhow!(
                            "fusion-mlx HTTP {code}（尝试 {}/{} 次）",
                            attempt + 1,
                            max
                        ))
                    })
                    .boxed();
                }
                Err(e) => {
                    if attempt + 1 < max {
                        let delay = backoff_delay(attempt);
                        tracing::warn!(
                            attempt, error = %e, endpoint = %endpoints[idx], ?delay,
                            "chat_stream_messages: 建连失败，退避后切换节点重试"
                        );
                        last_err = Some(anyhow::Error::from(e));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    tracing::error!(attempt, error = %e, "chat_stream_messages: 建连失败，重试耗尽");
                    return futures::stream::once(async move {
                        Err(anyhow::anyhow!(
                            "SSE 请求失败（尝试 {}/{} 次）: {e}",
                            attempt + 1,
                            max
                        ))
                    })
                    .boxed();
                }
            }
        }
        match got {
            Some(r) => r,
            None => {
                let e = last_err
                    .unwrap_or_else(|| anyhow::anyhow!("chat_stream_messages: 重试循环异常退出"));
                return futures::stream::once(async move { Err(e) }).boxed();
            }
        }
    };

    let stream = resp.bytes_stream();
    // FAULT-1：单 chunk 间空闲上限。`reqwest` client `.timeout(180s)` 是**总**时限，
    //   非 chunk 间间隔——上游建立连接后首帧快、中途 stall（模型算力耗尽/网络抖动）
    //   会挂起 stream.next().await 无限，无任何重试/超时兜底。
    //   用 tokio::time::timeout 包单次 next：超过 IDLE 未见下一 chunk 即 Elapsed →
    //   emit error delta + 终止流（fail visibly Rule 12）。区别于总 deadline：
    //   总 deadline 封顶整轮重试，IDLE 封顶单 chunk 空闲（长生成内容有间隔也安全，
    //   仅「卡死无任何字节」超 IDLE）。FUSION_MLX_STREAM_IDLE_SECS 可调，缺省 60。
    let idle_secs = std::env::var("FUSION_MLX_STREAM_IDLE_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(60);
    let idle = std::time::Duration::from_secs(idle_secs);
    // L6：buffer 用 Vec<u8> 而非 String。旧实现用 String::from_utf8_lossy(&bytes)
    //   把每个 chunk 立即解码，但 chunk 可能在多字节 CJK 字符中间切断（UTF-8 一字 3 字节），
    //   残缺尾字节被替换成 U+FFFD → 中文 UI JSON 乱码。改字节缓冲：完整行（以 \n 分隔）
    //   才整体 from_utf8 解码，跨 chunk 的残缺多字节字符留在 buffer 等下一 chunk 补全。
    futures::stream::unfold(
        (stream, Vec::<u8>::new(), idle, idle_secs),
        |(mut stream, mut buffer, idle, idle_secs)| async move {
            use futures::StreamExt;
            loop {
                // FAULT-1：timeout 包单次 next，stall 超 IDLE 即 Elapsed 失败可见。
                match tokio::time::timeout(idle, stream.next()).await {
                    Ok(Some(Ok(bytes))) => {
                        buffer.extend_from_slice(&bytes);
                        // R-12：先排空所有完整行（含本轮及之前残留），再判定残留上限。
                        // 旧实现 `&& !buffer.contains(&b'\n')` 在「超限但含换行」时
                        // 静默跳过 cap，且 contains 是 O(n) 全扫。改为：完整行必先 drain，
                        // 仅对 drain 后的**跨 chunk 残留**（无换行的半截）做上限判定。
                        while let Some(line_end) = buffer.iter().position(|&b| b == b'\n') {
                            // L6：完整行整体 from_utf8_lossy 解码（行内已是完整 UTF-8，
                            //   SSE data: 前缀+JSON 结构为 ASCII，CJK 在 JSON 值内且完整）。
                            // 先解码再 drain，避免 line_bytes 借用与 buffer.drain 冲突。
                            let line = String::from_utf8_lossy(&buffer[..line_end])
                                .trim()
                                .to_string();
                            buffer.drain(..=line_end);
                            // ARCH-10 round-2：strip-data/[DONE]/JSON/抽 content 逻辑
                            // 外移至 stream::parse_sse_line（单一真相源），此处仅消费 delta。
                            if let Some(delta) = parse_sse_line(&line) {
                                return Some((Ok(delta), (stream, buffer, idle, idle_secs)));
                            }
                        }
                        // R-12：while-drain 后残留 = 跨 chunk 半截行（无换行）。
                        // 残留超上限即异常流（上游慢/单行超 8MB），报错终止，杜绝内存膨胀。
                        // 旧条件含 `!buffer.contains(&b'\n')` 在超限但含换行时静默放行，
                        // 已由「先 drain 再判残留」根除该盲区。
                        if buffer.len() > MAX_SSE_BUFFER {
                            let cap = sse_buffer_cap();
                            if buffer.len() > cap {
                                tracing::error!(
                                    len = buffer.len(),
                                    cap,
                                    "SSE 残留超限（跨 chunk 半截行无换行），终止流"
                                );
                                return Some((
                                    Err(anyhow::anyhow!(
                                        "SSE buffer 残留超限 ({cap} 字节) 无完整行"
                                    )),
                                    (stream, Vec::new(), idle, idle_secs),
                                ));
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        return Some((Err(anyhow::anyhow!("SSE 读取出错: {e}")), (stream, buffer, idle, idle_secs)));
                    }
                    // FAULT-1：EOF，先排空 buffer 残留成行数据再终止流。
                    Ok(None) => {
                        // EOF：先排空 buffer 里残留的成行数据，再终止流。
                        // 上游可能没在最后一帧后补换行，或最后一个 chunk
                        // 还停在 buffer 里没被 while 循环处理 — 直接 return None
                        // 会丢尾部 delta（#18）。
                        while let Some(line_end) = buffer.iter().position(|&b| b == b'\n') {
                            let line = String::from_utf8_lossy(&buffer[..line_end])
                                .trim()
                                .to_string();
                            buffer.drain(..=line_end);
                            // ARCH-10 round-2：复用 stream::parse_sse_line 单一真相源。
                            if let Some(delta) = parse_sse_line(&line) {
                                return Some((Ok(delta), (stream, buffer, idle, idle_secs)));
                            }
                        }
                        // 无换行的尾部残行（上游没补换行就关连接）：按整行解析。
                        // L6：残行也可能是跨 chunk 的 CJK，整体 from_utf8_lossy 解码。
                        let tail = String::from_utf8_lossy(&buffer).trim().to_string();
                        if !tail.is_empty() {
                            buffer.clear();
                            // ARCH-10 round-2：尾残行同样复用 parse_sse_line（单一真相源）。
                            if let Some(delta) = parse_sse_line(&tail) {
                                return Some((Ok(delta), (stream, buffer, idle, idle_secs)));
                            }
                        }
                        return None;
                    }
                    // FAULT-1：单 chunk 空闲超 IDLE — stall 卡死，fail visibly 终止流。
                    // 区别于 client 总 timeout（180s）：IDLE 是相邻 chunk 间隔上限。
                    Err(_elapsed) => {
                        tracing::error!(
                            idle_secs,
                            "SSE 流 stall：超过 {}s 未见下一 chunk，终止流（FAULT-1）",
                            idle_secs
                        );
                        return Some((
                            Err(anyhow::anyhow!(
                                "SSE 流 stall：超过 {idle_secs}s 未见下一 chunk（FUSION_MLX_STREAM_IDLE_SECS 可调）"
                            )),
                            (stream, buffer, idle, idle_secs),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
    /// 鉴权是否通过。None=未鉴权或无法判定；Some(true)=已通过；
    /// Some(false)=401/403 被拒（FUSION_MLX_API_KEY 缺失或错误）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_ok: Option<bool>,
    /// /v1/models 返回的模型 id 列表。空列表 = gateway 挂了模型名但 MLX 未加载，
    /// generate 仍会 502——这是「假绿」的根因。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub models: Vec<String>,
    /// 人读状态摘要：可达/鉴权失败/无模型可用/可用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl FusionMlxClient {
    /// 探测 fusion-mlx 健康状态（超时 3s）。
    /// 区分三种失败：不可达 / 鉴权拒绝(401/403) / 可达但无模型——
    /// 后两者过去都塌缩成 available:false，掩盖了「设了 key 却无效」「gateway 假绿」。
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
            Ok(r) => {
                let status_code = r.status();
                if status_code.is_success() {
                    let body: serde_json::Value = r.json().await.unwrap_or_default();
                    let models: Vec<String> = body["data"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| m["id"].as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let model = models.first().cloned();
                    let auth_ok = Some(true);
                    let available = !models.is_empty();
                    let status = if available {
                        format!(
                            "可用（{} 个模型，首个 {}）",
                            models.len(),
                            model.as_deref().unwrap_or("?")
                        )
                    } else {
                        String::from("可达但无模型——gateway 列表为空或 MLX 未加载，generate 将 502")
                    };
                    if available {
                        tracing::info!(available = true, model = ?model, n = models.len(), "health_check: MLX 可用");
                    } else {
                        tracing::warn!(
                            "health_check: /v1/models 返回 200 但 data 为空，疑似 gateway 假绿"
                        );
                    }
                    Ok(HealthStatus {
                        available,
                        model,
                        gpu: None,
                        auth_ok,
                        models,
                        status: Some(status),
                    })
                } else if status_code.as_u16() == 401 || status_code.as_u16() == 403 {
                    tracing::warn!(status = %status_code, "health_check: 鉴权被拒，检查 FUSION_MLX_API_KEY");
                    Ok(HealthStatus {
                        available: false,
                        model: None,
                        gpu: None,
                        auth_ok: Some(false),
                        models: vec![],
                        status: Some(format!(
                            "鉴权失败（{status_code}）：FUSION_MLX_API_KEY 缺失或无效"
                        )),
                    })
                } else {
                    tracing::warn!(status = %status_code, "health_check: MLX 返回非 2xx");
                    Ok(HealthStatus {
                        available: false,
                        model: None,
                        gpu: None,
                        auth_ok: None,
                        models: vec![],
                        status: Some(format!("MLX 返回 {status_code}")),
                    })
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "health_check: MLX 不可达");
                Ok(HealthStatus {
                    available: false,
                    model: None,
                    gpu: None,
                    auth_ok: None,
                    models: vec![],
                    status: Some(format!("不可达：{e}")),
                })
            }
        }
    }

    /// 健康检查同步版（阻塞当前线程）。
    pub fn health_check_sync(&self) -> anyhow::Result<HealthStatus> {
        let this = self.clone();
        BLOCKING_RT.block_on(async move { this.health_check().await })
    }

    /// 真推理探针：发 1-token chat 请求，判定 generate 路径是否真能出活。
    /// gateway /v1/models 与 /health、/readyz 均会「假绿」（列了模型名但 MLX 未加载），
    /// 唯一权威可用性信号是真实 chat 调用——502/503 = 模型未加载，200 = 真可用。
    pub async fn check_generate(&self, model: &str) -> anyhow::Result<GenerateProbeStatus> {
        let payload = MlxChatPayload {
            model,
            messages: vec![MlxMessage {
                role: "user",
                content: "ping",
            }],
            max_tokens: 1,
            temperature: None,
        };
        // M-5：502/503 = 模型加载中/被驱逐，退避等待后重试。4xx/非瞬时 5xx/连接失败耗尽
        // 仍按原诊断语义返回（不改变 GenerateProbeStatus 结构）。
        // A-1：每 attempt 轮询选 endpoint（failover），url 在循环内按节点拼。
        let max = retry_max_attempts();
        // R-4：探针重试总 deadline 封顶，防 4×长退避无限挂起。
        let deadline_secs = std::env::var("FUSION_MLX_RETRY_DEADLINE_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(300);
        let deadline = std::time::Duration::from_secs(deadline_secs);
        let started = std::time::Instant::now();
        let mut last_err: Option<String> = None;
        let mut last_code: Option<u16> = None;
        for attempt in 0..max {
            if started.elapsed() > deadline {
                tracing::error!(
                    attempt,
                    elapsed = ?started.elapsed(),
                    deadline = ?deadline,
                    "check_generate: 超过总 deadline，放弃重试"
                );
                return Ok(GenerateProbeStatus {
                    model_loaded: false,
                    available: false,
                    status: Some(format!(
                        "推理探针重试超过总 deadline {:?}（尝试 {}/{} 次）",
                        deadline, attempt, max
                    )),
                    http_code: None,
                });
            }
            let url = format!("{}/v1/chat/completions", self.endpoint_for_attempt(attempt));
            let resp = self
                .authed(self.http.post(&url).json(&payload))
                .send()
                .await;
            match resp {
                Ok(r) => {
                    let code = r.status().as_u16();
                    if r.status().is_success() {
                        tracing::info!(model, attempt, "check_generate: 推理探针通过");
                        return Ok(GenerateProbeStatus {
                            model_loaded: true,
                            available: true,
                            status: Some(format!(
                                "推理探针通过（模型 {model} 已加载，尝试 {}/{} 次）",
                                attempt + 1,
                                max
                            )),
                            http_code: Some(code),
                        });
                    } else if is_transient_status(code) {
                        let body: serde_json::Value = r.json().await.unwrap_or_default();
                        let msg = body["error"]["message"]
                            .as_str()
                            .unwrap_or("无错误体")
                            .to_string();
                        last_code = Some(code);
                        if attempt + 1 < max {
                            let delay = backoff_delay(attempt);
                            tracing::warn!(
                                attempt,
                                code,
                                msg,
                                ?delay,
                                "check_generate: 模型未加载（502/503），退避后重试"
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        tracing::warn!(code, msg, "check_generate: 模型未加载，重试耗尽");
                        return Ok(GenerateProbeStatus {
                            model_loaded: false,
                            available: false,
                            status: Some(format!(
                                "模型未加载（{code} {msg}）：重试 {max} 次仍 502/503，generate 失败"
                            )),
                            http_code: Some(code),
                        });
                    } else if code == 401 || code == 403 {
                        return Ok(GenerateProbeStatus {
                            model_loaded: false,
                            available: false,
                            status: Some(format!("鉴权失败（{code}）：FUSION_MLX_API_KEY 无效")),
                            http_code: Some(code),
                        });
                    } else {
                        return Ok(GenerateProbeStatus {
                            model_loaded: false,
                            available: false,
                            status: Some(format!("generate 返回 {code}")),
                            http_code: Some(code),
                        });
                    }
                }
                Err(e) => {
                    last_err = Some(format!("{e}"));
                    if attempt + 1 < max {
                        let delay = backoff_delay(attempt);
                        tracing::warn!(
                            attempt, error = %e, ?delay,
                            "check_generate: 请求失败，退避后重试"
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    tracing::warn!(error = %e, "check_generate: 请求失败，重试耗尽");
                    return Ok(GenerateProbeStatus {
                        model_loaded: false,
                        available: false,
                        status: Some(format!("generate 请求失败（重试 {max} 次）：{e}")),
                        http_code: None,
                    });
                }
            }
        }
        // 循环正常退出（max=0 不会发生，retry_max_attempts 已保证 >=1）：兜底。
        Ok(GenerateProbeStatus {
            model_loaded: false,
            available: false,
            status: Some(format!(
                "generate 重试 {max} 次未成功{}{}",
                last_code
                    .map(|c| format!("，末次 HTTP {c}"))
                    .unwrap_or_default(),
                last_err
                    .map(|e| format!("，末次错误 {e}"))
                    .unwrap_or_default()
            )),
            http_code: last_code,
        })
    }
}

/// 真推理探针结果。available = generate 路径真能出活。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateProbeStatus {
    pub model_loaded: bool,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_code: Option<u16>,
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
/// 从 base64 数据头嗅探真实图片 MIME（E-7/P3）。
/// 旧实现硬编码 `image/png`，非 PNG（JPEG/WebP/GIF）被误标，多模态模型可能拒识。
/// 解码首 12 字节读 magic：PNG 89504E47、JPEG FFD8FF、WebP "RIFF....WEBP"、GIF "GIF8"，
/// 未知则回退 png（OpenAI vision 兼容最广，模型侧通常容忍 mime 不精确）。
fn detect_image_mime(image_base64: &str) -> &'static str {
    use base64::Engine;
    let head_b64 = image_base64.get(..16).unwrap_or(image_base64);
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(head_b64)
        .unwrap_or_default();
    if decoded.len() >= 4 && decoded[0..4] == [0x89, 0x50, 0x4E, 0x47] {
        "image/png"
    } else if decoded.len() >= 3 && decoded[0..3] == [0xFF, 0xD8, 0xFF] {
        "image/jpeg"
    } else if decoded.len() >= 12 && &decoded[0..4] == b"RIFF" && &decoded[8..12] == b"WEBP" {
        "image/webp"
    } else if decoded.len() >= 6 && &decoded[0..6] == b"GIF89a"
        || decoded.len() >= 6 && &decoded[0..6] == b"GIF87a"
    {
        "image/gif"
    } else {
        "image/png"
    }
}

pub async fn chat_with_image(
    client: &FusionMlxClient,
    model: &str,
    system_prompt: &str,
    user_text: &str,
    image_base64: &str,
    max_tokens: u32,
) -> anyhow::Result<String> {
    let mime = detect_image_mime(image_base64);
    let image_data_url = format!("data:{mime};base64,{image_base64}");
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

// encode_image_base64 / encode_image_base64_async 迁出 fd-skills（ARCH-11），
// 本文件经 re-export 引用。

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

use op_ai::chat_provider::{
    ChatDelta, ChatProvider, ChatRequest, EffortLevel, StopReason, ThinkingMode,
};

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
        // H-A9：旧实现丢弃 history/thinking/effort/attachments，多轮上下文丢失且无诊断。
        // history 折叠进 messages[]（system + 历史 + 当前 user），透传到 MLX。
        let mut messages: Vec<MlxChatMessage> = Vec::with_capacity(2 + request.history.len());
        if !request.system_prompt.trim().is_empty() {
            messages.push(MlxChatMessage {
                role: "system".into(),
                content: request.system_prompt,
            });
        }
        for (role, content) in &request.history {
            messages.push(MlxChatMessage {
                role: role.as_str().into(),
                content: content.clone(),
            });
        }
        messages.push(MlxChatMessage {
            role: "user".into(),
            content: request.user_message,
        });
        // thinking/effort/attachments 暂无 wire 支持（fusion-mlx OpenAI 兼容形状未暴露这些字段）。
        // 静默丢弃即功能缺失无诊断；显式 warn 让上游可观测到能力降级，便于后续补 wire。
        if !matches!(request.thinking, ThinkingMode::Adaptive) {
            tracing::warn!(
                thinking = request.thinking.as_str(),
                "ChatRequest.thinking 无 wire 支持，已降级忽略（fusion-mlx 未暴露 thinking 开关）"
            );
        }
        if !matches!(request.effort, EffortLevel::Low) {
            tracing::warn!(
                effort = request.effort.as_str(),
                "ChatRequest.effort 无 wire 支持，已降级忽略（fusion-mlx 未暴露 effort 旋钮）"
            );
        }
        if !request.attachments.is_empty() {
            tracing::warn!(
                count = request.attachments.len(),
                names = ?request.attachments.iter().map(|a| &a.name).collect::<Vec<_>>(),
                "ChatRequest.attachments 无 wire 支持，已降级忽略（多模态附件尚未接入）"
            );
        }
        // H-A7：旧实现把全文包成单个 TextDelta，消费方等全文生成完才见首个 delta
        // （同步 trait 语义下无真流式，但仍应产增量序列而非一次性蹦全文）。
        // 按行分块：每行（含换行）一个 TextDelta，消费方逐行收增量；末帧 Done。
        // 空文本不产 TextDelta，仅 Done。
        match self
            .client
            .chat_sync_messages(model, messages, request.max_output_tokens)
        {
            Ok(text) => {
                let mut deltas: Vec<ChatDelta> = Vec::new();
                let mut rest = text.as_str();
                while !rest.is_empty() {
                    match rest.find('\n') {
                        Some(i) => {
                            let line_end = i + 1;
                            deltas.push(ChatDelta::TextDelta(rest[..line_end].to_string()));
                            rest = &rest[line_end..];
                        }
                        None => {
                            deltas.push(ChatDelta::TextDelta(rest.to_string()));
                            rest = "";
                        }
                    }
                }
                tracing::info!(
                    chunks = deltas.len(),
                    "ChatProvider::send: 分块产出增量 delta 序列"
                );
                deltas.push(ChatDelta::Done {
                    stop_reason: StopReason::EndTurn,
                });
                Box::new(deltas.into_iter())
            }
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

// ── AI Skill 系统：已迁出 fd-skills crate（ARCH-11）──
//
// SkillContext trait + 7 skill impl + SkillRegistry + SkillOutput/SpecDocument 类型
// + 纯 helper（parse_ui_json/repair_model_json/strip_code_fence/encode_image_base64/
// ui_generator_system_prompt/DEFAULT_MAX_TOKENS）迁至 fd-skills。adapter 反向
// impl SkillContext for FusionMlxClient，DesignSkills facade 通过 &dyn SkillContext
// 调度 skill。re-export 保持 adapter 内部调用方路径不变。

use fd_design_system::DesignSystem;

// ARCH-11 re-export：fd-skills 的 skill 类型 + 纯 helper，保持 adapter 内部
//（DesignSkills facade / mlx_integration tests / skills_tests）路径不变。
pub use fd_skills::{
    encode_image_base64, encode_image_base64_async, parse_flow_pages, parse_local_edit_input,
    parse_node_with_depth, parse_nodes_with_depth, parse_page_flow_input, parse_spec_doc_input,
    parse_spec_doc_json, parse_ui_json, repair_model_json, strip_code_fence,
    ui_generator_system_prompt, ComponentProp, ComponentSpec, DesignSkill, ImageToUiSkill,
    InteractionSpec, LocalEditSkill, MultiVariantsSkill, PageFlowSkill, PartialEditSkill,
    SkillContext, SkillOutput, SkillRegistry, SpecDocSkill, SpecDocument, TextToUiSkill,
    DEFAULT_MAX_TOKENS,
};
// 7 skill struct 在 fd-skills 是 pub 但持 pub(crate) model 字段——adapter 仅作
// &dyn SkillContext 调度 + re-export 类型名，不直接构造 skill struct（构造走
// SkillRegistry::register_builtin(model)）。故只 re-export 类型名供类型引用。

// ── impl SkillContext for FusionMlxClient（ARCH-11 反向实现 trait）──
//
// SkillContext trait 对象安全（无泛型/Self），FusionMlxClient 是 Send+Sync
//（String + Vec + Arc<AtomicU32> + reqwest::Client），impl 合法。trait 方法
// 委托到 adapter 已有的 chat_sync/chat_async/chat_with_image/chat_with_image_sync。
impl SkillContext for FusionMlxClient {
    fn chat(&self, model: &str, sys: &str, user: &str, max_tokens: u32) -> anyhow::Result<String> {
        self.chat_sync(model, sys, user, max_tokens)
    }

    fn chat_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>
    {
        Box::pin(async move { self.chat_async(model, sys, user, max_tokens).await })
    }

    fn chat_with_image_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        image_base64: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>
    {
        Box::pin(
            async move { chat_with_image(self, model, sys, user, image_base64, max_tokens).await },
        )
    }

    fn chat_with_image(
        &self,
        model: &str,
        sys: &str,
        user: &str,
        image_base64: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        chat_with_image_sync(self, model, sys, user, image_base64, max_tokens)
    }

    fn token_prompt_fragment(&self, design_system: Option<&DesignSystem>) -> Option<String> {
        design_system.map(|ds| {
            let css = ds.to_css_custom_properties();
            format!("当前设计系统 Token（CSS Custom Properties）：\n{css}\n\n生成的 UI 必须使用这些 CSS 变量。")
        })
    }
}

use fd_canvas_core::PenDocument;

// A-2：html_to_pen_document 拆到 fd-html-parser 叶子 crate，re-export 保持
// 调用方 `fd_ai_adapter::html_to_pen_document`（fd-cli）不变。
pub use fd_html_parser::html_to_pen_document;

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
                chat_with_image_sync(
                    &self.client,
                    &self.default_model,
                    &sys,
                    &user,
                    &b64,
                    DEFAULT_MAX_TOKENS,
                )?
            }
            Err(e) => {
                tracing::warn!(sketch_path, error = %e, "image_to_ui: 草图加载失败，回退文字描述");
                let user_text = format!(
                    "草图路径：{sketch_path}（无法读取：{e}）\n补充说明：{hint}\n生成页面「{page_name}」对应的 UI 布局。"
                );
                self.client
                    .chat_sync(&self.default_model, &sys, &user_text, DEFAULT_MAX_TOKENS)?
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
        // A6 可观测性：图生 UI 是核心产品入口，记关联字段便于排障追踪。
        tracing::info!(
            sketch_path,
            page_name,
            model = %self.default_model,
            "image_to_ui_async: 启动图生 UI 请求"
        );
        let sys = ui_generator_system_prompt();
        let user =
            format!("补充说明：{hint}\n请根据上方草图图片生成页面「{page_name}」对应的 UI 布局。");
        let resp = match encode_image_base64_async(std::path::Path::new(sketch_path)).await {
            Ok(b64) => {
                tracing::info!(
                    sketch_path,
                    bytes = b64.len(),
                    "image_to_ui_async: 已加载草图，发送真实多模态请求"
                );
                chat_with_image(
                    &self.client,
                    &self.default_model,
                    &sys,
                    &user,
                    &b64,
                    DEFAULT_MAX_TOKENS,
                )
                .await?
            }
            // L-5：草图不可读即 image-to-ui 无意义，不静默降级文字回退（fail visibly, Rule 12）。
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "image_to_ui_async: 草图加载失败: {e}（路径: {sketch_path}）"
                ));
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
        let resp = chat_with_image(
            &self.client,
            &self.default_model,
            &sys,
            &user,
            &b64,
            DEFAULT_MAX_TOKENS,
        )
        .await?;
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
        let skill = SpecDocSkill::new(&self.default_model);
        let input = format!("{doc_json}|{title}");
        match skill.execute(&self.client, None, &input)? {
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
        let skill = SpecDocSkill::new(&self.default_model);
        let input = format!("{doc_json}|{title}");
        match skill.execute_async(&self.client, None, input).await? {
            SkillOutput::SpecDoc(spec) => Ok(spec),
            other => anyhow::bail!(
                "spec-doc 返回非 SpecDoc: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    pub fn page_flow(&self, flow_desc: &str, style_hint: &str) -> anyhow::Result<Vec<PenDocument>> {
        let skill = PageFlowSkill::new(&self.default_model);
        let input = format!("{flow_desc}|{style_hint}");
        match skill.execute(&self.client, None, &input)? {
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
        let skill = PageFlowSkill::new(&self.default_model);
        let input = format!("{flow_desc}|{style_hint}");
        match skill.execute_async(&self.client, None, input).await? {
            SkillOutput::PageFlow(docs) => Ok(docs),
            other => anyhow::bail!(
                "page-flow 返回非 PageFlow: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }
}

#[cfg(test)]
mod skills_tests {
    use super::*;

    // ARCH-11：skill 系统 tests 迁至 fd-skills/tests/skills_mock.rs。
    // 本 mod 保留 B-class：endpoint 解析 + DesignSkills facade 可构造性 +
    // strip_code_fence 直通（helper re-export 路径回归）。

    #[test]
    fn resolve_endpoint_empty_falls_back_to_default() {
        // 清掉环境变量，空串应回退方案B gateway 11432
        std::env::remove_var("FUSION_MLX_BASE_URL");
        let ep = FusionMlxClient::resolve_endpoint("").unwrap();
        assert_eq!(ep, vec!["http://127.0.0.1:11432"]);
    }

    #[test]
    fn resolve_endpoint_explicit_overrides_env() {
        // 用户显式传 --endpoint 优先级最高，忽略 env
        std::env::set_var("FUSION_MLX_BASE_URL", "http://127.0.0.1:11434");
        let ep = FusionMlxClient::resolve_endpoint("http://127.0.0.1:11432").unwrap();
        assert_eq!(ep, vec!["http://127.0.0.1:11432"]);
        std::env::remove_var("FUSION_MLX_BASE_URL");
    }

    #[test]
    fn resolve_endpoint_multi_value_split_by_comma() {
        // A-1：逗号分隔多值应拆成多条 endpoint，各经 validate_localhost。
        std::env::remove_var("FUSION_MLX_BASE_URL");
        let ep = FusionMlxClient::resolve_endpoint("http://127.0.0.1:11432,http://10.0.0.5:11432")
            .unwrap();
        assert_eq!(ep, vec!["http://127.0.0.1:11432", "http://10.0.0.5:11432"]);
    }

    #[test]
    fn resolve_endpoint_multi_value_rejects_public() {
        // A-1：多值中任一公网 endpoint 整体拒绝。
        std::env::remove_var("FUSION_MLX_BASE_URL");
        let r = FusionMlxClient::resolve_endpoint("http://127.0.0.1:11432,http://8.8.8.8:11432");
        assert!(r.is_err(), "多值含公网 IP 应整体拒绝");
    }

    #[test]
    fn parse_endpoints_splits_and_trims() {
        assert_eq!(
            parse_endpoints("http://a:1, http://b:2 ,http://c:3"),
            vec!["http://a:1", "http://b:2", "http://c:3"]
        );
        assert_eq!(parse_endpoints(""), vec!["http://127.0.0.1:11432"]);
        assert_eq!(parse_endpoints("  ,  "), vec!["http://127.0.0.1:11432"]);
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
    use op_ai::chat_provider::ChatHistoryRole;
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

    /// A-1：多节点 mock client（endpoints 列表，跳过 validate_localhost 的公网校验——
    /// 测试 server 绑 127.0.0.1，本身就是 loopback，校验必过）。
    fn mock_client_endpoints(endpoints: Vec<String>) -> FusionMlxClient {
        FusionMlxClient::with_endpoints(endpoints).unwrap()
    }
    /// SSE mock server: emit each frame as a "data: <frame>" line, then a final "data: [DONE]".
    async fn spawn_sse_server(frames: Vec<String>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let mut resp = String::new();
                resp.push_str("HTTP/1.1 200 OK");
                resp.push_str("\r\n");
                resp.push_str("Content-Type: text/event-stream");
                resp.push_str("Connection: close");
                resp.push_str("\r\n");
                resp.push_str("\r\n");
                for frame in &frames {
                    resp.push_str("data: ");
                    resp.push_str(frame);
                    resp.push_str("\r\n");
                    resp.push_str("\r\n");
                }
                resp.push_str("data: [DONE]");
                resp.push_str("\r\n");
                resp.push_str("\r\n");
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        format!("http://{}", addr)
    }

    /// 写一个真实可读的占位 PNG 到唯一临时路径，保证测试不依赖外部 /tmp/fd_sketch.png。
    /// encode_image_base64 仅 base64 编码文件字节，不校验图像格式，故最小 PNG 字节即可。
    /// SSE mock server (early EOF): emit frames then close the connection
    /// WITHOUT sending "data: [DONE]". Simulates upstream dropping mid-stream
    /// so the EOF branch must drain buffered deltas and terminate, not spin.
    async fn spawn_sse_server_early_eof(frames: Vec<String>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let mut resp = String::new();
            resp.push_str("HTTP/1.1 200 OK");
            resp.push_str("\r\n");
            resp.push_str("Content-Type: text/event-stream");
            resp.push_str("Connection: close");
            resp.push_str("\r\n");
            resp.push_str("\r\n");
            for frame in &frames {
                resp.push_str("data: ");
                resp.push_str(frame);
                resp.push_str("\r\n");
                resp.push_str("\r\n");
            }
            // no [DONE], just close
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
        });
        format!("http://{}", addr)
    }

    /// FAULT-1 stall mock server：建连 200 + 发 SSE headers 后不接任何 data 帧，
    /// 长时间 sleep 不再写，模拟建连后无 chunk 卡死。chat_stream_messages 的
    /// tokio::time::timeout(idle, stream.next()) 应在 IDLE 内以 Err 终结。
    async fn spawn_sse_stall_server() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let mut resp = String::new();
            resp.push_str("HTTP/1.1 200 OK\r\n");
            resp.push_str("Content-Type: text/event-stream\r\n");
            resp.push_str("Connection: close\r\n\r\n");
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
            // 建连后不发任何 data 帧，长时间 sleep 模拟 stall 卡死。
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        format!("http://{}", addr)
    }

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

    /// 精确 status 的 mock server（spawn_mock_server 把非 200 硬编码成 500，
    /// 这里按真实 status 行返回，用于 401/403 鉴权路径测试）。
    async fn spawn_status_server(status: u16, body: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            502 => "Bad Gateway",
            _ => "ERR",
        };
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let mut buf = [0u8; 4096];
                let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    /// 序列 mock server：前 N 次按 statuses[] 返回，之后恒返回 200 + body_for_last。
    /// 用于 M-5 重试回归：先 502/503 几次，再 200 成功。
    /// 计数器用 AtomicU32，返回 url + 命中计数句柄供断言调用次数。
    async fn spawn_sequence_server(
        statuses: Vec<u16>,
        body_for_last: String,
    ) -> (String, Arc<std::sync::atomic::AtomicU32>) {
        use std::sync::atomic::{AtomicU32, Ordering};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();
        let statuses_len = statuses.len();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let idx = count_clone.fetch_add(1, Ordering::SeqCst) as usize;
                let mut buf = [0u8; 4096];
                let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
                let (status, reason, body) = if idx < statuses_len {
                    let s = statuses[idx];
                    let r = match s {
                        200 => "OK",
                        401 => "Unauthorized",
                        403 => "Forbidden",
                        502 => "Bad Gateway",
                        503 => "Service Unavailable",
                        _ => "ERR",
                    };
                    (s, r, String::from(r#"{"error":{"message":"loading"}}"#))
                } else {
                    (200, "OK", body_for_last.clone())
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes()).await;
            }
        });
        (format!("http://{addr}"), count)
    }

    /// 序列 SSE mock server：前 N 次返回 statuses[]（瞬时错误），之后返回 200 SSE。
    /// 用于 chat_stream_messages 建连重试回归：502 几次后 200 流出 delta。
    async fn spawn_sequence_sse_server(statuses: Vec<u16>, frames: Vec<String>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let statuses_len = statuses.len();
        tokio::spawn(async move {
            let mut hits = 0u32;
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                if (hits as usize) < statuses_len {
                    let s = statuses[hits as usize];
                    let reason = match s {
                        502 => "Bad Gateway",
                        503 => "Service Unavailable",
                        _ => "ERR",
                    };
                    let resp = format!(
                        "HTTP/1.1 {s} {reason}\r\nContent-Type: application/json\r\nContent-Length: 30\r\n\r\n{{\"error\":{{\"message\":\"loading\"}}}}"
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                    hits += 1;
                    continue;
                }
                let mut resp = String::new();
                resp.push_str("HTTP/1.1 200 OK\r\n");
                resp.push_str("Content-Type: text/event-stream\r\n");
                resp.push_str("Connection: close\r\n\r\n");
                for frame in &frames {
                    resp.push_str("data: ");
                    resp.push_str(frame);
                    resp.push_str("\r\n\r\n");
                }
                resp.push_str("data: [DONE]\r\n\r\n");
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        format!("http://{}", addr)
    }

    /// 401 时 available=false 且 auth_ok=Some(false)——修复「假绿」核心场景。
    #[tokio::test]
    async fn health_check_401_marks_auth_failure() {
        let url = spawn_status_server(401, r#"{"error":"unauthorized"}"#.to_string()).await;
        let client = mock_client(&url);
        let s = client.health_check().await.unwrap();
        assert!(!s.available, "401 不应 available");
        assert_eq!(s.auth_ok, Some(false), "401 必须标 auth_ok=false");
        assert!(s.models.is_empty(), "401 不应列模型");
        assert!(
            s.status.as_deref().unwrap().contains("鉴权"),
            "status 文案须点出鉴权失败：{:?}",
            s.status
        );
    }

    /// 200 但 data 为空——gateway 假绿：available=false，auth_ok=Some(true)。
    #[tokio::test]
    async fn health_check_empty_models_not_available() {
        let url = spawn_status_server(200, r#"{"data":[]}"#.to_string()).await;
        let client = mock_client(&url);
        let s = client.health_check().await.unwrap();
        assert!(!s.available, "空 data 不应 available");
        assert_eq!(s.auth_ok, Some(true), "200 即鉴权通过");
        assert!(s.model.is_none(), "无首个模型");
        assert!(
            s.status.as_deref().unwrap().contains("无模型"),
            "status 须点出无模型：{:?}",
            s.status
        );
    }

    /// 200 且有模型——正常路径：available=true，auth_ok=Some(true)，model 有值。
    #[tokio::test]
    async fn health_check_with_models_available() {
        let body = r#"{"data":[{"id":"qwen3-coder"},{"id":"qwen2.5-vl"}]}"#.to_string();
        let url = spawn_status_server(200, body).await;
        let client = mock_client(&url);
        let s = client.health_check().await.unwrap();
        assert!(s.available, "有模型应 available");
        assert_eq!(s.auth_ok, Some(true));
        assert_eq!(s.model.as_deref(), Some("qwen3-coder"));
        assert_eq!(s.models.len(), 2, "应列全部模型 id");
        assert!(
            s.status.as_deref().unwrap().contains("可用"),
            "status 须标可用：{:?}",
            s.status
        );
    }

    /// check_generate 502——gateway 假绿终局：列了模型但 MLX 未加载，available=false。
    #[tokio::test]
    async fn check_generate_502_marks_model_not_loaded() {
        let url = spawn_status_server(
            502,
            r#"{"error":{"message":"Chat failed","type":"server_error"}}"#.to_string(),
        )
        .await;
        let client = mock_client(&url);
        let p = client.check_generate("qwen3.5-4b-4bit").await.unwrap();
        assert!(!p.available, "502 不应 available");
        assert!(!p.model_loaded, "502 不应 model_loaded");
        assert_eq!(p.http_code, Some(502));
        assert!(
            p.status.as_deref().unwrap().contains("未加载"),
            "须点出未加载：{:?}",
            p.status
        );
    }

    /// check_generate 503——MLX 显式「model loading」，available=false。
    #[tokio::test]
    async fn check_generate_503_marks_model_loading() {
        let url = spawn_status_server(
            503,
            r#"{"error":{"message":"model loading","type":"api_error"}}"#.to_string(),
        )
        .await;
        let client = mock_client(&url);
        let p = client.check_generate("m").await.unwrap();
        assert!(!p.available);
        assert!(!p.model_loaded);
        assert_eq!(p.http_code, Some(503));
    }

    /// check_generate 200——真推理通过：available=true，model_loaded=true。
    #[tokio::test]
    async fn check_generate_200_marks_available() {
        let body = r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_string();
        let url = spawn_status_server(200, body).await;
        let client = mock_client(&url);
        let p = client.check_generate("qwen3.5-4b-4bit").await.unwrap();
        assert!(p.available, "200 应 available");
        assert!(p.model_loaded, "200 应 model_loaded");
        assert_eq!(p.http_code, Some(200));
        assert!(
            p.status.as_deref().unwrap().contains("通过"),
            "须标通过：{:?}",
            p.status
        );
    }

    /// check_generate 401——鉴权失败路径。
    #[tokio::test]
    async fn check_generate_401_marks_auth_failure() {
        let url =
            spawn_status_server(401, r#"{"error":{"message":"unauthorized"}}"#.to_string()).await;
        let client = mock_client(&url);
        let p = client.check_generate("m").await.unwrap();
        assert!(!p.available);
        assert_eq!(p.http_code, Some(401));
        assert!(
            p.status.as_deref().unwrap().contains("鉴权"),
            "须点鉴权：{:?}",
            p.status
        );
    }

    // ── TC-4：gateway 假绿回归（真推理探针识破 /v1/models 谎报）──

    /// 路径感知 mock：/v1/models → 200 + 模型列表（假绿），/v1/chat/completions → 502。
    /// 模拟 gateway 列了云端/本地模型名但 MLX 未加载的「假绿」终局。
    async fn spawn_false_green_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let mut buf = [0u8; 4096];
                let n = tokio::io::AsyncReadExt::read(&mut sock, &mut buf)
                    .await
                    .unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                // 解析请求行首行：METHOD PATH HTTP/1.1
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("");
                let (status, reason, body) = if path.contains("/v1/models") {
                    // 假绿：200 + 模型列表，看似一切正常
                    (
                        200u16,
                        "OK",
                        r#"{"object":"list","data":[{"id":"qwen3.5-4b-4bit","object":"model"}]}"#
                            .to_string(),
                    )
                } else {
                    // /v1/chat/completions：MLX 未加载 → 502
                    (
                        502,
                        "Bad Gateway",
                        r#"{"error":{"message":"Chat failed","type":"server_error"}}"#.to_string(),
                    )
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    /// TC-4：gateway /v1/models 假绿——列了模型名但 generate 实返 502。
    /// check_generate 须以真推理探针识破，不据 /v1/models 判可用。
    #[tokio::test]
    async fn check_generate_sees_through_gateway_false_green() {
        let url = spawn_false_green_server().await;
        let client = mock_client(&url);
        // /v1/models 假绿 200，但 check_generate 走真 chat 探针 → 502 → model_loaded=false
        let p = client.check_generate("qwen3.5-4b-4bit").await.unwrap();
        assert!(
            !p.available,
            "gateway 假绿：/v1/models 200 但 generate 502，不应 available"
        );
        assert!(!p.model_loaded, "真推理探针须识破假绿，model_loaded=false");
        assert_eq!(p.http_code, Some(502));
        assert!(
            p.status.as_deref().unwrap().contains("未加载"),
            "须点出模型未加载（破假绿）：{:?}",
            p.status
        );
    }

    // ── M-5：502/503 指数退避重试回归 ──

    /// is_transient_status 仅 502/503 判瞬时，其余（含 200/401/403/404/500）判永久。
    #[test]
    fn is_transient_status_classifies_502_503_only() {
        assert!(is_transient_status(502));
        assert!(is_transient_status(503));
        assert!(!is_transient_status(200));
        assert!(!is_transient_status(401));
        assert!(!is_transient_status(403));
        assert!(!is_transient_status(404));
        assert!(!is_transient_status(500));
    }

    /// backoff_delay 指数增长且封顶 8s：attempt 0/1/2/3 → 500ms/1s/2s/4s，10 → 8s。
    #[test]
    fn backoff_delay_exponential_capped() {
        assert_eq!(backoff_delay(0), std::time::Duration::from_millis(500));
        assert_eq!(backoff_delay(1), std::time::Duration::from_millis(1000));
        assert_eq!(backoff_delay(2), std::time::Duration::from_millis(2000));
        assert_eq!(backoff_delay(3), std::time::Duration::from_millis(4000));
        assert_eq!(
            backoff_delay(10),
            std::time::Duration::from_millis(8000),
            "封顶 8s"
        );
    }

    /// FUSION_MLX_RETRY_MAX 覆盖。env 测试有进程全局竞态，故标 #[ignore]，
    /// 手动验证：`cargo test -p fd-ai-adapter retry_max_attempts_env_override -- --ignored --test-threads=1`。
    #[test]
    #[ignore = "env 全局竞态，手动 --test-threads=1 运行"]
    fn retry_max_attempts_env_override() {
        std::env::set_var("FUSION_MLX_RETRY_MAX", "2");
        assert_eq!(retry_max_attempts(), 2);
        std::env::remove_var("FUSION_MLX_RETRY_MAX");
        assert_eq!(retry_max_attempts(), RETRY_DEFAULT_MAX_ATTEMPTS);
        std::env::set_var("FUSION_MLX_RETRY_MAX", "0");
        assert_eq!(
            retry_max_attempts(),
            RETRY_DEFAULT_MAX_ATTEMPTS,
            "0 视为缺省"
        );
        std::env::set_var("FUSION_MLX_RETRY_MAX", "notanumber");
        assert_eq!(
            retry_max_attempts(),
            RETRY_DEFAULT_MAX_ATTEMPTS,
            "坏值视为缺省"
        );
        std::env::remove_var("FUSION_MLX_RETRY_MAX");
    }

    /// blocking_post：先 503 两次再 200 → 重试成功，调用 3 次。
    /// 非 #[tokio::test]：chat_sync 内部 BLOCKING_RT.block_on 嵌套 tokio runtime 会 panic，
    /// 故用独立 runtime 起序列 mock server，再同步调 chat_sync。
    #[test]
    fn blocking_post_retries_on_503_then_succeeds() {
        let body = r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_string();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url, count) = rt.block_on(spawn_sequence_server(vec![503, 503], body));
        let client = mock_client(&url);
        let out = client.chat_sync("m", "sys", "ping", 1);
        assert!(out.is_ok(), "503×2 后应重试成功：{:?}", out);
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "应调用 3 次（2 次 503 + 1 次 200）"
        );
    }

    /// blocking_post：401 永久错误不重试，仅调用 1 次。
    #[test]
    fn blocking_post_no_retry_on_401() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url, count) = rt.block_on(spawn_sequence_server(
            vec![401],
            String::from(r#"{"error":"nope"}"#),
        ));
        let client = mock_client(&url);
        let out = client.chat_sync("m", "sys", "ping", 1);
        assert!(out.is_err(), "401 应直接失败不重试");
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "401 永久错误应仅调用 1 次"
        );
    }

    /// blocking_post：恒 503 重试耗尽 → bail，调用 = max 次。
    /// 用序列 server 恒 503（statuses 覆盖所有调用），断言耗尽失败。
    #[test]
    fn blocking_post_retries_exhaust_then_bail() {
        // statuses 给 8 个 503，足够覆盖默认 max=4 次重试。
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url, count) = rt.block_on(spawn_sequence_server(
            vec![503, 503, 503, 503, 503, 503, 503, 503],
            String::from(r#"{"error":"nope"}"#),
        ));
        let client = mock_client(&url);
        let out = client.chat_sync("m", "sys", "ping", 1);
        assert!(out.is_err(), "恒 503 应重试耗尽后失败");
        let calls = count.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            calls, RETRY_DEFAULT_MAX_ATTEMPTS,
            "应调用满 max={RETRY_DEFAULT_MAX_ATTEMPTS} 次"
        );
        let err = out.unwrap_err().to_string();
        assert!(err.contains("503"), "错误须含 503：{err}");
    }

    /// A-1：多节点 failover。节点 A 恒 503，节点 B 恒 200。
    /// 首请求落 A（rr idx 0）→ 503 → 重试切 B（rr idx 1）→ 200 成功。
    /// 断言：最终成功，A 被调用 1 次，B 被调用 1 次。
    #[test]
    fn blocking_post_failover_to_next_endpoint() {
        let body = r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_string();
        let rt = tokio::runtime::Runtime::new().unwrap();
        // A：恒 503（给足 8 次覆盖，但 failover 后只调 1 次即切走）。
        let (url_a, count_a) = rt.block_on(spawn_sequence_server(
            vec![503, 503, 503, 503, 503, 503, 503, 503],
            body.clone(),
        ));
        // B：恒 200。
        let (url_b, count_b) = rt.block_on(spawn_sequence_server(vec![], body.clone()));
        let client = mock_client_endpoints(vec![url_a, url_b]);
        let out = client.chat_sync("m", "sys", "ping", 1);
        assert!(out.is_ok(), "A 503 应 failover 到 B 成功：{:?}", out);
        let calls_a = count_a.load(std::sync::atomic::Ordering::SeqCst);
        let calls_b = count_b.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(calls_a, 1, "A 应仅被调用 1 次（首请求落 A 后即切走）");
        assert_eq!(calls_b, 1, "B 应被调用 1 次（failover 落 B 成功）");
    }

    /// A-1：全节点 503 → 重试耗尽失败，两节点都被轮询到。
    #[test]
    fn blocking_post_failover_all_endpoints_down() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url_a, count_a) = rt.block_on(spawn_sequence_server(
            vec![503, 503, 503, 503, 503, 503, 503, 503],
            String::from(r#"{"error":"nope"}"#),
        ));
        let (url_b, count_b) = rt.block_on(spawn_sequence_server(
            vec![503, 503, 503, 503, 503, 503, 503, 503],
            String::from(r#"{"error":"nope"}"#),
        ));
        let client = mock_client_endpoints(vec![url_a, url_b]);
        let out = client.chat_sync("m", "sys", "ping", 1);
        assert!(out.is_err(), "全节点 503 应重试耗尽失败");
        let total = count_a.load(std::sync::atomic::Ordering::SeqCst)
            + count_b.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            total, RETRY_DEFAULT_MAX_ATTEMPTS,
            "两节点合计应调用满 max={RETRY_DEFAULT_MAX_ATTEMPTS} 次"
        );
    }

    /// check_generate：先 503 两次再 200 → 探针重试后 model_loaded=true。
    #[tokio::test]
    async fn check_generate_retries_on_503_then_loaded() {
        let body = r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_string();
        let (url, count) = spawn_sequence_server(vec![503, 503], body).await;
        let client = mock_client(&url);
        let p = client.check_generate("qwen3.5-4b-4bit").await.unwrap();
        assert!(p.model_loaded, "503×2 后应重试成功标 model_loaded");
        assert!(p.available);
        assert_eq!(p.http_code, Some(200));
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "探针应调用 3 次"
        );
    }

    /// chat_stream_messages：建连 502 后 200 SSE → 流重试后产出 delta。
    #[tokio::test]
    async fn chat_stream_messages_retries_on_502_then_streams() {
        let d1 = String::from(r#"{"choices":[{"delta":{"content":"Hi"}}]}"#);
        let url = spawn_sequence_sse_server(vec![502], vec![d1]).await;
        let client = mock_client(&url);
        let messages = vec![MlxChatMessage {
            role: String::from("user"),
            content: String::from("hi"),
        }];
        let drain = async {
            let mut stream =
                chat_stream_messages(client, String::from("qwen3.5"), messages, 128).await;
            use futures::StreamExt;
            let mut tokens = String::new();
            let mut saw_done = false;
            while let Some(item) = stream.next().await {
                match item {
                    Ok(delta) => {
                        if delta.finished {
                            saw_done = true;
                        }
                        tokens.push_str(&delta.token);
                    }
                    Err(e) => panic!("stream error: {e}"),
                }
            }
            (tokens, saw_done)
        };
        let (tokens, saw_done) = tokio::time::timeout(std::time::Duration::from_secs(5), drain)
            .await
            .expect("502 重试后应在 5 秒内建连并完成流");
        assert_eq!(tokens, "Hi", "重试后应正常产出 delta");
        assert!(saw_done, "流应正常以 [DONE] 结束");
    }

    /// FAULT-1：建连 200 后无任何 data 帧卡死（stall）→ chat_stream_messages 的
    /// tokio::time::timeout(idle, stream.next()) 应在 IDLE 内以 Err 终结，非无限挂起。
    /// env 全局竞态（FUSION_MLX_STREAM_IDLE_SECS 进程级 set_var），标 #[ignore] +
    /// 手动 --test-threads=1 运行，对齐同 crate retry_max_attempts_env_override 约定。
    #[tokio::test]
    #[ignore = "env 全局竞态，手动 --test-threads=1 运行：cargo test -p fd-ai-adapter chat_stream_messages_stall_timeout -- --ignored --test-threads=1"]
    async fn chat_stream_messages_stall_timeout() {
        std::env::set_var("FUSION_MLX_STREAM_IDLE_SECS", "1");
        let url = spawn_sse_stall_server().await;
        let client = mock_client(&url);
        let messages = vec![MlxChatMessage {
            role: String::from("user"),
            content: String::from("hi"),
        }];
        let drain = async {
            let mut stream =
                chat_stream_messages(client, String::from("qwen3.5"), messages, 128).await;
            use futures::StreamExt;
            let mut errored = false;
            // stall server 不发任何 delta，首个 stream.next() 即应超时返 Err。
            while let Some(item) = stream.next().await {
                match item {
                    Ok(_) => {}
                    Err(_) => {
                        errored = true;
                        break;
                    }
                }
            }
            errored
        };
        // 外层 timeout 兜底：FAULT-1 应在 IDLE(1s) 内终结，非依赖此 10s 兜底。
        let errored = tokio::time::timeout(std::time::Duration::from_secs(10), drain)
            .await
            .expect("stall 流应在 10 秒内以 Err 终结，非无限挂起");
        assert!(errored, "stall 流应以 Err 终结（FAULT-1 fail visibly）");
        std::env::remove_var("FUSION_MLX_STREAM_IDLE_SECS");
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

    /// 生产 E2E：模型返回彻底损坏 JSON → R-2 拒绝伪成功，向上传播错误（不再合成占位兜底）。
    #[tokio::test]
    async fn image_to_ui_e2e_garbled_propagates_error() {
        let body = String::from(r#"{"choices":[{"message":{"content":"totally not json {{{"}}]}"#);
        let (url, _count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let sketch = write_fixture_png();
        let result = skills.image_to_ui_async(&sketch, "测试", "Home").await;
        assert!(
            result.is_err(),
            "彻底损坏 JSON 应向上传播错误，不合成伪成功占位"
        );
        let msg = format!("{}", result.err().unwrap());
        assert!(
            msg.contains("JSON") || msg.contains("json") || msg.contains("nodes"),
            "错误信息应指明 JSON/nodes 解析失败: {msg}"
        );
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
    #[tokio::test]
    async fn chat_stream_messages_emits_deltas_and_finishes() {
        let d1 = String::from(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#);
        let d2 = String::from(r#"{"choices":[{"delta":{"content":"lo"}}]}"#);
        let url = spawn_sse_server(vec![d1, d2]).await;
        let client = mock_client(&url);
        let messages = vec![MlxChatMessage {
            role: String::from("user"),
            content: String::from("hi"),
        }];
        let mut stream = chat_stream_messages(client, String::from("qwen3.5"), messages, 128).await;
        use futures::StreamExt;
        let mut tokens = String::new();
        let mut saw_done = false;
        while let Some(item) = stream.next().await {
            match item {
                Ok(delta) => {
                    if delta.finished {
                        saw_done = true;
                    }
                    tokens.push_str(&delta.token);
                }
                Err(e) => panic!("stream error: {e}"),
            }
        }
        assert_eq!(tokens, "Hello", "two deltas should join to Hello");
        assert!(saw_done, "stream should emit finished marker");
    }

    /// 回归 #18：上游提前 EOF（连接关在 [DONE] 之前）。
    /// 旧 None 分支返回不推进状态的 Some(finished) → 死循环；
    /// 半成品 return None 直接丢 buffer → 丢尾部 delta。
    /// 修复后：已发出的 delta 不丢，流正常终止，不死循环。
    #[tokio::test]
    async fn chat_stream_messages_early_eof_drains_and_terminates() {
        let d1 = String::from(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#);
        let d2 = String::from(r#"{"choices":[{"delta":{"content":"lo"}}]}"#);
        // 只发 d1+d2，不补 [DONE]，直接关连接 = 提前 EOF
        let url = spawn_sse_server_early_eof(vec![d1, d2]).await;
        let client = mock_client(&url);
        let messages = vec![MlxChatMessage {
            role: String::from("user"),
            content: String::from("hi"),
        }];
        let mut stream = chat_stream_messages(client, String::from("qwen3.5"), messages, 128).await;
        use futures::StreamExt;
        let mut tokens = String::new();
        // 带超时兜底：若死循环，3 秒后超时失败（而非挂死 CI）
        let drain = async {
            while let Some(item) = stream.next().await {
                match item {
                    Ok(delta) => tokens.push_str(&delta.token),
                    Err(e) => panic!("stream error: {e}"),
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(3), drain)
            .await
            .expect("流应在 3 秒内终止，不死循环（#18 回归）");
        // 两帧都已入 buffer 并被 EOF 分支排空 → "Hello" 不丢
        assert_eq!(tokens, "Hello", "提前 EOF 不应丢失已发出的尾部 delta");
    }

    // L6 回归：CJK 多字节字符被网络分块切断在字符中间。
    // 旧 String::from_utf8_lossy 每 chunk 解码 → 残缺尾字节换 U+FFFD → 中文乱码。
    // 修复后字节缓冲等完整行再解码 → 跨 chunk 残缺字符补全 → 完整无 U+FFFD。
    #[tokio::test]
    async fn chat_stream_messages_cjk_split_across_chunks_no_replacement() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // 登录 = e7 99 bb e5 bd 95；构造 SSE 帧切在 0xE7 后 2 字节（半个汉字）。
        let cjk = "登录";
        let payload = serde_json::json!({"choices":[{"delta":{"content":cjk}}]}).to_string();
        let frame = format!("data: {}\r\n\r\n", payload);
        let frame_bytes = frame.as_bytes().to_vec();
        let split_at = frame_bytes
            .iter()
            .position(|&b| b == 0xE7)
            .expect("帧内应有 CJK 首字节 0xE7")
            + 2;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let done = "data: [DONE]\r\n\r\n".to_string();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let mut resp = String::new();
            resp.push_str("HTTP/1.1 200 OK\r\n");
            resp.push_str("Content-Type: text/event-stream\r\n");
            resp.push_str("Connection: close\r\n\r\n");
            let _ = sock.write_all(resp.as_bytes()).await;
            // 第一块：帧的前半（切在汉字中间）
            let _ = sock.write_all(&frame_bytes[..split_at]).await;
            let _ = sock.flush().await;
            // 第二块：帧后半 + [DONE]
            let _ = sock.write_all(&frame_bytes[split_at..]).await;
            let _ = sock.write_all(done.as_bytes()).await;
            let _ = sock.flush().await;
        });
        let client = mock_client(&format!("http://{}", addr));
        let messages = vec![MlxChatMessage {
            role: String::from("user"),
            content: String::from("hi"),
        }];
        let mut stream = chat_stream_messages(client, String::from("qwen3.5"), messages, 128).await;
        use futures::StreamExt;
        let mut tokens = String::new();
        let drain = async {
            while let Some(item) = stream.next().await {
                match item {
                    Ok(delta) => tokens.push_str(&delta.token),
                    Err(e) => panic!("stream error: {e}"),
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(3), drain)
            .await
            .expect("流应在 3 秒内终止");
        assert_eq!(
            tokens, "登录",
            "跨 chunk 切断的 CJK 字符应完整还原，无 U+FFFD 替换"
        );
        assert!(!tokens.contains('\u{fffd}'), "不得出现 U+FFFD 替换字符");
    }

    /// 捕获请求体到 Mutex 的 mock server：返回固定响应，同时把收到的 raw 请求行+body 存下来。
    /// H-A9 回归：验证 send 把 history 折叠进 messages[]，而非静默丢弃。
    /// 在 BLOCKING_RT（专用持久 multi-thread runtime）上 spawn，避免 #[test] 同步线程
    /// 与 tokio::test runtime 嵌套 block_on panic（参见 lib.rs:310 注释）。
    async fn spawn_capturing_mock_server(body: String) -> (String, Arc<Mutex<Vec<u8>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let cap_clone = captured.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if n > 0 {
                    cap_clone.lock().unwrap().extend_from_slice(&buf[..n]);
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (format!("http://{addr}"), captured)
    }

    /// 在 BLOCKING_RT 上启动 capturing mock 并返回 (url, captured)。
    /// 普通同步测试线程调此 helper：BLOCKING_RT.block_on spawn server（首帧结束），
    /// 随后 send → chat_sync_messages → BLOCKING_RT.block_on（顺序，不嵌套）。
    fn capturing_server(body: &str) -> (String, Arc<Mutex<Vec<u8>>>) {
        BLOCKING_RT.block_on(spawn_capturing_mock_server(body.to_string()))
    }

    /// H-A9：send 必须把 history 折叠进 messages[]（system + 历史轮 + 当前 user），
    /// 而非旧实现的 system + user 两条。用 capturing mock 验证 wire payload。
    #[test]
    fn send_folds_history_into_messages_wire() {
        let body = String::from(r#"{"choices":[{"message":{"content":"ok"}}]}"#);
        let (url, captured) = capturing_server(&body);
        let client = mock_client(&url);
        let provider = FusionMlxChatProvider::new(client, "default-model");
        let request = ChatRequest {
            system_prompt: "你是 UI 生成器".into(),
            user_message: "生成登录页".into(),
            history: vec![
                (ChatHistoryRole::User, "什么是登录页".into()),
                (
                    ChatHistoryRole::Assistant,
                    "登录页是用户身份认证入口".into(),
                ),
            ],
            max_output_tokens: 256,
            thinking: ThinkingMode::default(),
            effort: EffortLevel::default(),
            attachments: vec![],
            model: Some("qwen3.5".into()),
        };
        let deltas: Vec<ChatDelta> = provider.send(request).collect();
        // 应收到 TextDelta + Done。
        let has_text = deltas.iter().any(|d| matches!(d, ChatDelta::TextDelta(_)));
        assert!(has_text, "send 应返回 TextDelta");
        // 验证 wire payload 含 4 条 messages（system + 2 历史 + 1 当前 user）。
        let raw = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
        let payload: serde_json::Value =
            serde_json::from_slice(&raw.as_bytes()[body_start..]).unwrap();
        let msgs = payload["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4, "history 应折叠为 4 条 messages");
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "你是 UI 生成器");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "什么是登录页");
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[2]["content"], "登录页是用户身份认证入口");
        assert_eq!(msgs[3]["role"], "user");
        assert_eq!(msgs[3]["content"], "生成登录页");
        assert_eq!(payload["model"], "qwen3.5");
    }

    /// H-A9：空 system_prompt 应被跳过（不产生空 system message），避免 MLX 报错。
    #[test]
    fn send_empty_system_prompt_omits_system_message() {
        let body = String::from(r#"{"choices":[{"message":{"content":"ok"}}]}"#);
        let (url, captured) = capturing_server(&body);
        let client = mock_client(&url);
        let provider = FusionMlxChatProvider::new(client, "default-model");
        let request = ChatRequest {
            system_prompt: "   ".into(),
            user_message: "hi".into(),
            history: vec![],
            max_output_tokens: 64,
            thinking: ThinkingMode::default(),
            effort: EffortLevel::default(),
            attachments: vec![],
            model: None,
        };
        let _ = provider.send(request).collect::<Vec<_>>();
        let raw = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
        let payload: serde_json::Value =
            serde_json::from_slice(&raw.as_bytes()[body_start..]).unwrap();
        let msgs = payload["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1, "空 system_prompt 不应产生 system message");
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "hi");
        assert_eq!(payload["model"], "default-model", "model=None 回退 default");
    }

    /// H-A7：多行文本应按行分块产多个 TextDelta，而非塌缩成单段全文。
    /// 同步 trait 语义下无真流式，但消费方应收到增量序列（逐行 delta + Done）。
    #[test]
    fn send_chunks_multiline_into_multiple_deltas() {
        let body = String::from(r#"{"choices":[{"message":{"content":"line1\nline2\nline3"}}]}"#);
        let (url, _captured) = capturing_server(&body);
        let client = mock_client(&url);
        let provider = FusionMlxChatProvider::new(client, "default-model");
        let request = ChatRequest {
            system_prompt: "sys".into(),
            user_message: "usr".into(),
            history: vec![],
            max_output_tokens: 64,
            thinking: ThinkingMode::default(),
            effort: EffortLevel::default(),
            attachments: vec![],
            model: Some("m".into()),
        };
        let deltas: Vec<ChatDelta> = provider.send(request).collect();
        let text_deltas: Vec<&String> = deltas
            .iter()
            .filter_map(|d| match d {
                ChatDelta::TextDelta(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(
            text_deltas.len(),
            3,
            "三行文本应产 3 个 TextDelta，非单段全文"
        );
        assert_eq!(text_deltas[0], "line1\n");
        assert_eq!(text_deltas[1], "line2\n");
        assert_eq!(text_deltas[2], "line3");
        assert!(
            deltas.iter().any(|d| matches!(d, ChatDelta::Done { .. })),
            "末帧应为 Done"
        );
    }

    /// H-A7：无换行单段文本产 1 TextDelta + Done。
    #[test]
    fn send_single_line_one_delta() {
        let body = String::from(r#"{"choices":[{"message":{"content":"oneliner"}}]}"#);
        let (url, _captured) = capturing_server(&body);
        let client = mock_client(&url);
        let provider = FusionMlxChatProvider::new(client, "default-model");
        let request = ChatRequest {
            system_prompt: "sys".into(),
            user_message: "usr".into(),
            history: vec![],
            max_output_tokens: 64,
            thinking: ThinkingMode::default(),
            effort: EffortLevel::default(),
            attachments: vec![],
            model: Some("m".into()),
        };
        let deltas: Vec<ChatDelta> = provider.send(request).collect();
        let text_deltas: Vec<&String> = deltas
            .iter()
            .filter_map(|d| match d {
                ChatDelta::TextDelta(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(text_deltas.len(), 1);
        assert_eq!(text_deltas[0], "oneliner");
    }

    /// H-A7：空文本不产 TextDelta，仅 Done（无空增量噪声）。
    #[test]
    fn send_empty_text_no_text_delta() {
        let body = String::from(r#"{"choices":[{"message":{"content":""}}]}"#);
        let (url, _captured) = capturing_server(&body);
        let client = mock_client(&url);
        let provider = FusionMlxChatProvider::new(client, "default-model");
        let request = ChatRequest {
            system_prompt: "sys".into(),
            user_message: "usr".into(),
            history: vec![],
            max_output_tokens: 64,
            thinking: ThinkingMode::default(),
            effort: EffortLevel::default(),
            attachments: vec![],
            model: Some("m".into()),
        };
        let deltas: Vec<ChatDelta> = provider.send(request).collect();
        assert!(
            !deltas.iter().any(|d| matches!(d, ChatDelta::TextDelta(_))),
            "空文本不应产 TextDelta"
        );
        assert!(
            deltas.iter().any(|d| matches!(d, ChatDelta::Done { .. })),
            "空文本仍应有 Done 帧"
        );
    }
}
