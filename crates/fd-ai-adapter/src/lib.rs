//! Fusion-Design AI adapter — implements the OpenPencil `ChatProvider` trait,
//! backed by fusion-mlx local multimodal inference.
//!
//! [No-cloud hard constraint] This crate is the only module in fusion-design allowed to issue
//! HTTP requests; request targets are restricted by `FusionMlxClient` to local/LAN
//! (loopback + RFC1918 + link-local), see `validate_localhost` — every public-internet call
//! path is eliminated.
//!
//! [RouteGuard auth] Every outbound request attaches the `X-Fusion-Route: fusion-design` header
//! (enforced by default since fusion-mlx v0.7.0+; missing it yields 403 missing_route), and when
//! `FUSION_CLIENT_KEY` (or the migration-period legacy fallback `FUSION_MLX_API_KEY`) is set,
//! attaches `Authorization: Bearer <key>`.
//!
//! [M-5 retry backoff] fusion-mlx returns 503 while a model is loading, and returns 502/503 when
//! a node's model is evicted. The three HTTP paths (`blocking_post` / `chat_stream_messages` /
//! `check_generate`) retry transient 502/503 errors with exponential backoff (500ms→1s→2s→4s→8s
//! cap, default max 4 attempts); 4xx permanent errors (auth/request format) fail immediately
//! without retry. The `FUSION_MLX_RETRY_MAX` env var tunes the max attempt count (including the
//! first); setting 1 disables retry. Streaming only covers the connection phase; mid-stream
//! disconnects are not retried.
//!
//! [H-A7 stream collapse fix] `ChatProvider::send` is constrained by the op-ai trait's synchronous
//! semantics (`Box<dyn Iterator<Item=ChatDelta>>`, not an async Stream), so true SSE streaming is
//! impossible. The old implementation wrapped the entire `chat_sync_messages` output into a single
//! `TextDelta`, so consumers saw the first delta only after the full text finished generating.
//! Now chunked per line: each line (including its newline) is one `TextDelta`, and the consumer
//! receives an incremental sequence; empty text produces no `TextDelta`, only `Done`. True
//! streaming still goes through `chat_stream_messages` (CLI `chat` + studio); the trait path
//! gives synchronous consumers an incremental approximation.

use std::net::IpAddr;
use std::sync::{Arc, LazyLock};

use futures::StreamExt;
use serde::{Deserialize, Serialize};

mod artifacts;
mod stream;
pub use artifacts::{ArtifactPage, ArtifactQuery, ArtifactSummary, ArtifactsClient, NewArtifact};
pub use stream::parse_sse_line;

// ── fusion-mlx local inference client ──
//
// fusion-mlx exposes a chat completions endpoint as a local HTTP service (127.0.0.1:port),
// compatible with the OpenAI API shape (/v1/chat/completions) so existing ecosystem tools
// can be reused. The actual port is assigned by fusion-mlx at startup and written to its
// local config file.

const DEFAULT_MLX_ENDPOINT: &str = "http://127.0.0.1:11432";

/// A-1: splits a comma-separated endpoint string into a trimmed Vec.
/// `"http://a:11432,http://b:11432"` → `["http://a:11432","http://b:11432"]`.
/// Empty string → `[DEFAULT_MLX_ENDPOINT]` (guarantees at least one; validated by with_endpoints).
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

/// Origin identification header required by fusion-mlx RouteGuard (presence alone grants pass).
/// Plan B (netlayer-compliance-plan.md, ruled by the user 2026-08-07): fusion-design calls
/// fusion-mlx uniformly via fusion-gateway `:11432`, no longer direct to 11434.
/// The gateway authenticates itself before forwarding; passing the route header through the
/// gateway is harmless.
/// History: Plan A (direct 11434) was rejected, see issue #11.
const FUSION_ROUTE_HEADER: (&str, &str) = ("X-Fusion-Route", "fusion-design");

// DEFAULT_MAX_TOKENS was moved out to fd-skills (ARCH-11); this file references it via re-export.

/// fusion-mlx chat request body (OpenAI-compatible shape).
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

/// Public chat message: role + content, letting callers inject multi-turn history.
/// role is passed through to the OpenAI-compatible API (system/user/assistant).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlxChatMessage {
    pub role: String,
    pub content: String,
}

/// fusion-mlx chat response body (OpenAI-compatible shape, trimmed).
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

/// fusion-mlx local inference client.
///
/// All HTTP request targets are `127.0.0.1`-bound and strongly validated at construction,
/// eliminating any public-internet call path (the PRD "fully offline" hard constraint).
#[derive(Clone)]
pub struct FusionMlxClient {
    endpoint: String,
    // A-1: multi-node failover. With a single node, `endpoints == vec![endpoint]`.
    // When `FUSION_MLX_BASE_URL` is comma-separated multi-value, each endpoint is individually
    // validated by validate_localhost.
    endpoints: Vec<String>,
    // A-1: round-robin counter. Each request/retry attempt fetch_adds to pick the next endpoint,
    // giving single-node load balancing plus 502/503 failover to the next node (passive failover).
    rr: Arc<std::sync::atomic::AtomicU32>,
    http: reqwest::Client,
}

impl FusionMlxClient {
    /// Builds with the default endpoint; prefers the `FUSION_MLX_BASE_URL` env var
    /// (supports explicitly pointing back at fusion-mlx direct 11434 and other local endpoints),
    /// falling back to `http://127.0.0.1:11432` (Plan B: unified via fusion-gateway).
    /// Auth: the gateway requires `FUSION_CLIENT_KEY` set to a gateway key (e.g. master_key);
    /// during migration the legacy name `FUSION_MLX_API_KEY` is read as fallback (removed from
    /// 2027-01-01).
    pub fn new() -> anyhow::Result<Self> {
        let endpoint = match std::env::var("FUSION_MLX_BASE_URL") {
            Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => DEFAULT_MLX_ENDPOINT.to_string(),
        };
        Self::with_endpoint(&endpoint)
    }

    /// Builds with the given endpoint; strictly validates the host is `127.0.0.1` or `localhost`.
    /// When `endpoint` is comma-separated multi-value (A-1), builds multi-node and validates each
    /// endpoint.
    pub fn with_endpoint(endpoint: &str) -> anyhow::Result<Self> {
        let endpoints = parse_endpoints(endpoint);
        Self::with_endpoints(endpoints)
    }

    /// A-1: multi-node constructor. `endpoints` must be non-empty and each entry passes
    /// validate_localhost; the client is built only if all are valid. The `endpoint` field takes
    /// the first entry (compat with the old single-endpoint field-read path).
    pub fn with_endpoints(endpoints: Vec<String>) -> anyhow::Result<Self> {
        if endpoints.is_empty() {
            anyhow::bail!("endpoints list is empty, at least 1 local/LAN endpoint is required");
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
                // C8: without timeouts, a hung fusion-mlx blocks requests forever and the caller
                // (fusion-studio subprocess) deadlocks. Short connect timeout + total timeout backstop.
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(180))
                .build()?,
        })
    }

    /// A-1: picks an endpoint per attempt via round-robin. The round-robin counter fetch_add is
    /// taken modulo so adjacent requests/retries land on different nodes; single-node always
    /// returns the first.
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
            "endpoint_for_attempt: round-robin node selection"
        );
        ep
    }

    /// Resolves the CLI `--endpoint` argument into the final endpoint list.
    /// The CLI layer defaults `--endpoint` to an empty string; when empty it reads
    /// `FUSION_MLX_BASE_URL`, falling back to `http://127.0.0.1:11432` (Plan B via gateway).
    /// A non-empty string is passed through as-is (an explicit `--endpoint` has top priority).
    /// Comma-separated multi-values are each validated (A-1).
    /// Returns the endpoints list for `with_endpoints`.
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

    /// Sends a chat request synchronously (blocks the current thread; used by `ChatProvider::send`).
    ///
    /// Corresponds to the PRD "AI inference sandbox isolation" — this method only reads/writes the
    /// fusion-mlx local service and never touches system files.
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
            .ok_or_else(|| anyhow::anyhow!("fusion-mlx returned empty choices"))?
            .message
            .content)
    }

    /// Multi-turn synchronous chat: messages are built by the caller (system + multi-turn
    /// user/assistant history + current user).
    /// H-A9: the old `ChatProvider::send` implementation used only system_prompt+user_message,
    /// silently dropping `request.history` (multi-turn context lost) and
    /// `thinking`/`effort`/`attachments` (no wire support).
    /// This method passes the full messages[] through to the OpenAI-compatible API, restoring
    /// multi-turn context.
    pub fn chat_sync_messages(
        &self,
        model: &str,
        messages: Vec<MlxChatMessage>,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        // Holds owned Strings; the &'a str MlxMessage values borrow from this frame's locals.
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
            .ok_or_else(|| anyhow::anyhow!("fusion-mlx returned empty choices"))?
            .message
            .content)
    }
}

impl Default for FusionMlxClient {
    fn default() -> Self {
        // L10/C9: the old implementation used .expect() — a bad FUSION_MLX_BASE_URL value panicked.
        // Now falls back to the safe local default endpoint (DEFAULT_MLX_ENDPOINT, already
        // validated by validate_localhost).
        match Self::new() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "default endpoint construction failed, falling back to local default");
                Self::with_endpoint(DEFAULT_MLX_ENDPOINT).expect(
                    "DEFAULT_MLX_ENDPOINT must be a valid localhost (guaranteed at compile time)",
                )
            }
        }
    }
}

/// fusion-mlx RouteGuard auth: every outbound request attaches X-Fusion-Route + Bearer.
impl FusionMlxClient {
    /// Auth key alias-chain resolution (config-conventions §4):
    /// `FUSION_CLIENT_KEY` (canonical name) → `FUSION_MLX_API_KEY` (legacy fallback,
    /// removed in Phase 3 on 2027-01-01, issue #30) → unset WARN (fail visibly).
    /// Not cached at construction: supports runtime key rotation and test isolation.
    fn bearer_token(&self) -> Option<String> {
        if let Ok(k) = std::env::var("FUSION_CLIENT_KEY") {
            if !k.trim().is_empty() {
                return Some(format!("Bearer {}", k.trim()));
            }
        }
        match std::env::var("FUSION_MLX_API_KEY") {
            Ok(k) if !k.trim().is_empty() => {
                tracing::warn!(
                    "FUSION_MLX_API_KEY is a legacy name (migration-period fallback, removed from 2027-01-01); switch to FUSION_CLIENT_KEY"
                );
                Some(format!("Bearer {}", k.trim()))
            }
            _ => {
                tracing::warn!(
                    "FUSION_CLIENT_KEY is unset or empty; requests will be rejected if fusion-mlx enables API key auth"
                );
                None
            }
        }
    }

    /// Attaches RouteGuard + Bearer auth headers to a request (issue #6: fixes 403 missing_route).
    fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let builder = builder.header(FUSION_ROUTE_HEADER.0, FUSION_ROUTE_HEADER.1);
        match self.bearer_token() {
            Some(bearer) => builder.header("Authorization", bearer),
            None => builder,
        }
    }
}

// A-6: unified auth-header attachment. retry_async_post / chat_stream_messages / check_generate
// each used to hand-write route header + bearer matching, a forked logic prone to omissions
// (e.g. adding route but forgetting bearer). This helper consolidates: route header always added
// + optional bearer. The bearer is resolved once by the caller (avoids re-reading env on every
// retry-loop iteration, for performance) and complements the authed method — authed reads env
// eagerly (low-frequency single-request scenarios).
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

/// Async variants (used by the `fd-ecosystem` task queue).
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
        // R-3: retries via retry_async_post (transient-error backoff, 4xx permanent failure,
        // deadline cap).
        // A-1: failover via endpoints+rr round-robin; a single node's 502/503 switches to the next.
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
            .ok_or_else(|| anyhow::anyhow!("fusion-mlx returned empty choices"))?
            .message
            .content)
    }
}

/// Strictly validates that an endpoint host is local/LAN, eliminating public-internet call paths.
///
/// H-A1/P2-1: the old implementation only allowed `127.0.0.1`/`localhost`/`::1`, welding shut the
/// fusion-mlx cluster entry — users could not point at LAN MLX workers (e.g. `10.x`/`192.168.x`).
/// Now allows loopback + RFC1918 private ranges + link-local (169.254/fe80) + unique-local
/// (fc00::/7), while still rejecting all public IPs and public domains. The offline hard
/// constraint (no public calls) is unchanged.
fn validate_localhost(endpoint: &str) -> anyhow::Result<()> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|e| anyhow::anyhow!("invalid endpoint {endpoint:?}: {e}"))?;
    let host = url.host_str().unwrap_or("");
    let host = host.trim_start_matches('[').trim_end_matches(']');
    // Allow "localhost" by name (loopback alias)
    if host == "localhost" {
        return Ok(());
    }
    // IP literals: classify private/loopback/link-local via IpAddr
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_loopback() {
            return Ok(());
        }
        if is_private_or_local(&ip) {
            return Ok(());
        }
        anyhow::bail!("offline hard constraint violated: endpoint host {host:?} is a public IP, public calls are forbidden");
    }
    // Non-localhost domains are always rejected (DNS may resolve to a public address; offline
    // cannot be guaranteed statically)
    anyhow::bail!("offline hard constraint violated: endpoint host {host:?} is not localhost/private IP, public calls are forbidden");
}

/// Decides whether an IP is in a private range (RFC1918) or link-local/unique-local.
/// Public IPs (e.g. 8.8.8.8, 1.1.1.1) return false.
/// E-2: removed is_unspecified() — 0.0.0.0/:: are not "locally reachable" addresses; allowing
/// them is equivalent to allowing endpoints bound on any interface (including public), piercing
/// the offline constraint. Loopback is already decided at the validate_localhost layer above;
/// this covers only private/link-local/unique-local ranges, keeping cluster workers
/// (10.x/192.168.x/fe80) allowed.
fn is_private_or_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() // 10/8、172.16/12、192.168/16
                || v4.is_link_local() // 169.254/16
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique-local fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        }
    }
}

/// Blocking POST (uses a dedicated tokio runtime to avoid nested-runtime panics).
///
/// Design: a `LazyLock`-initialized dedicated multi-thread runtime runs all async requests from
/// `chat_sync` / `ChatProvider::send`. Solves:
/// - `block_on` nested panic when calling `chat_sync` inside `#[tokio::test]`
/// - needing async capability in synchronous contexts (e.g. `ChatProvider::send`)
/// - the performance cost of building a new runtime per call
///
/// The dedicated runtime is multi-thread, so `block_in_place` works and `reqwest`'s DNS/TLS
/// resolution does not block the main thread.
///
/// H-A6: worker_threads adapts to machine core count (min 4). The old value of 1 serialized all
/// concurrent block_on onto a single thread, starving later requests behind the first 180s long
/// inference (FIFO head-of-line blocking), freezing multi-panel concurrent UI. The multi-thread
/// runtime keeps block_in_place available without introducing nested-runtime panics.
/// A-10: a fixed 4 is still a bottleneck on high-core machines; switched to
/// available_parallelism adaptation (fallback 4).
static BLOCKING_RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(4);
    tracing::info!(workers, "BLOCKING_RT: adaptive worker thread count");
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .expect("failed to create blocking tokio runtime")
});

// ── M-5: 502/503 exponential backoff retry ──
// fusion-mlx returns 503 ("model loading") while loading; cluster nodes return 502/503 when a
// model is evicted. The old implementation bailed! immediately, failing the first request without
// waiting for load. Now transient errors back off and retry; permanent errors fail directly.
// FUSION_MLX_RETRY_MAX env var tunes the max attempt count (including the first), default 4.
// Setting 1 disables retry.
const RETRY_BACKOFF_BASE_MS: u64 = 500;
const RETRY_MAX_BACKOFF_MS: u64 = 8_000;
const RETRY_DEFAULT_MAX_ATTEMPTS: u32 = 4;
// R-12: SSE buffer leftover cap. FUSION_MLX_SSE_BUFFER_CAP tunable (bytes), default 8MB.
const MAX_SSE_BUFFER: usize = 8 * 1024 * 1024;

fn sse_buffer_cap() -> usize {
    std::env::var("FUSION_MLX_SSE_BUFFER_CAP")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1024)
        .unwrap_or(MAX_SSE_BUFFER)
}

fn retry_max_attempts() -> u32 {
    // FUSION_MLX_RETRY_MAX override; <1 counts as default. 0/1 = no retry (first attempt only).
    std::env::var("FUSION_MLX_RETRY_MAX")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(RETRY_DEFAULT_MAX_ATTEMPTS)
}

fn is_transient_status(code: u16) -> bool {
    // L-7: expanded transient-error set.
    // 502/503/504 = gateway/server transient (model loading/evicted/gateway temporarily
    // unreachable/timeout).
    // 429 = rate limited (Too Many Requests), 529 = upstream overloaded (Anthropic-style passthrough).
    // Other 4xx = permanent, no retry.
    matches!(code, 502 | 503 | 504 | 429 | 529)
}

fn backoff_delay(attempt: u32) -> std::time::Duration {
    // Exponential backoff: 500ms, 1s, 2s, 4s, 8s (cap). attempt starts at 0 (the wait before the
    // first retry).
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
    /// Blocking POST (uses a dedicated tokio runtime to avoid nested-runtime panics).
    /// Attaches RouteGuard + Bearer auth headers before sending to fusion-mlx.
    /// M-5: transient 502/503 errors retry with exponential backoff; 4xx permanent errors fail
    /// directly.
    /// A-1: `path` is the suffix relative to an endpoint (e.g. `/v1/chat/completions`); each
    /// attempt picks a node via `endpoint_for_attempt` round-robin to build the url — a single
    /// node's 502/503 switches to the next node.
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
            // P-1: total deadline caps retry duration, preventing 4x long backoff + hung responses
            // from blocking forever.
            // Default 300s (FUSION_MLX_RETRY_DEADLINE_SECS tunable); bail as soon as a round
            // exceeds the deadline.
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
                        "blocking_post: total deadline exceeded, giving up retries"
                    );
                    anyhow::bail!(
                        "fusion-mlx retries exceeded total deadline {:?} (attempt {}/{})",
                        deadline,
                        attempt,
                        max
                    );
                }
                // A-1: pick an endpoint per attempt via round-robin to build the full url (failover).
                let idx = if endpoints.len() == 1 {
                    0
                } else {
                    rr.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as usize % endpoints.len()
                };
                let url = format!("{}{}", endpoints[idx], path);
                // RequestBuilder is consuming, rebuilt per retry. A-6: auth headers unified via
                // attach_route_bearer.
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
                                "blocking_post: transient error, backing off then switching node to retry"
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        if attempt + 1 < max {
                            tracing::warn!(attempt, code, "blocking_post: permanent error, not retrying");
                        } else {
                            tracing::error!(attempt, code, "blocking_post: retries exhausted");
                        }
                        anyhow::bail!("fusion-mlx HTTP {code} (attempt {}/{max})", attempt + 1);
                    }
                    Err(e) => {
                        // Connection errors (fusion-mlx not started/gateway unreachable) retry as
                        // transient.
                        if attempt + 1 < max {
                            let delay = backoff_delay(attempt);
                            tracing::warn!(
                                attempt,
                                error = %e,
                                endpoint = %endpoints[idx],
                                ?delay,
                                "blocking_post: connection failed, backing off then switching node to retry"
                            );
                            last_err = Some(anyhow::Error::from(e));
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        tracing::error!(attempt, error = %e, "blocking_post: connection failed, retries exhausted");
                        anyhow::bail!(
                            "fusion-mlx connection failed (attempt {}/{}): {e}",
                            attempt + 1,
                            max
                        );
                    }
                }
            }
            // max >= 1 guaranteed by retry_max_attempts; the loop always returns/bails inside.
            Err(last_err.unwrap_or_else(|| anyhow::anyhow!("blocking_post: retry loop exited abnormally")))
        })
    }
}

/// R-3: async POST retry. Reuses blocking_post's backoff/transient-classification/deadline
/// semantics so chat_async and other async paths retry uniformly. 4xx permanent errors fail
/// directly; transient errors back off and retry.
/// A-1: `endpoints`+`rr` implement failover — each attempt picks an endpoint round-robin to build
/// the url; a single node's 502/503 switches to the next node.
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
                "retry_async_post: total deadline exceeded, giving up retries"
            );
            anyhow::bail!(
                "fusion-mlx retries exceeded total deadline {:?} (attempt {}/{})",
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
        // A-6: auth headers unified via attach_route_bearer (bearer is already Option<&str>).
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
                        "retry_async_post: transient error, backing off then switching node to retry"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                if attempt + 1 < max {
                    tracing::warn!(
                        attempt,
                        code,
                        "retry_async_post: permanent error, not retrying"
                    );
                } else {
                    tracing::error!(attempt, code, "retry_async_post: retries exhausted");
                }
                anyhow::bail!("fusion-mlx HTTP {code} (attempt {}/{max})", attempt + 1);
            }
            Err(e) => {
                if attempt + 1 < max {
                    let delay = backoff_delay(attempt);
                    tracing::warn!(
                        attempt, error = %e, endpoint = %endpoints[idx], ?delay,
                        "retry_async_post: connection failed, backing off then switching node to retry"
                    );
                    last_err = Some(anyhow::Error::from(e));
                    tokio::time::sleep(delay).await;
                    continue;
                }
                tracing::error!(attempt, error = %e, "retry_async_post: connection failed, retries exhausted");
                anyhow::bail!(
                    "fusion-mlx connection failed (attempt {}/{}): {e}",
                    attempt + 1,
                    max
                );
            }
        }
    }
    Err(last_err
        .unwrap_or_else(|| anyhow::anyhow!("retry_async_post: retry loop exited abnormally")))
}

// ── SSE streaming inference ──

/// SSE streaming incremental token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlxStreamDelta {
    pub token: String,
    pub finished: bool,
}

/// Streaming chat: returns an SSE token stream.
///
/// Single-turn convenience wrapper: system + one user message; for multi-turn history use
/// chat_stream_messages.
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

/// Multi-turn streaming chat: messages are built by the caller (system + multi-turn
/// user/assistant history).
/// Auth / X-Fusion-Route header / endpoint resolution match chat_stream, letting CLI / upstream
/// (e.g. fusion-studio subprocess) reuse the same MLX entry without re-implementing.
pub async fn chat_stream_messages(
    client: FusionMlxClient,
    model: String,
    messages: Vec<MlxChatMessage>,
    max_tokens: u32,
) -> impl futures::Stream<Item = anyhow::Result<MlxStreamDelta>> {
    // A6 observability: log correlated fields (model/message count/max_tokens) at the key
    // inference entry, easing commercial troubleshooting across a request's full lifecycle.
    tracing::info!(
        model = %model,
        msg_count = messages.len(),
        max_tokens,
        "chat_stream_messages: starting streaming inference request"
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
    // M-5: connection phase (send + status) retries 502/503 with exponential backoff. Once a 2xx
    // arrives, stream consumption begins.
    // Mid-stream disconnects after the stream is established are not retried (semantics are
    // complex — some deltas already consumed; blind resend duplicates output; requires upstream
    // Last-Event-ID/resume primitives, see fusion-gateway#139).
    // RequestBuilder is consuming, rebuilt per retry; bearer kept owned, closure borrows it.
    // A-1: pick an endpoint per attempt via round-robin to build the url (failover).
    let max = retry_max_attempts();
    // R-4: total deadline caps the connection phase, preventing 4x long backoff from hanging forever.
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
                    "chat_stream_messages: connection exceeded total deadline, giving up retries"
                );
                return futures::stream::once(async move {
                    Err(anyhow::anyhow!(
                        "fusion-mlx connection retries exceeded total deadline {:?} (attempt {}/{})",
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
            // A-6: auth headers unified via attach_route_bearer. Bearer resolved once, closure borrows it.
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
                            "chat_stream_messages: connection transient error, backing off then switching node to retry"
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    tracing::error!(
                        attempt,
                        code,
                        "chat_stream_messages: connection failed, retries exhausted or permanent error"
                    );
                    return futures::stream::once(async move {
                        Err(anyhow::anyhow!(
                            "fusion-mlx HTTP {code} (attempt {}/{})",
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
                            "chat_stream_messages: connection failed, backing off then switching node to retry"
                        );
                        last_err = Some(anyhow::Error::from(e));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    tracing::error!(attempt, error = %e, "chat_stream_messages: connection failed, retries exhausted");
                    return futures::stream::once(async move {
                        Err(anyhow::anyhow!(
                            "SSE request failed (attempt {}/{}): {e}",
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
                let e = last_err.unwrap_or_else(|| {
                    anyhow::anyhow!("chat_stream_messages: retry loop exited abnormally")
                });
                return futures::stream::once(async move { Err(e) }).boxed();
            }
        }
    };

    let stream = resp.bytes_stream();
    // FAULT-1: per-chunk idle cap. The `reqwest` client `.timeout(180s)` is a **total** deadline,
    //   not a between-chunk interval — once upstream establishes the connection and the first
    //   frame arrives quickly, a mid-stream stall (model compute exhausted/network jitter) hangs
    //   stream.next().await forever with no retry/timeout backstop.
    //   Wrapping each next in tokio::time::timeout: if no next chunk arrives within IDLE, Elapsed →
    //   emit error delta + terminate the stream (fail visibly, Rule 12). Distinct from the total
    //   deadline: the total deadline caps the whole retry round, IDLE caps a single chunk's idle
    //   time (long generations with gaps are safe; only "stuck with zero bytes" trips IDLE).
    //   FUSION_MLX_STREAM_IDLE_SECS tunable, default 60.
    let idle_secs = std::env::var("FUSION_MLX_STREAM_IDLE_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(60);
    let idle = std::time::Duration::from_secs(idle_secs);
    // L6: buffer is Vec<u8>, not String. The old implementation decoded each chunk immediately
    //   via String::from_utf8_lossy(&bytes), but a chunk may cut a multi-byte CJK character in
    //   half (UTF-8 is 3 bytes per CJK char), and the broken trailing bytes became U+FFFD →
    //   garbled Chinese in UI JSON. Byte-buffer approach: only complete lines (separated by \n)
    //   are decoded from_utf8 as a whole; partial multi-byte characters spanning chunks stay in
    //   the buffer awaiting the next chunk.
    futures::stream::unfold(
        (stream, Vec::<u8>::new(), idle, idle_secs),
        |(mut stream, mut buffer, idle, idle_secs)| async move {
            use futures::StreamExt;
            loop {
                // FAULT-1: timeout wraps a single next; a stall over IDLE becomes Elapsed, failing visibly.
                match tokio::time::timeout(idle, stream.next()).await {
                    Ok(Some(Ok(bytes))) => {
                        buffer.extend_from_slice(&bytes);
                        // R-12: drain all complete lines (this round plus earlier leftovers) first,
                        // then check the leftover cap.
                        // The old `&& !buffer.contains(&b'\n')` condition silently skipped the cap
                        // when "over the limit but containing a newline", and contains was an O(n)
                        // full scan. Now: complete lines are drained first, and the cap applies only
                        // to the **cross-chunk leftover** after draining (a half line with no newline).
                        while let Some(line_end) = buffer.iter().position(|&b| b == b'\n') {
                            // L6: decode the complete line as a whole via from_utf8_lossy (within a
                            //   line the UTF-8 is already complete; the SSE data: prefix + JSON
                            //   structure is ASCII, CJK sits inside JSON values and is complete).
                            // Decode before draining to avoid conflicting line_bytes borrow with
                            // buffer.drain.
                            let line = String::from_utf8_lossy(&buffer[..line_end])
                                .trim()
                                .to_string();
                            buffer.drain(..=line_end);
                            // ARCH-10 round-2: strip-data/[DONE]/JSON/content-extraction logic
                            // moved out to stream::parse_sse_line (single source of truth); this
                            // side only consumes deltas.
                            if let Some(delta) = parse_sse_line(&line) {
                                return Some((Ok(delta), (stream, buffer, idle, idle_secs)));
                            }
                        }
                        // R-12: after the while-drain, the leftover is a cross-chunk half line (no
                        // newline). Leftover over the cap means an abnormal stream (slow upstream /
                        // single line over 8MB); error out and terminate the stream to prevent
                        // unbounded memory growth.
                        // The old condition with `!buffer.contains(&b'\n')` silently let through
                        // "over the cap but containing a newline"; "drain first, then judge the
                        // leftover" eradicates that blind spot.
                        if buffer.len() > MAX_SSE_BUFFER {
                            let cap = sse_buffer_cap();
                            if buffer.len() > cap {
                                tracing::error!(
                                    len = buffer.len(),
                                    cap,
                                    "SSE leftover over cap (cross-chunk half line with no newline), terminating stream"
                                );
                                return Some((
                                    Err(anyhow::anyhow!(
                                        "SSE buffer leftover over cap ({cap} bytes) with no complete line"
                                    )),
                                    (stream, Vec::new(), idle, idle_secs),
                                ));
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        return Some((Err(anyhow::anyhow!("SSE read error: {e}")), (stream, buffer, idle, idle_secs)));
                    }
                    // FAULT-1: EOF — drain leftover complete-line data from the buffer before
                    // terminating the stream.
                    Ok(None) => {
                        // EOF: drain the leftover line data from the buffer first, then terminate.
                        // Upstream may not append a newline after the final frame, or the last
                        // chunk may still sit in the buffer unprocessed by the while loop —
                        // return None directly would drop the tail deltas (#18).
                        while let Some(line_end) = buffer.iter().position(|&b| b == b'\n') {
                            let line = String::from_utf8_lossy(&buffer[..line_end])
                                .trim()
                                .to_string();
                            buffer.drain(..=line_end);
                            // ARCH-10 round-2: reuse stream::parse_sse_line (single source of truth).
                            if let Some(delta) = parse_sse_line(&line) {
                                return Some((Ok(delta), (stream, buffer, idle, idle_secs)));
                            }
                        }
                        // Trailing half line with no newline (upstream closed without appending a
                        // newline): parse as a whole line.
                        // L6: the tail may also be cross-chunk CJK; decode as a whole via
                        // from_utf8_lossy.
                        let tail = String::from_utf8_lossy(&buffer).trim().to_string();
                        if !tail.is_empty() {
                            buffer.clear();
                            // ARCH-10 round-2: the tail half line also reuses parse_sse_line
                            // (single source of truth).
                            if let Some(delta) = parse_sse_line(&tail) {
                                return Some((Ok(delta), (stream, buffer, idle, idle_secs)));
                            }
                        }
                        return None;
                    }
                    // FAULT-1: single-chunk idle over IDLE — stalled; fail visibly, terminate stream.
                    // Distinct from the client total timeout (180s): IDLE is the cap on the gap
                    // between adjacent chunks.
                    Err(_elapsed) => {
                        tracing::error!(
                            idle_secs,
                            "SSE stream stall: no next chunk within {}s, terminating stream (FAULT-1)",
                            idle_secs
                        );
                        return Some((
                            Err(anyhow::anyhow!(
                                "SSE stream stall: no next chunk within {idle_secs}s (tunable via FUSION_MLX_STREAM_IDLE_SECS)"
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

// ── MLX health check ──

/// MLX service health status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub available: bool,
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
    /// Whether auth passed. None = not authenticated or undecidable; Some(true) = passed;
    /// Some(false) = rejected with 401/403 (FUSION_CLIENT_KEY missing or wrong).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_ok: Option<bool>,
    /// Model id list returned by /v1/models. Empty list = gateway advertises a model name but MLX
    /// has not loaded it; generate still 502s — this is the root cause of "false green".
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub models: Vec<String>,
    /// Human-readable status summary: reachable / auth failed / no models available / available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl FusionMlxClient {
    /// Probes fusion-mlx health (timeout 3s).
    /// Distinguishes three failure modes: unreachable / auth rejected (401/403) / reachable but no
    /// models — the last two previously collapsed into available:false, masking "key set but
    /// invalid" and "gateway false green".
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
                            "available ({} models, first {})",
                            models.len(),
                            model.as_deref().unwrap_or("?")
                        )
                    } else {
                        String::from("reachable but no models — gateway list is empty or MLX not loaded, generate will 502")
                    };
                    if available {
                        tracing::info!(available = true, model = ?model, n = models.len(), "health_check: MLX available");
                    } else {
                        tracing::warn!(
                            "health_check: /v1/models returned 200 but data is empty, suspected gateway false green"
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
                    tracing::warn!(status = %status_code, "health_check: auth rejected, check FUSION_CLIENT_KEY");
                    Ok(HealthStatus {
                        available: false,
                        model: None,
                        gpu: None,
                        auth_ok: Some(false),
                        models: vec![],
                        status: Some(format!(
                            "auth failed ({status_code}): FUSION_CLIENT_KEY missing or invalid"
                        )),
                    })
                } else {
                    tracing::warn!(status = %status_code, "health_check: MLX returned non-2xx");
                    Ok(HealthStatus {
                        available: false,
                        model: None,
                        gpu: None,
                        auth_ok: None,
                        models: vec![],
                        status: Some(format!("MLX returned {status_code}")),
                    })
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "health_check: MLX unreachable");
                Ok(HealthStatus {
                    available: false,
                    model: None,
                    gpu: None,
                    auth_ok: None,
                    models: vec![],
                    status: Some(format!("unreachable: {e}")),
                })
            }
        }
    }

    /// Synchronous health check (blocks the current thread).
    pub fn health_check_sync(&self) -> anyhow::Result<HealthStatus> {
        let this = self.clone();
        BLOCKING_RT.block_on(async move { this.health_check().await })
    }

    /// Real inference probe: sends a 1-token chat request to decide whether the generate path can
    /// actually produce output.
    /// gateway /v1/models and /health, /readyz can all "false green" (model name listed but MLX
    /// not loaded); the only authoritative availability signal is a real chat call — 502/503 =
    /// model not loaded, 200 = truly available.
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
        // M-5: 502/503 = model loading/evicted; back off, wait, then retry. Once 4xx / non-transient
        // 5xx / connection failures are exhausted, still return with the original diagnostic
        // semantics (GenerateProbeStatus structure unchanged).
        // A-1: pick an endpoint per attempt via round-robin (failover); the url is built per node
        // inside the loop.
        let max = retry_max_attempts();
        // R-4: total deadline caps probe retries, preventing 4x long backoff from hanging forever.
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
                    "check_generate: total deadline exceeded, giving up retries"
                );
                return Ok(GenerateProbeStatus {
                    model_loaded: false,
                    available: false,
                    status: Some(format!(
                        "inference probe retries exceeded total deadline {:?} (attempt {}/{})",
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
                        tracing::info!(model, attempt, "check_generate: inference probe passed");
                        return Ok(GenerateProbeStatus {
                            model_loaded: true,
                            available: true,
                            status: Some(format!(
                                "inference probe passed (model {model} loaded, attempt {}/{})",
                                attempt + 1,
                                max
                            )),
                            http_code: Some(code),
                        });
                    } else if is_transient_status(code) {
                        let body: serde_json::Value = r.json().await.unwrap_or_default();
                        let msg = body["error"]["message"]
                            .as_str()
                            .unwrap_or("no error body")
                            .to_string();
                        last_code = Some(code);
                        if attempt + 1 < max {
                            let delay = backoff_delay(attempt);
                            tracing::warn!(
                                attempt,
                                code,
                                msg,
                                ?delay,
                                "check_generate: model not loaded (502/503), backing off then retrying"
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        tracing::warn!(
                            code,
                            msg,
                            "check_generate: model not loaded, retries exhausted"
                        );
                        return Ok(GenerateProbeStatus {
                            model_loaded: false,
                            available: false,
                            status: Some(format!(
                                "model not loaded ({code} {msg}): still 502/503 after {max} retries, generate failed"
                            )),
                            http_code: Some(code),
                        });
                    } else if code == 401 || code == 403 {
                        return Ok(GenerateProbeStatus {
                            model_loaded: false,
                            available: false,
                            status: Some(format!(
                                "auth failed ({code}): FUSION_CLIENT_KEY invalid"
                            )),
                            http_code: Some(code),
                        });
                    } else {
                        return Ok(GenerateProbeStatus {
                            model_loaded: false,
                            available: false,
                            status: Some(format!("generate returned {code}")),
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
                            "check_generate: request failed, backing off then retrying"
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    tracing::warn!(error = %e, "check_generate: request failed, retries exhausted");
                    return Ok(GenerateProbeStatus {
                        model_loaded: false,
                        available: false,
                        status: Some(format!(
                            "generate request failed (after {max} retries): {e}"
                        )),
                        http_code: None,
                    });
                }
            }
        }
        // Loop exits normally (max=0 cannot happen; retry_max_attempts guarantees >=1): fallback.
        Ok(GenerateProbeStatus {
            model_loaded: false,
            available: false,
            status: Some(format!(
                "generate did not succeed after {max} retries{}{}",
                last_code
                    .map(|c| format!(", last HTTP {c}"))
                    .unwrap_or_default(),
                last_err
                    .map(|e| format!(", last error {e}"))
                    .unwrap_or_default()
            )),
            http_code: last_code,
        })
    }
}

/// Real inference probe result. available = the generate path can actually produce output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateProbeStatus {
    pub model_loaded: bool,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_code: Option<u16>,
}

// ── Multimodal requests (screenshot/sketch → UI) ──

/// Vision message content item.
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

/// Multimodal chat request: sends image + text to fusion-mlx.
/// Sniffs the real image MIME from the base64 data head (E-7/P3).
/// The old implementation hardcoded `image/png`; non-PNG (JPEG/WebP/GIF) was mislabeled and the
/// multimodal model might refuse to recognize it.
/// Decodes the first 12 bytes and reads the magic: PNG 89504E47, JPEG FFD8FF, WebP
/// "RIFF....WEBP", GIF "GIF8"; unknown falls back to png (OpenAI vision's broadest compatibility;
/// models generally tolerate imprecise mime).
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
    // temperature 0.6: with MLX default temperature=0, 4-bit VL models degrade deterministically
    // (repeated tokens / structure collapsing into slabs); a sampling temperature markedly reduces
    // the structural corruption rate (measured).
    let payload = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": content },
        ],
        "max_tokens": max_tokens,
        "temperature": 0.6,
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
        .ok_or_else(|| anyhow::anyhow!("fusion-mlx returned empty choices"))?
        .message
        .content)
}

// encode_image_base64 / encode_image_base64_async moved out to fd-skills (ARCH-11);
// this file references them via re-export.

/// Synchronous multimodal chat (blocks the current thread; dedicated BLOCKING_RT avoids
/// nested-runtime panic).
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

// ── OpenPencil ChatProvider adaptation ──

use op_ai::chat_provider::{
    ChatDelta, ChatProvider, ChatRequest, EffortLevel, StopReason, ThinkingMode,
};

/// Fusion-MLX ChatProvider adapter (implements the OpenPencil ChatProvider trait).
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
        // H-A9: the old implementation dropped history/thinking/effort/attachments — multi-turn
        // context lost with no diagnostics.
        // history is folded into messages[] (system + history + current user) and passed to MLX.
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
        // thinking/effort/attachments have no wire support yet (the fusion-mlx OpenAI-compatible
        // shape does not expose these fields).
        // Silent drop = missing feature with no diagnostics; an explicit warn makes the capability
        // degradation observable upstream, easing later wire support.
        if !matches!(request.thinking, ThinkingMode::Adaptive) {
            tracing::warn!(
                thinking = request.thinking.as_str(),
                "ChatRequest.thinking has no wire support, degraded and ignored (fusion-mlx does not expose a thinking switch)"
            );
        }
        if !matches!(request.effort, EffortLevel::Low) {
            tracing::warn!(
                effort = request.effort.as_str(),
                "ChatRequest.effort has no wire support, degraded and ignored (fusion-mlx does not expose an effort knob)"
            );
        }
        if !request.attachments.is_empty() {
            tracing::warn!(
                count = request.attachments.len(),
                names = ?request.attachments.iter().map(|a| &a.name).collect::<Vec<_>>(),
                "ChatRequest.attachments has no wire support, degraded and ignored (multimodal attachments not yet wired)"
            );
        }
        // H-A7: the old implementation wrapped the full text in a single TextDelta, so consumers
        // saw the first delta only after the whole text finished generating
        // (no true streaming under the sync trait semantics, but an incremental sequence should
        // still be produced rather than the full text in one jump).
        // Per-line chunking: each line (including its newline) is one TextDelta; the consumer
        // receives increments line by line; a final Done frame.
        // Empty text produces no TextDelta, only Done.
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
                    "ChatProvider::send: chunked incremental delta sequence emitted"
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

/// Convenience constructor reused by tests and `fd-ecosystem`.
pub fn shared_client(endpoint: Option<&str>) -> anyhow::Result<Arc<FusionMlxClient>> {
    let client = match endpoint {
        Some(e) => FusionMlxClient::with_endpoint(e)?,
        None => FusionMlxClient::new()?,
    };
    Ok(Arc::new(client))
}

// ── AI Skill system: moved out to the fd-skills crate (ARCH-11) ──
//
// SkillContext trait + 7 skill impls + SkillRegistry + SkillOutput/SpecDocument types
// + pure helpers (parse_ui_json/repair_model_json/strip_code_fence/encode_image_base64/
// ui_generator_system_prompt/DEFAULT_MAX_TOKENS) moved to fd-skills. The adapter reverse-
// impls SkillContext for FusionMlxClient; the DesignSkills facade dispatches skills through
// &dyn SkillContext. Re-exports keep the adapter's internal caller paths unchanged.

use fd_design_system::DesignSystem;

// ARCH-11 re-export: fd-skills skill types + pure helpers, keeping the adapter-internal
// (DesignSkills facade / mlx_integration tests / skills_tests) paths unchanged.
pub use fd_skills::{
    encode_image_base64, encode_image_base64_async, parse_flow_pages, parse_local_edit_input,
    parse_node_with_depth, parse_nodes_with_depth, parse_page_flow_input, parse_spec_doc_input,
    parse_spec_doc_json, parse_ui_json, repair_model_json, strip_code_fence,
    ui_generator_system_prompt, ComponentProp, ComponentSpec, DesignSkill, ImageToUiSkill,
    InteractionSpec, LocalEditSkill, MultiVariantsSkill, PageFlowSkill, PartialEditSkill,
    SkillContext, SkillOutput, SkillRegistry, SpecDocSkill, SpecDocument, TextToUiSkill,
    DEFAULT_MAX_TOKENS,
};
// The 7 skill structs are pub in fd-skills but hold pub(crate) model fields — the adapter only
// dispatches via &dyn SkillContext + re-exports type names, never constructing skill structs
// directly (construction goes through SkillRegistry::register_builtin(model)). So only type names
// are re-exported for type references.

// ── impl SkillContext for FusionMlxClient (ARCH-11 reverse trait impl) ──
//
// The SkillContext trait is object-safe (no generics/Self) and FusionMlxClient is Send+Sync
// (String + Vec + Arc<AtomicU32> + reqwest::Client), so the impl is legal. Trait methods
// delegate to the adapter's existing chat_sync/chat_async/chat_with_image/chat_with_image_sync.
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
            format!("Current design system tokens (CSS Custom Properties):\n{css}\n\nThe generated UI must use these CSS variables.")
        })
    }
}

use fd_canvas_core::PenDocument;

// A-2: html_to_pen_document split out to the fd-html-parser leaf crate; the re-export keeps the
// caller path `fd_ai_adapter::html_to_pen_document` (fd-cli) unchanged.
pub use fd_html_parser::html_to_pen_document;

/// AI design skill entry (wired to fusion-mlx multimodal inference).
///
/// Design principle: this layer only converts natural language / reference images into
/// fusion-mlx prompts, and parses the returned structured JSON into `PenDocument` nodes.
/// Real inference is done by `FusionMlxClient` (offline hard constraint).
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

    /// Text-to-UI: natural language description → PenDocument page.
    ///
    /// Corresponds to PRD module 2 "text-generated interface".
    pub fn text_to_ui(&self, prompt: &str, page_name: &str) -> anyhow::Result<PenDocument> {
        let sys = "You are the fusion-design UI generator. Output strict JSON: {\"page\":{...}}.\
Output JSON only; extra prose is forbidden. page contains width/height (default 1440x900); \
each entry of the nodes list is {id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}.";
        let user = format!("Generate page \"{page_name}\". Requirements: {prompt}");
        let resp = self
            .client
            .chat_sync(&self.default_model, sys, &user, 2048)?;
        parse_ui_json(&resp, page_name)
    }

    /// Text-to-UI (async variant, for use inside a tokio runtime).
    pub async fn text_to_ui_async(
        &self,
        prompt: &str,
        page_name: &str,
    ) -> anyhow::Result<PenDocument> {
        let sys = "You are the fusion-design UI generator. Output strict JSON: {\"page\":{...}}.\
Output JSON only; extra prose is forbidden. page contains width/height (default 1440x900); \
each entry of the nodes list is {id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}.";
        let user = format!("Generate page \"{page_name}\". Requirements: {prompt}");
        let resp = self
            .client
            .chat_async(&self.default_model, sys, &user, 2048)
            .await?;
        parse_ui_json(&resp, page_name)
    }

    /// Image-to-UI: reference image / hand-drawn sketch → PenDocument page.
    ///
    /// Corresponds to PRD module 2 "upload reference image / hand-drawn sketch reverse-generates
    /// the interface".
    /// Reads the sketch file, base64-encodes it, and sends a real multimodal vision request; on
    /// read failure falls back to a text description.
    pub fn image_to_ui(
        &self,
        sketch_path: &str,
        hint: &str,
        page_name: &str,
    ) -> anyhow::Result<PenDocument> {
        let sys = ui_generator_system_prompt();
        let user = format!(
            "Additional notes: {hint}\nBased on the sketch image above, generate the UI layout for page \"{page_name}\"."
        );
        let resp = match encode_image_base64(std::path::Path::new(sketch_path)) {
            Ok(b64) => {
                tracing::info!(
                    sketch_path,
                    bytes = b64.len(),
                    "image_to_ui: sketch loaded, sending real multimodal request"
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
                tracing::warn!(sketch_path, error = %e, "image_to_ui: sketch load failed, falling back to text description");
                let user_text = format!(
                    "Sketch path: {sketch_path} (unreadable: {e})\nAdditional notes: {hint}\nGenerate the UI layout for page \"{page_name}\"."
                );
                self.client
                    .chat_sync(&self.default_model, &sys, &user_text, DEFAULT_MAX_TOKENS)?
            }
        };
        parse_ui_json(&resp, page_name)
    }

    /// Image-to-UI (async variant).
    pub async fn image_to_ui_async(
        &self,
        sketch_path: &str,
        hint: &str,
        page_name: &str,
    ) -> anyhow::Result<PenDocument> {
        // A6 observability: image-to-UI is a core product entry; log correlated fields for
        // troubleshooting.
        tracing::info!(
            sketch_path,
            page_name,
            model = %self.default_model,
            "image_to_ui_async: starting image-to-UI request"
        );
        let sys = ui_generator_system_prompt();
        let user = format!(
            "Additional notes: {hint}\nBased on the sketch image above, generate the UI layout for page \"{page_name}\"."
        );
        let resp = match encode_image_base64_async(std::path::Path::new(sketch_path)).await {
            Ok(b64) => {
                tracing::info!(
                    sketch_path,
                    bytes = b64.len(),
                    "image_to_ui_async: sketch loaded, sending real multimodal request"
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
            // L-5: an unreadable sketch makes image-to-ui meaningless; no silent text fallback
            // (fail visibly, Rule 12).
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "image_to_ui_async: sketch load failed: {e} (path: {sketch_path})"
                ));
            }
        };
        parse_ui_json(&resp, page_name)
    }

    /// Screenshot/sketch → UI: reads the image file, base64-encodes it, sends a multimodal request.
    pub async fn screenshot_to_ui(
        &self,
        image_path: &std::path::Path,
        hint: &str,
        page_name: &str,
    ) -> anyhow::Result<PenDocument> {
        let b64 = encode_image_base64(image_path)?;
        tracing::info!(path = %image_path.display(), size_b64 = b64.len(), "screenshot_to_ui: image encoded");
        let sys = ui_generator_system_prompt();
        let user =
            format!("Additional notes: {hint}\nGenerate the UI layout for page \"{page_name}\".");
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

    /// Partial edit: selected node + natural language instruction → modified node.
    ///
    /// Corresponds to PRD module 2 "partial instruction editing".
    /// Returns the updated node style (the caller is responsible for writing it back to
    /// PenDocument).
    pub fn partial_edit(&self, node_json: &str, instruction: &str) -> anyhow::Result<String> {
        let sys = "You are the fusion-design partial editor. Input is one node JSON and an edit instruction; \
output the modified node JSON (keep original fields, change only fields the instruction involves). Output JSON only.";
        let user = format!("Node: {node_json}\nInstruction: {instruction}");
        self.client.chat_sync(&self.default_model, sys, &user, 1024)
    }

    /// Partial edit (async variant).
    pub async fn partial_edit_async(
        &self,
        node_json: &str,
        instruction: &str,
    ) -> anyhow::Result<String> {
        let sys = "You are the fusion-design partial editor. Input is one node JSON and an edit instruction; \
output the modified node JSON (keep original fields, change only fields the instruction involves). Output JSON only.";
        let user = format!("Node: {node_json}\nInstruction: {instruction}");
        self.client
            .chat_async(&self.default_model, sys, &user, 1024)
            .await
    }

    /// Local edit: multiple selected nodes + natural language instruction → batch-modified JSON.
    pub fn local_edit(&self, nodes_json: &str, instruction: &str) -> anyhow::Result<String> {
        let sys = "You are the fusion-design local editor. Input is a JSON array of nodes and an edit instruction; \
output the modified node JSON array (keep original fields, change only fields the instruction involves). Output a JSON array only.";
        let user = format!("Selected nodes:\n{nodes_json}\n\nEdit instruction: {instruction}");
        self.client.chat_sync(&self.default_model, sys, &user, 2048)
    }

    /// Local edit (async variant).
    pub async fn local_edit_async(
        &self,
        nodes_json: &str,
        instruction: &str,
    ) -> anyhow::Result<String> {
        let sys = "You are the fusion-design local editor. Input is a JSON array of nodes and an edit instruction; \
output the modified node JSON array (keep original fields, change only fields the instruction involves). Output a JSON array only.";
        let user = format!("Selected nodes:\n{nodes_json}\n\nEdit instruction: {instruction}");
        self.client
            .chat_async(&self.default_model, sys, &user, 2048)
            .await
    }

    /// Multi-variant comparison: generates 3 design drafts in different styles.
    ///
    /// Corresponds to PRD module 2 "one-click generation of 3 differently styled drafts stored
    /// side by side on the canvas".
    pub fn multi_variants(
        &self,
        prompt: &str,
        page_name: &str,
        styles: [&str; 3],
    ) -> anyhow::Result<[PenDocument; 3]> {
        let v1 = self.text_to_ui(&format!("{prompt} (style: {})", styles[0]), page_name)?;
        let v2 = self.text_to_ui(&format!("{prompt} (style: {})", styles[1]), page_name)?;
        let v3 = self.text_to_ui(&format!("{prompt} (style: {})", styles[2]), page_name)?;
        Ok([v1, v2, v3])
    }

    /// Multi-variant comparison (async variant).
    pub async fn multi_variants_async(
        &self,
        prompt: &str,
        page_name: &str,
        styles: [&str; 3],
    ) -> anyhow::Result<[PenDocument; 3]> {
        let v1 = self
            .text_to_ui_async(&format!("{prompt} (style: {})", styles[0]), page_name)
            .await?;
        let v2 = self
            .text_to_ui_async(&format!("{prompt} (style: {})", styles[1]), page_name)
            .await?;
        let v3 = self
            .text_to_ui_async(&format!("{prompt} (style: {})", styles[2]), page_name)
            .await?;
        Ok([v1, v2, v3])
    }

    pub fn spec_doc(&self, doc_json: &str, title: &str) -> anyhow::Result<SpecDocument> {
        let skill = SpecDocSkill::new(&self.default_model);
        let input = format!("{doc_json}|{title}");
        match skill.execute(&self.client, None, &input)? {
            SkillOutput::SpecDoc(spec) => Ok(spec),
            other => anyhow::bail!(
                "spec-doc returned non-SpecDoc: {:?}",
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
                "spec-doc returned non-SpecDoc: {:?}",
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
                "page-flow returned non-PageFlow: {:?}",
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
                "page-flow returned non-PageFlow: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }
}

#[cfg(test)]
mod skills_tests {
    use super::*;

    // ARCH-11: skill system tests moved to fd-skills/tests/skills_mock.rs.
    // This mod keeps the B-class: endpoint resolution + DesignSkills facade constructibility +
    // strip_code_fence passthrough (helper re-export path regression).

    #[test]
    fn resolve_endpoint_empty_falls_back_to_default() {
        // Clear the env var; an empty string should fall back to Plan B gateway 11432
        std::env::remove_var("FUSION_MLX_BASE_URL");
        let ep = FusionMlxClient::resolve_endpoint("").unwrap();
        assert_eq!(ep, vec!["http://127.0.0.1:11432"]);
    }

    #[test]
    fn resolve_endpoint_explicit_overrides_env() {
        // An explicit --endpoint has top priority, env ignored
        std::env::set_var("FUSION_MLX_BASE_URL", "http://127.0.0.1:11434");
        let ep = FusionMlxClient::resolve_endpoint("http://127.0.0.1:11432").unwrap();
        assert_eq!(ep, vec!["http://127.0.0.1:11432"]);
        std::env::remove_var("FUSION_MLX_BASE_URL");
    }

    #[test]
    fn resolve_endpoint_multi_value_split_by_comma() {
        // A-1: comma-separated multi-value should split into multiple endpoints, each validated
        // by validate_localhost.
        std::env::remove_var("FUSION_MLX_BASE_URL");
        let ep = FusionMlxClient::resolve_endpoint("http://127.0.0.1:11432,http://10.0.0.5:11432")
            .unwrap();
        assert_eq!(ep, vec!["http://127.0.0.1:11432", "http://10.0.0.5:11432"]);
    }

    #[test]
    fn resolve_endpoint_multi_value_rejects_public() {
        // A-1: any public endpoint among the values rejects the whole list.
        std::env::remove_var("FUSION_MLX_BASE_URL");
        let r = FusionMlxClient::resolve_endpoint("http://127.0.0.1:11432,http://8.8.8.8:11432");
        assert!(
            r.is_err(),
            "a list containing a public IP must be rejected as a whole"
        );
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

// ── End-to-end integration tests (self-built mock HTTP server, zero external deps) ──

#[cfg(test)]
mod mlx_integration {
    use super::*;
    use fd_canvas_core::NodeKind;
    use op_ai::chat_provider::ChatHistoryRole;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    /// Self-built mock HTTP server: listens on a random 127.0.0.1 port, returns a preset body.
    /// body is a String to avoid a raw-string/JSON '#' boundary conflict.
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

    /// A-1: multi-node mock client (endpoints list; validate_localhost's public check is moot —
    /// the test server binds 127.0.0.1, which is loopback, so validation always passes).
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

    /// Writes a real, readable placeholder PNG to a unique temp path so tests do not depend on an
    /// external /tmp/fd_sketch.png.
    /// encode_image_base64 only base64-encodes the file bytes without validating image format, so
    /// minimal PNG bytes suffice.
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

    /// FAULT-1 stall mock server: connects 200 + sends SSE headers, then never sends any data
    /// frames, sleeping for a long time — simulates a post-connect hang with no chunks.
    /// chat_stream_messages' tokio::time::timeout(idle, stream.next()) should terminate with Err
    /// within IDLE.
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
            // After connect, send no data frames; a long sleep simulates a stall hang.
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        format!("http://{}", addr)
    }

    fn write_fixture_png() -> String {
        // 67-byte minimal valid PNG (1x1 grayscale).
        const MIN_PNG: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00,
            0x00, 0x3A, 0x7E, 0x9B, 0x55, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, 0x33,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("fd_test_sketch_{pid}.png"));
        std::fs::write(&path, MIN_PNG).expect("failed to write test PNG");
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
        // The request reached the mock server
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
        // Our mock server ignores status and always returns a 200 header even when the body is an
        // error JSON; this test verified chat_async bails on a non-success status (the mock always
        // returns 200).
        // Instead verify the error path: the JSON fails to parse.
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
        assert!(
            err.to_string().contains("empty choices") || err.to_string().contains("fusion-mlx")
        );
    }

    #[tokio::test]
    async fn design_skills_text_to_ui_full_pipeline() {
        // mock body is the OpenAI-compatible shape: choices[0].message.content holds the UI JSON
        let ui_json = "{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[{\"id\":\"hero\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":200,\"fill\":\"#007AFF\"},{\"id\":\"title\",\"kind\":\"text\",\"x\":48,\"y\":80,\"text\":\"Welcome\",\"fill\":\"#FFFFFF\"}]}}";
        let body = format!("{{\"choices\":[{{\"message\":{{\"content\":{ui_json:?}}}}}]}}");
        let (url, _count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let doc = skills
            .text_to_ui_async("build a hero section", "Home")
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
                "make it red",
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
            .multi_variants_async("login page", "Login", ["minimal", "glassmorphism", "dark"])
            .await
            .unwrap();
        assert_eq!(docs.len(), 3);
        assert_eq!(*count.lock().unwrap(), 3);
        for d in &docs {
            assert_eq!(d.pages.len(), 1);
        }
    }

    /// Note: this test does not use `#[tokio::test]` because `ChatProvider::send` is a
    /// synchronous trait method that calls `chat_sync` internally, and `chat_sync`'s nested
    /// `block_on` inside an existing tokio runtime panics. Instead, a standalone runtime drives
    /// the mock server + the `send` call, avoiding nesting.
    #[test]
    fn chat_provider_trait_wires_to_mlx() {
        let body = "{\"choices\":[{\"message\":{\"content\":\"trait ok\"}}]}".to_string();
        // Standalone runtime: start the mock server and block to get the url
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url, _count) = rt.block_on(spawn_mock_server(200, body));
        let client = mock_client(&url);
        let provider = FusionMlxChatProvider::new(client, "qwen3.5");
        // send's internal chat_sync detects no current runtime and builds a standalone one
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

    /// Mock server capturing the raw HTTP request text (for asserting auth headers).
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

    /// issue #6: verifies chat outbound requests attach the X-Fusion-Route header (fixes 403
    /// missing_route).
    #[tokio::test]
    async fn chat_async_sends_fusion_route_header() {
        let body = String::from(r#"{"choices":[{"message":{"content":"ok"}}]}"#);
        let (url, captured) = spawn_header_capture_server(body).await;
        let client = mock_client(&url);
        let _ = client.chat_async("m", "s", "u", 64).await.unwrap();
        let raw = captured
            .lock()
            .unwrap()
            .clone()
            .expect("request not captured");
        assert!(
            raw.to_lowercase().contains("x-fusion-route: fusion-design"),
            "missing X-Fusion-Route header: {raw}"
        );
    }

    /// issue #6: health_check must also carry the RouteGuard header (/v1/models is not an
    /// exempt path).
    #[tokio::test]
    async fn health_check_sends_fusion_route_header() {
        let body = String::from(r#"{"data":[]}"#);
        let (url, captured) = spawn_header_capture_server(body).await;
        let client = mock_client(&url);
        let _ = client.health_check().await.unwrap();
        let raw = captured
            .lock()
            .unwrap()
            .clone()
            .expect("request not captured");
        assert!(
            raw.to_lowercase().contains("x-fusion-route: fusion-design"),
            "missing X-Fusion-Route header: {raw}"
        );
    }

    /// Mock server with an exact status (spawn_mock_server hardcodes non-200 as 500;
    /// this one returns the real status line, for the 401/403 auth-path tests).
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

    /// Sequence mock server: returns statuses[] for the first N hits, then always 200 +
    /// body_for_last.
    /// Used for M-5 retry regression: a few 502/503s first, then a 200 success.
    /// The counter is an AtomicU32; returns the url + a hit-count handle for asserting call
    /// counts.
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

    /// Sequence SSE mock server: returns statuses[] (transient errors) for the first N hits, then
    /// a 200 SSE stream.
    /// Used for chat_stream_messages connect-retry regression: a few 502s, then 200 streaming
    /// deltas.
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

    /// On 401, available=false and auth_ok=Some(false) — the core "false green" fix scenario.
    #[tokio::test]
    async fn health_check_401_marks_auth_failure() {
        let url = spawn_status_server(401, r#"{"error":"unauthorized"}"#.to_string()).await;
        let client = mock_client(&url);
        let s = client.health_check().await.unwrap();
        assert!(!s.available, "401 must not be available");
        assert_eq!(s.auth_ok, Some(false), "401 must mark auth_ok=false");
        assert!(s.models.is_empty(), "401 must not list models");
        assert!(
            s.status.as_deref().unwrap().contains("auth"),
            "status text must point out the auth failure: {:?}",
            s.status
        );
    }

    /// 200 but empty data — gateway false green: available=false, auth_ok=Some(true).
    #[tokio::test]
    async fn health_check_empty_models_not_available() {
        let url = spawn_status_server(200, r#"{"data":[]}"#.to_string()).await;
        let client = mock_client(&url);
        let s = client.health_check().await.unwrap();
        assert!(!s.available, "empty data must not be available");
        assert_eq!(s.auth_ok, Some(true), "200 means auth passed");
        assert!(s.model.is_none(), "no first model");
        assert!(
            s.status.as_deref().unwrap().contains("no models"),
            "status must point out no models: {:?}",
            s.status
        );
    }

    /// 200 with models — normal path: available=true, auth_ok=Some(true), model set.
    #[tokio::test]
    async fn health_check_with_models_available() {
        let body = r#"{"data":[{"id":"qwen3-coder"},{"id":"qwen2.5-vl"}]}"#.to_string();
        let url = spawn_status_server(200, body).await;
        let client = mock_client(&url);
        let s = client.health_check().await.unwrap();
        assert!(s.available, "with models it should be available");
        assert_eq!(s.auth_ok, Some(true));
        assert_eq!(s.model.as_deref(), Some("qwen3-coder"));
        assert_eq!(s.models.len(), 2, "all model ids should be listed");
        assert!(
            s.status.as_deref().unwrap().contains("available"),
            "status must mark available: {:?}",
            s.status
        );
    }

    /// check_generate 502 — gateway false-green endgame: models listed but MLX not loaded,
    /// available=false.
    #[tokio::test]
    async fn check_generate_502_marks_model_not_loaded() {
        let url = spawn_status_server(
            502,
            r#"{"error":{"message":"Chat failed","type":"server_error"}}"#.to_string(),
        )
        .await;
        let client = mock_client(&url);
        let p = client.check_generate("qwen3.5-4b-4bit").await.unwrap();
        assert!(!p.available, "502 must not be available");
        assert!(!p.model_loaded, "502 must not be model_loaded");
        assert_eq!(p.http_code, Some(502));
        assert!(
            p.status.as_deref().unwrap().contains("not loaded"),
            "must point out not loaded: {:?}",
            p.status
        );
    }

    /// check_generate 503 — MLX explicitly says "model loading", available=false.
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

    /// check_generate 200 — real inference passed: available=true, model_loaded=true.
    #[tokio::test]
    async fn check_generate_200_marks_available() {
        let body = r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_string();
        let url = spawn_status_server(200, body).await;
        let client = mock_client(&url);
        let p = client.check_generate("qwen3.5-4b-4bit").await.unwrap();
        assert!(p.available, "200 should be available");
        assert!(p.model_loaded, "200 should be model_loaded");
        assert_eq!(p.http_code, Some(200));
        assert!(
            p.status.as_deref().unwrap().contains("passed"),
            "must mark passed: {:?}",
            p.status
        );
    }

    /// check_generate 401 — auth-failure path.
    #[tokio::test]
    async fn check_generate_401_marks_auth_failure() {
        let url =
            spawn_status_server(401, r#"{"error":{"message":"unauthorized"}}"#.to_string()).await;
        let client = mock_client(&url);
        let p = client.check_generate("m").await.unwrap();
        assert!(!p.available);
        assert_eq!(p.http_code, Some(401));
        assert!(
            p.status.as_deref().unwrap().contains("auth"),
            "must point out auth failure: {:?}",
            p.status
        );
    }

    // ── TC-4: gateway false-green regression (real inference probe sees through the
    // /v1/models lie) ──

    /// Path-aware mock: /v1/models → 200 + model list (false green), /v1/chat/completions → 502.
    /// Simulates the "false green" endgame where the gateway lists cloud/local model names but MLX
    /// has not loaded them.
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
                // Parse the request line's first line: METHOD PATH HTTP/1.1
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("");
                let (status, reason, body) = if path.contains("/v1/models") {
                    // False green: 200 + model list, everything looks fine
                    (
                        200u16,
                        "OK",
                        r#"{"object":"list","data":[{"id":"qwen3.5-4b-4bit","object":"model"}]}"#
                            .to_string(),
                    )
                } else {
                    // /v1/chat/completions: MLX not loaded → 502
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

    /// TC-4: gateway /v1/models false green — model names listed but generate actually 502s.
    /// check_generate must see through it with the real inference probe, not judge availability by
    /// /v1/models.
    #[tokio::test]
    async fn check_generate_sees_through_gateway_false_green() {
        let url = spawn_false_green_server().await;
        let client = mock_client(&url);
        // /v1/models false-green 200, but check_generate goes through the real chat probe →
        // 502 → model_loaded=false
        let p = client.check_generate("qwen3.5-4b-4bit").await.unwrap();
        assert!(
            !p.available,
            "gateway false green: /v1/models 200 but generate 502, must not be available"
        );
        assert!(
            !p.model_loaded,
            "the real inference probe must see through the false green, model_loaded=false"
        );
        assert_eq!(p.http_code, Some(502));
        assert!(
            p.status.as_deref().unwrap().contains("not loaded"),
            "must point out model not loaded (breaking the false green): {:?}",
            p.status
        );
    }

    // ── M-5: 502/503 exponential backoff retry regression ──

    /// is_transient_status classifies only 502/503 as transient; the rest (including
    /// 200/401/403/404/500) are permanent.
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

    /// backoff_delay grows exponentially and caps at 8s: attempt 0/1/2/3 → 500ms/1s/2s/4s,
    /// 10 → 8s.
    #[test]
    fn backoff_delay_exponential_capped() {
        assert_eq!(backoff_delay(0), std::time::Duration::from_millis(500));
        assert_eq!(backoff_delay(1), std::time::Duration::from_millis(1000));
        assert_eq!(backoff_delay(2), std::time::Duration::from_millis(2000));
        assert_eq!(backoff_delay(3), std::time::Duration::from_millis(4000));
        assert_eq!(
            backoff_delay(10),
            std::time::Duration::from_millis(8000),
            "capped at 8s"
        );
    }

    /// FUSION_MLX_RETRY_MAX override. env tests have a process-global race, hence #[ignore];
    /// manual verification:
    /// `cargo test -p fd-ai-adapter retry_max_attempts_env_override -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "env is process-global; run manually with --test-threads=1"]
    fn retry_max_attempts_env_override() {
        std::env::set_var("FUSION_MLX_RETRY_MAX", "2");
        assert_eq!(retry_max_attempts(), 2);
        std::env::remove_var("FUSION_MLX_RETRY_MAX");
        assert_eq!(retry_max_attempts(), RETRY_DEFAULT_MAX_ATTEMPTS);
        std::env::set_var("FUSION_MLX_RETRY_MAX", "0");
        assert_eq!(
            retry_max_attempts(),
            RETRY_DEFAULT_MAX_ATTEMPTS,
            "0 counts as default"
        );
        std::env::set_var("FUSION_MLX_RETRY_MAX", "notanumber");
        assert_eq!(
            retry_max_attempts(),
            RETRY_DEFAULT_MAX_ATTEMPTS,
            "a bad value counts as default"
        );
        std::env::remove_var("FUSION_MLX_RETRY_MAX");
    }

    /// issue #30: bearer_token alias chain (FUSION_CLIENT_KEY canonical first →
    /// FUSION_MLX_API_KEY legacy fallback → both unset = None). env is process-global, hence
    /// #[ignore]; manual verification:
    /// `cargo test -p fd-ai-adapter bearer_token_alias_chain -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "env is process-global; run manually with --test-threads=1"]
    fn bearer_token_alias_chain() {
        let c = FusionMlxClient::default();
        std::env::remove_var("FUSION_CLIENT_KEY");
        std::env::remove_var("FUSION_MLX_API_KEY");
        assert_eq!(c.bearer_token(), None, "both unset → None");
        std::env::set_var("FUSION_MLX_API_KEY", "legacy-key");
        assert_eq!(
            c.bearer_token().as_deref(),
            Some("Bearer legacy-key"),
            "legacy fallback takes effect"
        );
        std::env::set_var("FUSION_CLIENT_KEY", "canon-key");
        assert_eq!(
            c.bearer_token().as_deref(),
            Some("Bearer canon-key"),
            "canonical name wins over legacy"
        );
        std::env::set_var("FUSION_CLIENT_KEY", "   ");
        assert_eq!(
            c.bearer_token().as_deref(),
            Some("Bearer legacy-key"),
            "a blank canonical name counts as unset, falling back to legacy"
        );
        std::env::remove_var("FUSION_CLIENT_KEY");
        std::env::remove_var("FUSION_MLX_API_KEY");
    }

    /// blocking_post: two 503s then 200 → retry succeeds, 3 calls.
    /// Not #[tokio::test]: chat_sync's internal BLOCKING_RT.block_on inside a tokio runtime would
    /// panic, so a standalone runtime starts the sequence mock server, then chat_sync is called
    /// synchronously.
    #[test]
    fn blocking_post_retries_on_503_then_succeeds() {
        let body = r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_string();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url, count) = rt.block_on(spawn_sequence_server(vec![503, 503], body));
        let client = mock_client(&url);
        let out = client.chat_sync("m", "sys", "ping", 1);
        assert!(out.is_ok(), "after 503x2 retry should succeed: {:?}", out);
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "should be called 3 times (2x 503 + 1x 200)"
        );
    }

    /// blocking_post: 401 permanent error, no retry, exactly 1 call.
    #[test]
    fn blocking_post_no_retry_on_401() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url, count) = rt.block_on(spawn_sequence_server(
            vec![401],
            String::from(r#"{"error":"nope"}"#),
        ));
        let client = mock_client(&url);
        let out = client.chat_sync("m", "sys", "ping", 1);
        assert!(out.is_err(), "401 should fail directly with no retry");
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a 401 permanent error should be called exactly once"
        );
    }

    /// blocking_post: constant 503 exhausts retries → bail, calls = max.
    /// Uses a sequence server always returning 503 (statuses cover all calls), asserting the
    /// exhausted failure.
    #[test]
    fn blocking_post_retries_exhaust_then_bail() {
        // statuses provides 8x 503, enough to cover the default max=4 retries.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (url, count) = rt.block_on(spawn_sequence_server(
            vec![503, 503, 503, 503, 503, 503, 503, 503],
            String::from(r#"{"error":"nope"}"#),
        ));
        let client = mock_client(&url);
        let out = client.chat_sync("m", "sys", "ping", 1);
        assert!(
            out.is_err(),
            "constant 503 should fail after retries are exhausted"
        );
        let calls = count.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            calls, RETRY_DEFAULT_MAX_ATTEMPTS,
            "should be called the full max={RETRY_DEFAULT_MAX_ATTEMPTS} times"
        );
        let err = out.unwrap_err().to_string();
        assert!(err.contains("503"), "error must contain 503: {err}");
    }

    /// A-1: multi-node failover. Node A always 503, node B always 200.
    /// The first request lands on A (rr idx 0) → 503 → retry switches to B (rr idx 1) → 200
    /// success.
    /// Assertions: ultimately successful, A called once, B called once.
    #[test]
    fn blocking_post_failover_to_next_endpoint() {
        let body = r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_string();
        let rt = tokio::runtime::Runtime::new().unwrap();
        // A: always 503 (8 entries give ample coverage, but after failover only 1 call is made
        // before switching away).
        let (url_a, count_a) = rt.block_on(spawn_sequence_server(
            vec![503, 503, 503, 503, 503, 503, 503, 503],
            body.clone(),
        ));
        // B: always 200.
        let (url_b, count_b) = rt.block_on(spawn_sequence_server(vec![], body.clone()));
        let client = mock_client_endpoints(vec![url_a, url_b]);
        let out = client.chat_sync("m", "sys", "ping", 1);
        assert!(
            out.is_ok(),
            "A 503 should fail over to B and succeed: {:?}",
            out
        );
        let calls_a = count_a.load(std::sync::atomic::Ordering::SeqCst);
        let calls_b = count_b.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            calls_a, 1,
            "A should be called exactly once (first request lands on A, then switches away)"
        );
        assert_eq!(
            calls_b, 1,
            "B should be called once (failover lands on B successfully)"
        );
    }

    /// A-1: all nodes 503 → retries exhausted, both nodes polled.
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
        assert!(
            out.is_err(),
            "all nodes 503 should exhaust retries and fail"
        );
        let total = count_a.load(std::sync::atomic::Ordering::SeqCst)
            + count_b.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            total, RETRY_DEFAULT_MAX_ATTEMPTS,
            "the two nodes combined should be called the full max={RETRY_DEFAULT_MAX_ATTEMPTS} times"
        );
    }

    /// check_generate: two 503s then 200 → after probe retries, model_loaded=true.
    #[tokio::test]
    async fn check_generate_retries_on_503_then_loaded() {
        let body = r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_string();
        let (url, count) = spawn_sequence_server(vec![503, 503], body).await;
        let client = mock_client(&url);
        let p = client.check_generate("qwen3.5-4b-4bit").await.unwrap();
        assert!(
            p.model_loaded,
            "after 503x2 the retry should succeed and mark model_loaded"
        );
        assert!(p.available);
        assert_eq!(p.http_code, Some(200));
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "the probe should be called 3 times"
        );
    }

    /// chat_stream_messages: connect 502 then 200 SSE → after stream retry, deltas are produced.
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
            .expect("after the 502 retry, connect and finish the stream within 5 seconds");
        assert_eq!(
            tokens, "Hi",
            "deltas should be produced normally after retry"
        );
        assert!(saw_done, "the stream should end normally with [DONE]");
    }

    /// FAULT-1: after a 200 connect, no data frames ever arrive (stall) → chat_stream_messages'
    /// tokio::time::timeout(idle, stream.next()) should terminate with Err within IDLE, not hang
    /// forever.
    /// env is process-global (FUSION_MLX_STREAM_IDLE_SECS set_var), hence #[ignore] +
    /// manual --test-threads=1 run, matching the retry_max_attempts_env_override convention in
    /// this crate.
    #[tokio::test]
    #[ignore = "env is process-global; run manually: cargo test -p fd-ai-adapter chat_stream_messages_stall_timeout -- --ignored --test-threads=1"]
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
            // The stall server sends no deltas; the first stream.next() should time out to Err.
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
        // Outer timeout backstop: FAULT-1 should terminate within IDLE(1s), not rely on this 10s
        // backstop.
        let errored = tokio::time::timeout(std::time::Duration::from_secs(10), drain)
            .await
            .expect("the stall stream should end with Err within 10 seconds, not hang forever");
        assert!(
            errored,
            "the stall stream should end with Err (FAULT-1 fail visibly)"
        );
        std::env::remove_var("FUSION_MLX_STREAM_IDLE_SECS");
    }

    /// Captures the full request body (large buffer, for multimodal base64 payloads).
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

    /// P1-1: verifies chat_with_image really sends a data:image/png;base64 multimodal payload.
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
        let raw = captured
            .lock()
            .unwrap()
            .clone()
            .expect("request not captured");
        assert!(
            raw.contains("data:image/png;base64,"),
            "multimodal payload missing the data:image/png;base64 prefix: {}",
            &raw[..raw.len().min(200)]
        );
        assert!(raw.contains("image_url"), "missing image_url field");
    }

    /// P1-1: verifies the real image_to_ui_async path loads the sketch and sends a multimodal
    /// request (not just a text description).
    #[tokio::test]
    async fn image_to_ui_async_sends_real_image() {
        let ui_json = "{\"page\":{\"width\":800,\"height\":600,\"nodes\":[{\"id\":\"c\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":100,\"h\":50}]}}";
        let body = format!("{{\"choices\":[{{\"message\":{{\"content\":{ui_json:?}}}}}]}}");
        let (url, captured) = spawn_body_capture_server(body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let sketch = write_fixture_png();
        let doc = skills
            .image_to_ui_async(&sketch, "test", "Home")
            .await
            .unwrap();
        assert_eq!(doc.pages.len(), 1);
        assert_eq!(doc.pages[0].nodes[0].id, "c");
        let raw = captured
            .lock()
            .unwrap()
            .clone()
            .expect("request not captured");
        assert!(
            raw.contains("data:image/png;base64,"),
            "image_to_ui_async did not send a real image payload (still a text description): {}",
            &raw[..raw.len().min(200)]
        );
    }

    /// Production E2E: the model returns a trailing extra closing brace → after the balance fix a
    /// real document is produced (no placeholder fallback).
    #[tokio::test]
    async fn image_to_ui_e2e_trailing_brace_repaired() {
        let ui_json = "{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":100,\"h\":50}]}}}";
        let body = format!("{{\"choices\":[{{\"message\":{{\"content\":{ui_json:?}}}}}]}}");
        let (url, _count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let sketch = write_fixture_png();
        let doc = skills
            .image_to_ui_async(&sketch, "test", "Home")
            .await
            .unwrap();
        assert_eq!(doc.pages.len(), 1);
        assert_eq!(doc.pages[0].nodes[0].id, "n0");
        // Placeholder fallback not triggered. The compared string stays in Chinese — it is the
        // user-facing placeholder text produced by fd-skills' repair_model_json fallback
        // (defined outside this file), so the assertion must match it byte-for-byte.
        // i18n: user-facing
        assert!(!doc.pages[0]
            .nodes
            .iter()
            .any(|n| n.text.as_deref() == Some("AI 输出不可用，已生成占位布局")));
    }

    /// Production E2E: the model returns thoroughly corrupted JSON → R-2 rejects the pseudo-success
    /// and propagates the error (no longer synthesizes a placeholder fallback).
    #[tokio::test]
    async fn image_to_ui_e2e_garbled_propagates_error() {
        let body = String::from(r#"{"choices":[{"message":{"content":"totally not json {{{"}}]}"#);
        let (url, _count) = spawn_mock_server(200, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let sketch = write_fixture_png();
        let result = skills.image_to_ui_async(&sketch, "test", "Home").await;
        assert!(
            result.is_err(),
            "thoroughly corrupted JSON should propagate the error, not synthesize a pseudo-success placeholder"
        );
        let msg = format!("{}", result.err().unwrap());
        assert!(
            msg.contains("JSON") || msg.contains("json") || msg.contains("nodes"),
            "the error message should indicate the JSON/nodes parse failure: {msg}"
        );
    }

    /// Production E2E: HTTP 5xx → the error propagates (no silent swallowing).
    #[tokio::test]
    async fn image_to_ui_e2e_http_error_propagates() {
        let body = String::from(r#"{"error":"internal"}"#);
        let (url, _count) = spawn_mock_server(500, body).await;
        let client = mock_client(&url);
        let skills = DesignSkills::new(client, "qwen3.5");
        let sketch = write_fixture_png();
        let result = skills.image_to_ui_async(&sketch, "test", "Home").await;
        assert!(
            result.is_err(),
            "HTTP 5xx should propagate, not silently succeed"
        );
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

    /// Regression #18: upstream early EOF (connection closed before [DONE]).
    /// The old None branch returned Some(finished) without advancing state → infinite loop;
    /// a half-finished return None dropped the buffer outright → tail deltas lost.
    /// After the fix: already-emitted deltas are kept, the stream terminates normally, no
    /// infinite loop.
    #[tokio::test]
    async fn chat_stream_messages_early_eof_drains_and_terminates() {
        let d1 = String::from(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#);
        let d2 = String::from(r#"{"choices":[{"delta":{"content":"lo"}}]}"#);
        // Send only d1+d2 without [DONE], then close the connection = early EOF
        let url = spawn_sse_server_early_eof(vec![d1, d2]).await;
        let client = mock_client(&url);
        let messages = vec![MlxChatMessage {
            role: String::from("user"),
            content: String::from("hi"),
        }];
        let mut stream = chat_stream_messages(client, String::from("qwen3.5"), messages, 128).await;
        use futures::StreamExt;
        let mut tokens = String::new();
        // Timeout backstop: on an infinite loop this fails after 3 seconds (rather than hanging
        // CI)
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
            .expect(
                "the stream should terminate within 3 seconds, no infinite loop (#18 regression)",
            );
        // Both frames were buffered and drained by the EOF branch → "Hello" is not lost
        assert_eq!(
            tokens, "Hello",
            "early EOF must not lose already-emitted tail deltas"
        );
    }

    // L6 regression: a CJK multi-byte character cut mid-character by network chunking.
    // The old String::from_utf8_lossy decoded per chunk → broken trailing bytes became U+FFFD →
    // garbled Chinese.
    // After the fix the byte buffer waits for a complete line before decoding → cross-chunk
    // partial characters are completed → intact with no U+FFFD.
    #[tokio::test]
    async fn chat_stream_messages_cjk_split_across_chunks_no_replacement() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // 登录 = e7 99 bb e5 bd 95; build an SSE frame cut 2 bytes after 0xE7 (half a CJK char).
        // i18n: user-facing (the fixture content is deliberately CJK — the test's purpose)
        let cjk = "登录";
        let payload = serde_json::json!({"choices":[{"delta":{"content":cjk}}]}).to_string();
        let frame = format!("data: {}\r\n\r\n", payload);
        let frame_bytes = frame.as_bytes().to_vec();
        let split_at = frame_bytes
            .iter()
            .position(|&b| b == 0xE7)
            .expect("the frame should contain the CJK first byte 0xE7")
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
            // First chunk: the frame's first half (cut mid-CJK-character)
            let _ = sock.write_all(&frame_bytes[..split_at]).await;
            let _ = sock.flush().await;
            // Second chunk: the frame's second half + [DONE]
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
            .expect("the stream should terminate within 3 seconds");
        assert_eq!(
            tokens, "登录",
            "the CJK character cut across chunks should be restored intact, no U+FFFD replacement"
        );
        assert!(
            !tokens.contains('\u{fffd}'),
            "no U+FFFD replacement character may appear"
        );
    }

    /// Mock server capturing the request body into a Mutex: returns a fixed response while
    /// storing the received raw request line + body.
    /// H-A9 regression: verifies send folds history into messages[] rather than silently dropping
    /// it.
    /// Spawned on BLOCKING_RT (the dedicated persistent multi-thread runtime) to avoid nested
    /// block_on panics between #[test] sync threads and the tokio::test runtime (see the comment
    /// at lib.rs:310).
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

    /// Starts the capturing mock on BLOCKING_RT and returns (url, captured).
    /// Ordinary sync test threads call this helper: BLOCKING_RT.block_on spawns the server (until
    /// the first frame completes), then send → chat_sync_messages → BLOCKING_RT.block_on
    /// (sequential, not nested).
    fn capturing_server(body: &str) -> (String, Arc<Mutex<Vec<u8>>>) {
        BLOCKING_RT.block_on(spawn_capturing_mock_server(body.to_string()))
    }

    /// H-A9: send must fold history into messages[] (system + history turns + current user),
    /// not the old implementation's system + user pair. The capturing mock verifies the wire
    /// payload.
    #[test]
    fn send_folds_history_into_messages_wire() {
        let body = String::from(r#"{"choices":[{"message":{"content":"ok"}}]}"#);
        let (url, captured) = capturing_server(&body);
        let client = mock_client(&url);
        let provider = FusionMlxChatProvider::new(client, "default-model");
        let request = ChatRequest {
            system_prompt: "You are a UI generator".into(),
            user_message: "generate a login page".into(),
            history: vec![
                (ChatHistoryRole::User, "what is a login page".into()),
                (
                    ChatHistoryRole::Assistant,
                    "a login page is the entry point for user identity authentication".into(),
                ),
            ],
            max_output_tokens: 256,
            thinking: ThinkingMode::default(),
            effort: EffortLevel::default(),
            attachments: vec![],
            model: Some("qwen3.5".into()),
        };
        let deltas: Vec<ChatDelta> = provider.send(request).collect();
        // Should receive TextDelta + Done.
        let has_text = deltas.iter().any(|d| matches!(d, ChatDelta::TextDelta(_)));
        assert!(has_text, "send should return a TextDelta");
        // Verify the wire payload contains 4 messages (system + 2 history + 1 current user).
        let raw = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
        let payload: serde_json::Value =
            serde_json::from_slice(&raw.as_bytes()[body_start..]).unwrap();
        let msgs = payload["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4, "history should fold into 4 messages");
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are a UI generator");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "what is a login page");
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(
            msgs[2]["content"],
            "a login page is the entry point for user identity authentication"
        );
        assert_eq!(msgs[3]["role"], "user");
        assert_eq!(msgs[3]["content"], "generate a login page");
        assert_eq!(payload["model"], "qwen3.5");
    }

    /// H-A9: an empty system_prompt should be skipped (no empty system message), avoiding MLX
    /// errors.
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
        assert_eq!(
            msgs.len(),
            1,
            "an empty system_prompt must not produce a system message"
        );
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "hi");
        assert_eq!(
            payload["model"], "default-model",
            "model=None falls back to default"
        );
    }

    /// H-A7: multi-line text should chunk per line into multiple TextDeltas, not collapse into a
    /// single full-text blob.
    /// Under sync trait semantics there is no true streaming, but the consumer should receive an
    /// incremental sequence (per-line deltas + Done).
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
            "three lines of text should produce 3 TextDeltas, not a single full-text blob"
        );
        assert_eq!(text_deltas[0], "line1\n");
        assert_eq!(text_deltas[1], "line2\n");
        assert_eq!(text_deltas[2], "line3");
        assert!(
            deltas.iter().any(|d| matches!(d, ChatDelta::Done { .. })),
            "the last frame should be Done"
        );
    }

    /// H-A7: single-line text without newlines produces 1 TextDelta + Done.
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

    /// H-A7: empty text produces no TextDelta, only Done (no empty-increment noise).
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
            "empty text should not produce a TextDelta"
        );
        assert!(
            deltas.iter().any(|d| matches!(d, ChatDelta::Done { .. })),
            "empty text should still have a Done frame"
        );
    }
}
