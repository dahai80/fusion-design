//! API handlers: generate / iterate / screenshot restore / version / export /
//! health check / design systems.
//!
//! All model calls go through fd-ai-adapter (gateway 11432 by default,
//! FUSION_MLX_BASE_URL override, FUSION_CLIENT_KEY alias-chain auth). Request
//! body cap REQUEST_CAP guards against OOM.

use std::sync::Arc;

use serde::Deserialize;

use fd_codegen::Codegen;

use crate::state::VersionEntry;
use crate::AppState;

/// Request body cap (bytes). Screenshot base64 can reach several MB; 8MB matches
/// the IPC message guardrail.
const REQUEST_CAP: usize = 8 * 1024 * 1024;

/// Model generate/iterate attempt count (first try + 1 retry). The 4B model
/// occasionally truncates JSON; retrying significantly reduces the failure rate.
const GEN_ATTEMPTS: usize = 2;

/// Vision restore default model: must be a vision-capable model.
const VISION_MODEL_DEFAULT: &str = "mlx-community--Qwen2.5-VL-7B-Instruct-4bit";

/// Resolves the request model id: explicit > FUSION_MLX_MODEL > built-in default
/// (locally loaded 4B text model).
fn resolve_model(explicit: &Option<String>) -> String {
    if let Some(m) = explicit {
        if !m.trim().is_empty() {
            return m.trim().to_string();
        }
    }
    match std::env::var("FUSION_MLX_MODEL") {
        Ok(m) if !m.trim().is_empty() => m.trim().to_string(),
        _ => "mlx-community--Qwen3.5-4B-MLX-4bit".to_string(),
    }
}

/// Builds the model client (endpoint resolution chain reuses fd-ai-adapter:
/// env > gateway default).
fn build_client() -> anyhow::Result<fd_ai_adapter::FusionMlxClient> {
    fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint("")?,
    )
}

/// PenDocument serialization (same format as the .fusiondesign file).
fn doc_json(doc: &fd_canvas_core::PenDocument) -> anyhow::Result<String> {
    Ok(serde_json::to_string(doc)?)
}

fn doc_from_json(json: &str) -> anyhow::Result<fd_canvas_core::PenDocument> {
    serde_json::from_str(json).map_err(|e| anyhow::anyhow!("PenDocument deserialization failed: {e}"))
}

/// Unified JSON response wrapper.
fn ok_json(value: serde_json::Value) -> hyper::Response<http_body_util::Full<hyper::body::Bytes>> {
    let body = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    json_response(hyper::StatusCode::OK, &body)
}

fn err_json(
    status: hyper::StatusCode,
    message: &str,
) -> hyper::Response<http_body_util::Full<hyper::body::Bytes>> {
    let body = serde_json::json!({"error": message}).to_string();
    json_response(status, &body)
}

/// Field-targeted error (issue#35 contract item): 4xx validation failures must
/// name the missing/invalid field so consumers don't have to reverse-engineer it
/// per endpoint. Response shape {"error", "field"}.
fn err_field(
    status: hyper::StatusCode,
    field: &str,
    message: &str,
) -> hyper::Response<http_body_util::Full<hyper::body::Bytes>> {
    let body = serde_json::json!({"error": message, "field": field}).to_string();
    json_response(status, &body)
}

fn json_response(
    status: hyper::StatusCode,
    body: &str,
) -> hyper::Response<http_body_util::Full<hyper::body::Bytes>> {
    hyper::Response::builder()
        .status(status)
        .header(
            hyper::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )
        .body(http_body_util::Full::new(hyper::body::Bytes::from(
            body.to_string(),
        )))
        .unwrap_or_else(|_| {
            hyper::Response::new(http_body_util::Full::new(hyper::body::Bytes::from(
                "{\"error\":\"internal\"}".to_string(),
            )))
        })
}

type Resp = hyper::Response<http_body_util::Full<hyper::body::Bytes>>;

pub async fn health(state: &Arc<AppState>, deep: bool) -> Resp {
    let session_count = state.sessions.list_ids().len();
    // Deep probe via ?deep=1: /v1/models auth + listing (async health_check,
    // 3s timeout). Must not use health_check_sync: the sync blocking client
    // carries its own internal runtime, and calling it inside an async handler
    // deadlocks (async context exclusivity).
    let mlx = if deep {
        match build_client() {
            Ok(c) => Some(c.health_check().await),
            Err(_) => None,
        }
    } else {
        None
    };
    let mlx_json = match mlx {
        None => serde_json::json!({"probed": false}),
        Some(Ok(h)) => serde_json::json!({
            "probed": true, "available": h.available, "model": h.model,
            "chat_model": resolve_model(&None),
            "auth_ok": h.auth_ok, "models": h.models, "status": h.status,
        }),
        Some(Err(e)) => {
            serde_json::json!({"probed": true, "available": false, "error": e.to_string()})
        }
    };
    ok_json(serde_json::json!({
        "service": "fusion-design-server",
        "version": env!("CARGO_PKG_VERSION"),
        "sessions": session_count,
        "mlx": mlx_json,
    }))
}

pub fn design_systems(state: &Arc<AppState>) -> Resp {
    let reg = state.registry.lock().unwrap();
    ok_json(serde_json::json!({
        "systems": reg.list(),
        "active": reg.active().map(|d| d.id.clone()),
        "theme": format!("{:?}", reg.current_theme()),
    }))
}

/// Design-system token CSS (consumed by canvas ApplyTokens; theme=light|dark).
// issue#44: design-intelligence dataset in-process singleton (embedded CSV, read-only).
static DESIGN_INTEL: std::sync::OnceLock<fd_search::DesignIntel> = std::sync::OnceLock::new();

/// GET /api/design-intel/search?q=&kind=&limit= (issue #44).
/// Dataset search: deterministic keyword scoring, zero HTTP, zero model calls.
pub fn design_intel_search(q: &str, kind: &str, limit: usize) -> Resp {
    let q = q.trim();
    if q.is_empty() {
        return err_field(
            hyper::StatusCode::BAD_REQUEST,
            "q",
            "q is required (non-empty free-text query)",
        );
    }
    let limit = limit.clamp(1, 50);
    let intel = DESIGN_INTEL.get_or_init(fd_search::DesignIntel::embedded);
    match intel.search_kind(kind, q, limit) {
        Some(hits) => ok_json(serde_json::json!({
            "kind": kind,
            "q": q,
            "count": hits.len(),
            "hits": hits,
        })),
        None => err_field(
            hyper::StatusCode::BAD_REQUEST,
            "kind",
            &format!(
                "unknown kind: {kind} (valid: {})",
                fd_search::KINDS.to_vec().join(" | ") + " | all"
            ),
        ),
    }
}

pub fn tokens(state: &Arc<AppState>, ds: &str, theme: &str) -> Resp {
    let mut reg = state.registry.lock().unwrap();
    let theme = match theme {
        "dark" => fd_design_system::Theme::Dark,
        _ => fd_design_system::Theme::Light,
    };
    reg.activate_theme(theme);
    match reg.get(ds) {
        Some(system) => {
            // Issue #42: token panel needs name/value pairs (CSS vars replace
            // '.' with '-'; the panel wants raw token names). Merged view =
            // light base + dark overlay values for the requested theme.
            let overlay: std::collections::HashMap<&str, &fd_design_system::Token> = system
                .dark_tokens
                .as_ref()
                .filter(|_| theme == fd_design_system::Theme::Dark)
                .map(|d| d.iter().map(|t| (t.name.as_str(), t)).collect())
                .unwrap_or_default();
            let tokens: Vec<serde_json::Value> = system
                .tokens
                .iter()
                .map(|t| {
                    let value = overlay
                        .get(t.name.as_str())
                        .map(|d| d.value.to_css_value())
                        .unwrap_or_else(|| t.value.to_css_value());
                    serde_json::json!({"name": t.name, "value": value})
                })
                .collect();
            ok_json(serde_json::json!({
                "design_system": ds,
                "theme": theme,
                "css": system.to_css_custom_properties_for_theme(theme),
                "tokens": tokens,
            }))
        }
        None => err_field(
            hyper::StatusCode::NOT_FOUND,
            "ds",
            &format!("design system {ds} does not exist"),
        ),
    }
}

#[derive(Deserialize)]
pub struct GenerateRequest {
    pub prompt: String,
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub design_system: Option<String>,
}

pub async fn generate(state: &Arc<AppState>, body: &[u8]) -> Resp {
    let req: GenerateRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return err_field(
                hyper::StatusCode::BAD_REQUEST,
                "body",
                &format!("failed to parse request body: {e}"),
            )
        }
    };
    if req.prompt.trim().is_empty() {
        return err_field(
            hyper::StatusCode::BAD_REQUEST,
            "prompt",
            "prompt must not be empty (non-empty string)",
        );
    }
    let page = req.page.unwrap_or_else(|| "page-1".to_string());
    let model = resolve_model(&req.model);
    tracing::info!(page = %page, model = %model, "api/generate: text-to-UI");

    // issue#44: design-intel context injection — search the embedded dataset
    // (style/palette/typography/UX guidelines/diagrams) by the user prompt and
    // append a structured suggestion block on hit. Deterministic retrieval, no
    // model, no network.
    let base_prompt = match DESIGN_INTEL
        .get_or_init(fd_search::DesignIntel::embedded)
        .recommend(&req.prompt, 3)
    {
        Some(block) => {
            tracing::info!("api/generate: design-intel context injected");
            format!("{}\n{block}", req.prompt)
        }
        None => req.prompt.clone(),
    };

    let client = match build_client() {
        Ok(c) => c,
        Err(e) => {
            return err_json(
                hyper::StatusCode::BAD_GATEWAY,
                &format!("failed to build model client: {e}"),
            )
        }
    };
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    // The 4B model occasionally truncates JSON (beyond what the parse_ui_json
    // repair chain can fix); retry once before declaring failure.
    let mut doc = None;
    let mut last_err = String::new();
    for attempt in 1..=GEN_ATTEMPTS {
        // Retries must vary the prompt: the gateway caches identical prompts, so
        // retrying the same string would only return the same bad output.
        let prompt = if attempt == 1 {
            base_prompt.clone()
        } else {
            format!(
                "{}\n(attempt {attempt}, output valid complete JSON strictly)",
                base_prompt
            )
        };
        match skills.text_to_ui_async(&prompt, &page).await {
            Ok(d) => {
                doc = Some(d);
                break;
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "api/generate: generation failed, retrying");
                last_err = e.to_string();
            }
        }
    }
    let Some(mut doc) = doc else {
        tracing::error!(error = %last_err, "api/generate: generation failed (including retries)");
        return err_json(
            hyper::StatusCode::BAD_GATEWAY,
            &format!("generation failed: {last_err}"),
        );
    };
    if let Some(ds) = req.design_system.as_deref() {
        let reg = state.registry.lock().unwrap();
        if reg.get(ds).is_some() {
            doc.active_design_system = Some(ds.to_string());
        } else {
            tracing::warn!(design_system = %ds, "api/generate: unknown design system, ignoring");
        }
    }
    match finalize_new_session(state, &doc, &format!("generate: {}", truncate(&req.prompt, 40))) {
        Ok(v) => v,
        Err(e) => err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct IterateRequest {
    pub session: String,
    pub instruction: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub design_system: Option<String>,
    /// Region selection (issue#34): iterate only these node ids; prompt is
    /// O(region) instead of O(doc).
    #[serde(default)]
    pub region: Option<Vec<String>>,
}

/// Iterate: current version document + instruction -> full page rewritten as a
/// new version.
///
/// Goes through the same parse_ui_json path as text_to_ui (schema validated);
/// the prompt carries the current document JSON with instructions to keep
/// unmentioned content unchanged. 4B model full-page rewrite capped at 4096
/// tokens (same as DEFAULT_MAX_TOKENS).
pub async fn iterate(state: &Arc<AppState>, body: &[u8]) -> Resp {
    let req: IterateRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return err_field(
                hyper::StatusCode::BAD_REQUEST,
                "body",
                &format!("failed to parse request body: {e}"),
            )
        }
    };
    if req.instruction.trim().is_empty() {
        return err_field(
            hyper::StatusCode::BAD_REQUEST,
            "instruction",
            "instruction must not be empty (non-empty string)",
        );
    }
    let Some(sess) = state.sessions.get(&req.session) else {
        return err_field(
            hyper::StatusCode::NOT_FOUND,
            "session",
            &format!("session {} does not exist", req.session),
        );
    };
    let Some(cur) = sess.current_doc() else {
        return err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, "session has no current version");
    };
    // Region externalization (issue#34): region given but no match -> 400 (fail
    // visibly, no silent fallback to full document).
    let region: Option<Vec<String>> = req.region.as_ref().map(|ids| {
        ids.iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });
    if let Some(ids) = &region {
        if crate::prompt::select_region(&cur.document, ids).is_none() {
            return err_field(
                hyper::StatusCode::BAD_REQUEST,
                "region",
                &format!("region matched no nodes: {ids:?}"),
            );
        }
    }
    let model = resolve_model(&req.model);
    tracing::info!(
        session = %req.session,
        model = %model,
        region = region.as_ref().map(|r| r.len()).unwrap_or(0),
        "api/iterate: conversational iteration"
    );

    let client = match build_client() {
        Ok(c) => c,
        Err(e) => {
            return err_json(
                hyper::StatusCode::BAD_GATEWAY,
                &format!("failed to build model client: {e}"),
            )
        }
    };
    let sys = crate::prompt::ITERATE_SYS;
    let changelog = crate::prompt::changelog_of(&sess);
    let user_base = crate::prompt::build_iterate_user(
        &cur.document,
        region.as_deref(),
        &changelog,
        &req.instruction,
    );
    let page_name = page_name_of(&cur.document).unwrap_or_else(|| "page-1".to_string());
    // Same as generate: model output is occasionally non-schema JSON; retry the
    // call+parse as a whole once.
    let mut doc = None;
    let mut last_err = String::new();
    for attempt in 1..=GEN_ATTEMPTS {
        let user = if attempt == 1 {
            user_base.clone()
        } else {
            format!("{user_base}\n(attempt {attempt}, output valid complete JSON strictly)")
        };
        let resp = match client.chat_async(&model, sys, &user, 4096).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(attempt, error = %e, "api/iterate: model call failed, retrying");
                last_err = e.to_string();
                continue;
            }
        };
        match fd_ai_adapter::parse_ui_json(&resp, &page_name) {
            Ok(d) => {
                doc = Some(d);
                break;
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "api/iterate: output parse failed, retrying");
                last_err = "model output did not follow the page schema".to_string();
            }
        }
    }
    let Some(mut doc) = doc else {
        tracing::error!(error = %last_err, "api/iterate: iteration failed (including retries)");
        return err_json(
            hyper::StatusCode::BAD_GATEWAY,
            &format!("iteration failed: {last_err}"),
        );
    };
    // Region mode (issue#34): the model returned only region nodes; merge them
    // deterministically back into the full page.
    if let Some(ids) = &region {
        let model_nodes: Vec<serde_json::Value> = doc_json(&doc)
            .ok()
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .and_then(|v| {
                v.get("pages")
                    .and_then(|p| p.get(0))
                    .and_then(|p| p.get("nodes"))
                    .and_then(|n| n.as_array())
                    .cloned()
            })
            .unwrap_or_default();
        let merged = crate::prompt::merge_region_nodes(&cur.document, &model_nodes, ids);
        let Some(merged) = merged else {
            tracing::error!(region = ?ids, "api/iterate: model returned no region nodes, refusing to silently drop nodes");
            return err_json(
                hyper::StatusCode::BAD_GATEWAY,
                "model returned no region node results; retry or drop region to iterate the full page",
            );
        };
        match doc_from_json(&merged) {
            Ok(d) => doc = d,
            Err(e) => {
                tracing::error!(error = %e, "api/iterate: document invalid after region merge");
                return err_json(
                    hyper::StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("region merge failed: {e}"),
                );
            }
        }
    }
    // Iteration preserves the design-system declaration: explicit request > carry
    // over from the current version.
    if let Some(ds) = req.design_system.as_deref() {
        let reg = state.registry.lock().unwrap();
        if reg.get(ds).is_some() {
            doc.active_design_system = Some(ds.to_string());
        }
    } else if doc.active_design_system.is_none() {
        if let Ok(prev) = doc_from_json(&cur.document) {
            doc.active_design_system = prev.active_design_system;
        }
    }
    match push_version(
        state,
        &req.session,
        &doc,
        &format!("iterate: {}", truncate(&req.instruction, 40)),
    ) {
        Ok(v) => v,
        Err(e) => err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct RestoreRequest {
    /// Image base64 (no data: prefix; the frontend strips it).
    pub image_base64: String,
    #[serde(default)]
    pub hint: Option<String>,
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub design_system: Option<String>,
}

/// Screenshot/sketch restore: fusion-mlx multimodal vision input -> PenDocument
/// (issue#31 acceptance item).
pub async fn restore(state: &Arc<AppState>, body: &[u8]) -> Resp {
    let req: RestoreRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return err_field(
                hyper::StatusCode::BAD_REQUEST,
                "body",
                &format!("failed to parse request body: {e}"),
            )
        }
    };
    if req.image_base64.trim().is_empty() {
        return err_field(
            hyper::StatusCode::BAD_REQUEST,
            "image_base64",
            "image_base64 must not be empty (plain base64, no data: prefix)",
        );
    }
    let page = req.page.unwrap_or_else(|| "restored".to_string());
    let hint = req
        .hint
        .unwrap_or_else(|| "Restore this UI as an editable prototype".to_string());
    // Vision restore defaults to a VL model: the 4B text model's vision output
    // has a high structural corruption rate (observed: large runs of missing
    // quotes/colons); Qwen2.5-VL-7B output structure is reliable. The request
    // model field can override.
    let model = req
        .model
        .unwrap_or_else(|| VISION_MODEL_DEFAULT.to_string());
    tracing::info!(page = %page, model = %model, b64_len = req.image_base64.len(), "api/restore: screenshot restore");

    // base64 -> temp file -> screenshot_to_ui (multimodal vision path), deleted
    // after use (process-data cleanup).
    let bytes = match base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        req.image_base64.trim(),
    ) {
        Ok(b) => b,
        Err(e) => {
            return err_field(
                hyper::StatusCode::BAD_REQUEST,
                "image_base64",
                &format!("image_base64 decode failed: {e}"),
            )
        }
    };
    if bytes.is_empty() {
        return err_field(
            hyper::StatusCode::BAD_REQUEST,
            "image_base64",
            "image content is empty",
        );
    }
    let tmp_dir = std::env::temp_dir();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let img_path = tmp_dir.join(format!("fd-restore-{ts}.png"));
    if let Err(e) = std::fs::write(&img_path, &bytes) {
        return err_json(
            hyper::StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to write temporary image: {e}"),
        );
    }
    async fn run_restore(
        img_path: &std::path::Path,
        model: &str,
        hint: &str,
        page: &str,
    ) -> anyhow::Result<fd_canvas_core::PenDocument> {
        let client = build_client()?;
        let skills = fd_ai_adapter::DesignSkills::new(client, model.to_string());
        skills.screenshot_to_ui(img_path, hint, page).await
    }
    // Vision model output is occasionally structurally corrupted; retry once
    // with a varied hint (retrying the same string would return the same bad
    // output).
    let mut result = Err(anyhow::anyhow!("not executed"));
    for attempt in 1..=GEN_ATTEMPTS {
        let attempt_hint = if attempt == 1 {
            hint.clone()
        } else {
            format!("{hint} (attempt {attempt}, output valid complete JSON strictly, no extra quotes)")
        };
        result = run_restore(&img_path, &model, &attempt_hint, &page).await;
        if result.is_ok() {
            break;
        }
        tracing::warn!(attempt, "api/restore: restore failed, retrying");
    }
    let _ = std::fs::remove_file(&img_path);

    let mut doc = match result {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "api/restore: multimodal restore failed (including retries)");
            return err_json(hyper::StatusCode::BAD_GATEWAY, &format!("restore failed: {e}"));
        }
    };
    if let Some(ds) = req.design_system.as_deref() {
        let reg = state.registry.lock().unwrap();
        if reg.get(ds).is_some() {
            doc.active_design_system = Some(ds.to_string());
        } else {
            tracing::warn!(design_system = %ds, "api/restore: unknown design system, ignoring");
        }
    }
    match req.session {
        Some(sid) => {
            match push_version(state, &sid, &doc, &format!("restore: {}", truncate(&hint, 40))) {
                Ok(v) => v,
                Err(e) => err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        None => match finalize_new_session(state, &doc, &format!("restore: {}", truncate(&hint, 40)))
        {
            Ok(v) => v,
            Err(e) => err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
    }
}

pub fn session_info(state: &Arc<AppState>, id: &str) -> Resp {
    let Some(sess) = state.sessions.get(id) else {
        return err_field(
            hyper::StatusCode::NOT_FOUND,
            "session",
            &format!("session {id} does not exist"),
        );
    };
    ok_json(session_json(&sess))
}

pub fn sessions(state: &Arc<AppState>) -> Resp {
    let ids = state.sessions.list_ids();
    let mut out = Vec::new();
    for id in &ids {
        if let Some(s) = state.sessions.get(id) {
            out.push(session_json(&s));
        }
    }
    ok_json(serde_json::json!({"sessions": out}))
}

pub fn version_doc(state: &Arc<AppState>, id: &str, version: u32) -> Resp {
    let Some(sess) = state.sessions.get(id) else {
        return err_json(
            hyper::StatusCode::NOT_FOUND,
            &format!("session {id} does not exist"),
        );
    };
    match sess.versions.iter().find(|v| v.version == version) {
        Some(v) => {
            ok_json(serde_json::json!({"session": id, "version": version, "document": v.document}))
        }
        None => err_field(
            hyper::StatusCode::NOT_FOUND,
            "version",
            &format!("session {id} has no version v{version}"),
        ),
    }
}

#[derive(Deserialize)]
pub struct RollbackRequest {
    pub session: String,
    pub version: u32,
}

pub fn rollback(state: &Arc<AppState>, body: &[u8]) -> Resp {
    let req: RollbackRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return err_field(
                hyper::StatusCode::BAD_REQUEST,
                "body",
                &format!("failed to parse request body: {e}"),
            )
        }
    };
    if let Err(e) = state.sessions.rollback(&req.session, req.version) {
        return err_field(hyper::StatusCode::NOT_FOUND, "version", &e.to_string());
    }
    let sess = match state.sessions.get(&req.session) {
        Some(s) => s,
        None => return err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, "session lost after rollback"),
    };
    let cur = sess
        .current_doc()
        .map(|v| v.document.clone())
        .unwrap_or_default();
    ok_json(serde_json::json!({
        "session": req.session,
        "current_version": sess.current,
        "document": cur,
    }))
}

#[derive(Deserialize)]
pub struct ExportRequest {
    pub session: String,
    /// html | react.
    pub format: String,
    #[serde(default)]
    pub version: Option<u32>,
}

/// Export: fd-codegen HTML / React+Tailwind (issue#31 acceptance: HTML opens,
/// React compiles).
pub fn export(state: &Arc<AppState>, body: &[u8]) -> Resp {
    let req: ExportRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return err_field(
                hyper::StatusCode::BAD_REQUEST,
                "body",
                &format!("failed to parse request body: {e}"),
            )
        }
    };
    let Some(sess) = state.sessions.get(&req.session) else {
        return err_field(
            hyper::StatusCode::NOT_FOUND,
            "session",
            &format!("session {} does not exist", req.session),
        );
    };
    let target_version = req.version.unwrap_or(sess.current);
    let Some(entry) = sess.versions.iter().find(|v| v.version == target_version) else {
        return err_field(
            hyper::StatusCode::NOT_FOUND,
            "version",
            &format!("session {} has no version v{}", req.session, target_version),
        );
    };
    let doc = match doc_from_json(&entry.document) {
        Ok(d) => d,
        Err(e) => return err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    // Token color resolution: when a design system is declared, resolve var(--*)
    // to concrete colors (#8 policy, exported artifact is dependency-free).
    let doc = {
        let reg = state.registry.lock().unwrap();
        fd_codegen::resolve_tokens(&doc, &reg)
    };
    let (filename, content, mime) = match req.format.as_str() {
        "html" => (
            format!("fusion-design-v{}.html", target_version),
            fd_codegen::HtmlCodegen.generate(&doc),
            "text/html; charset=utf-8",
        ),
        "react" => (
            format!("GeneratedPageV{}.tsx", target_version),
            fd_codegen::ReactTailwindCodegen {
                component_name: format!("GeneratedPageV{target_version}"),
            }
            .generate(&doc),
            "text/plain; charset=utf-8",
        ),
        other => {
            return err_field(
                hyper::StatusCode::BAD_REQUEST,
                "format",
                &format!("unsupported export format {other} (supported: html / react)"),
            );
        }
    };
    tracing::info!(session = %req.session, version = target_version, format = %req.format, "api/export: export");
    ok_json(serde_json::json!({
        "filename": filename,
        "content": content,
        "mime": mime,
    }))
}

// ── internal helpers ──

fn session_json(sess: &crate::state::Session) -> serde_json::Value {
    serde_json::json!({
        "session": sess.id,
        "created_at": sess.created_at,
        "current_version": sess.current,
        "versions": sess.versions.iter().map(version_meta).collect::<Vec<_>>(),
    })
}

fn version_meta(v: &VersionEntry) -> serde_json::Value {
    serde_json::json!({
        "version": v.version,
        "label": v.label,
        "created_at": v.created_at,
        "node_count": v.node_count,
    })
}

/// New session + first version + response (shared by generate/restore).
fn finalize_new_session(
    state: &Arc<AppState>,
    doc: &fd_canvas_core::PenDocument,
    label: &str,
) -> anyhow::Result<Resp> {
    let json = doc_json(doc)?;
    let id = state.sessions.create(json.clone(), label);
    Ok(ok_json(serde_json::json!({
        "session": id,
        "current_version": 1,
        "document": json,
    })))
}

fn push_version(
    state: &Arc<AppState>,
    session: &str,
    doc: &fd_canvas_core::PenDocument,
    label: &str,
) -> anyhow::Result<Resp> {
    let json = doc_json(doc)?;
    let version = state.sessions.push_version(session, json.clone(), label)?;
    Ok(ok_json(serde_json::json!({
        "session": session,
        "current_version": version,
        "document": json,
    })))
}

// Issue #42: direct node mutation (canvas drag + token panel apply). Patches
// one node on the current version and pushes a new version — the workbench
// persists client-side edits without a model round-trip.
#[derive(Deserialize)]
pub struct NodeMutateRequest {
    pub node_id: String,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub w: Option<f64>,
    #[serde(default)]
    pub h: Option<f64>,
    #[serde(default)]
    pub fill: Option<String>,
    #[serde(default)]
    pub stroke: Option<String>,
    #[serde(default)]
    pub radius: Option<f64>,
    #[serde(default)]
    pub font_size: Option<f64>,
    #[serde(default)]
    pub opacity: Option<f64>,
}

fn mutate_node_in_doc(doc: &mut fd_canvas_core::PenDocument, req: &NodeMutateRequest) -> bool {
    fn apply(nodes: &mut [fd_canvas_core::PenNode], req: &NodeMutateRequest) -> bool {
        for node in nodes.iter_mut() {
            if node.id == req.node_id {
                let style = &mut node.style;
                if let Some(v) = req.x {
                    node.x = v;
                }
                if let Some(v) = req.y {
                    node.y = v;
                }
                if let Some(v) = req.w {
                    node.w = v;
                }
                if let Some(v) = req.h {
                    node.h = v;
                }
                if let Some(ref v) = req.fill {
                    style.fill = Some(v.clone());
                }
                if let Some(ref v) = req.stroke {
                    style.stroke = Some(v.clone());
                }
                if let Some(v) = req.radius {
                    style.radius = Some(v);
                }
                if let Some(v) = req.font_size {
                    style.font_size = Some(v);
                }
                if let Some(v) = req.opacity {
                    style.opacity = Some(v);
                }
                return true;
            }
            if apply(&mut node.children, req) {
                return true;
            }
        }
        false
    }
    doc.pages.iter_mut().any(|page| apply(&mut page.nodes, req))
}

pub fn node_mutate(state: &Arc<AppState>, session: &str, body: &[u8]) -> Resp {
    let req: NodeMutateRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return err_field(
                hyper::StatusCode::BAD_REQUEST,
                "body",
                &format!("failed to parse request body: {e}"),
            )
        }
    };
    if req.node_id.trim().is_empty() {
        return err_field(
            hyper::StatusCode::BAD_REQUEST,
            "node_id",
            "node_id must not be empty",
        );
    }
    let has_change = req.x.is_some()
        || req.y.is_some()
        || req.w.is_some()
        || req.h.is_some()
        || req.fill.is_some()
        || req.stroke.is_some()
        || req.radius.is_some()
        || req.font_size.is_some()
        || req.opacity.is_some();
    if !has_change {
        return err_field(
            hyper::StatusCode::BAD_REQUEST,
            "body",
            "at least one change field is required (x/y/w/h/fill/stroke/radius/font_size/opacity)",
        );
    }
    let mut doc = match resolve_session_doc(state, session, None) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    if !mutate_node_in_doc(&mut doc, &req) {
        return err_field(
            hyper::StatusCode::NOT_FOUND,
            "node_id",
            &format!("node {node_id} does not exist", node_id = req.node_id),
        );
    }
    match push_version(state, session, &doc, &format!("edit: {}", req.node_id)) {
        Ok(resp) => {
            tracing::info!(session = %session, node_id = %req.node_id, "api/session/mutate: node updated");
            resp
        }
        Err(e) => err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Ecosystem IPC base directory: FUSION_DESIGN_DATA_DIR > ~/.fusion-design.
fn ipc_base_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("FUSION_DESIGN_DATA_DIR") {
        return std::path::PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".fusion-design")
}

/// Resolves the session's current/specified version document into a PenDocument.
fn resolve_session_doc(
    state: &Arc<AppState>,
    session: &str,
    version: Option<u32>,
) -> Result<fd_canvas_core::PenDocument, Box<Resp>> {
    let Some(sess) = state.sessions.get(session) else {
        return Err(Box::new(err_field(
            hyper::StatusCode::NOT_FOUND,
            "session",
            &format!("session {session} does not exist"),
        )));
    };
    let entry = match version {
        Some(v) => sess.versions.iter().find(|e| e.version == v).cloned(),
        None => sess.current_doc().cloned(),
    };
    let Some(entry) = entry else {
        return Err(Box::new(err_field(
            hyper::StatusCode::NOT_FOUND,
            "version",
            &format!("session {session} has no version v{:?}", version),
        )));
    };
    doc_from_json(&entry.document).map_err(|e| {
        Box::new(err_json(
            hyper::StatusCode::INTERNAL_SERVER_ERROR,
            &format!("PenDocument deserialization failed: {e}"),
        ))
    })
}

#[derive(Deserialize)]
pub struct SendToCodeRequest {
    pub session: String,
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub component: Option<String>,
}

/// [Send to Code] (issue#33/DS-2): exports the current design via fd-ecosystem
/// file IPC (HTML + React/Tailwind) into the fusion-code convention directory
/// and sends a notification message.
pub fn send_to_code(state: &Arc<AppState>, body: &[u8]) -> Resp {
    send_to_code_with_base(state, &ipc_base_dir(), body)
}

pub fn send_to_code_with_base(state: &Arc<AppState>, base: &std::path::Path, body: &[u8]) -> Resp {
    let req: SendToCodeRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return err_json(
                hyper::StatusCode::BAD_REQUEST,
                &format!("failed to parse request body: {e}"),
            )
        }
    };
    let doc = match resolve_session_doc(state, &req.session, req.version) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let component = req
        .component
        .unwrap_or_else(|| "FusionDesignExport".to_string());
    let link = fd_ecosystem::EcosystemLink::new(base);
    match link.sync_to_code(&doc, &component) {
        Ok(files) => {
            tracing::info!(
                session = %req.session,
                files = files.len(),
                "api/send-to-code: design exported to the fusion-code IPC directory"
            );
            ok_json(serde_json::json!({
                "ok": true,
                "component": component,
                "files": files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            }))
        }
        Err(e) => {
            tracing::error!(error = %e, "api/send-to-code: IPC write failed");
            err_json(
                hyper::StatusCode::INTERNAL_SERVER_ERROR,
                &format!("send to Code failed: {e}"),
            )
        }
    }
}

#[derive(Deserialize)]
pub struct SaveArtifactRequest {
    pub session: String,
    #[serde(default)]
    pub version: Option<u32>,
    #[serde(default)]
    pub title: Option<String>,
    /// design / document / slides (carried in by landing-page create cards).
    #[serde(default)]
    pub kind: Option<String>,
}

/// [Save to Artifacts] (issue#33/DS-2): prefer creating the artifact via the
/// artifacts-engine REST API (visible on the landing page, with HTML content);
/// on success do not also write IPC — the engine's own IPC consumer would create
/// another type=data artifact from the same directory, and double-writing means
/// duplicate ingestion. Only when the engine is unreachable, fall back to writing
/// IPC (the consumer back-fills ingestion after the engine recovers); the
/// response carries engine_degraded for visibility.
pub async fn save_artifact(state: &Arc<AppState>, body: &[u8]) -> Resp {
    save_artifact_dual(state, &ipc_base_dir(), body, None).await
}

async fn save_artifact_dual(
    state: &Arc<AppState>,
    base: &std::path::Path,
    body: &[u8],
    client_override: Option<fd_ai_adapter::ArtifactsClient>,
) -> Resp {
    let req: SaveArtifactRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(_) => return save_artifact_with_base(state, base, body),
    };
    let doc = match resolve_session_doc(state, &req.session, req.version) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let title = req
        .title
        .clone()
        .unwrap_or_else(|| format!("fusion-design {}", req.session));
    let Some(new_artifact) = engine_create_request(state, &req, &doc, &title) else {
        return save_artifact_with_base(state, base, body);
    };
    let client = match client_override {
        Some(c) => Ok(c),
        None => fd_ai_adapter::ArtifactsClient::from_env(),
    };
    let engine_err = match client {
        Ok(c) => match c.create(&new_artifact).await {
            Ok(a) => {
                tracing::info!(id = %a.id, "api/save-artifact: ingested into artifacts-engine (IPC not written to avoid duplicate ingestion)");
                return ok_json(serde_json::json!({
                    "ok": true,
                    "artifact": a,
                    "engine_degraded": false,
                    "ipc_written": false,
                }));
            }
            Err(e) => e.to_string(),
        },
        Err(e) => e.to_string(),
    };
    tracing::warn!(error = %engine_err, "api/save-artifact: engine ingestion failed, falling back to IPC write");
    let mut v = match write_artifact_ipc(state, base, body) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    v["engine_degraded"] = serde_json::Value::Bool(true);
    v["error"] = serde_json::Value::String(engine_err);
    v["ipc_written"] = serde_json::Value::Bool(true);
    ok_json(v)
}

/// Ingestion request for "Save to Artifacts": the document is codegen'd to HTML
/// and mapped to an engine-accepted kind (app / document / template); the landing
/// page uses it for display and to open the viewer.
fn engine_create_request(
    state: &Arc<AppState>,
    req: &SaveArtifactRequest,
    doc: &fd_canvas_core::PenDocument,
    title: &str,
) -> Option<fd_ai_adapter::NewArtifact> {
    let reg = state.registry.lock().unwrap();
    let resolved = fd_codegen::resolve_tokens(doc, &reg);
    drop(reg);
    let content = fd_codegen::HtmlCodegen.generate(&resolved);
    let kind = match req.kind.as_deref() {
        Some("document") => "document",
        Some("slides") => "template",
        Some("design") | None => "app",
        Some(other) => {
            tracing::warn!(kind = other, "api/save-artifact: unknown kind, ingesting as app");
            "app"
        }
    };
    Some(fd_ai_adapter::NewArtifact {
        session_id: req.session.clone(),
        name: title.to_string(),
        artifact_type: "html".to_string(),
        content,
        kind: Some(kind.to_string()),
        summary: Some(truncate(title, 120)),
        metadata: Some(serde_json::json!({
            "source": "fusion-design",
            "design_system": doc.active_design_system,
            "kind": req.kind,
        })),
    })
}

pub fn save_artifact_with_base(state: &Arc<AppState>, base: &std::path::Path, body: &[u8]) -> Resp {
    match write_artifact_ipc(state, base, body) {
        Ok(v) => ok_json(v),
        Err(r) => *r,
    }
}

fn write_artifact_ipc(
    state: &Arc<AppState>,
    base: &std::path::Path,
    body: &[u8],
) -> Result<serde_json::Value, Box<Resp>> {
    let req: SaveArtifactRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return Err(Box::new(err_json(
                hyper::StatusCode::BAD_REQUEST,
                &format!("failed to parse request body: {e}"),
            )))
        }
    };
    let doc = resolve_session_doc(state, &req.session, req.version)?;
    let title = req
        .title
        .unwrap_or_else(|| format!("fusion-design {}", req.session));
    let msg = fd_ecosystem::LinkMessage {
        target: fd_ecosystem::EcosystemTarget::Custom("fusion-artifacts-engine".to_string()),
        action: "save_artifact".to_string(),
        payload: serde_json::json!({
            "source": "fusion-design",
            "title": title,
            "session": req.session,
            "version": req.version.unwrap_or_else(|| {
                state
                    .sessions
                    .get(&req.session)
                    .map(|s| s.current)
                    .unwrap_or(0)
            }),
            "document": doc_json(&doc).unwrap_or_default(),
            "design_system": doc.active_design_system,
            "kind": req.kind,
        }),
    };
    let link = fd_ecosystem::EcosystemLink::new(base);
    match link.send(&msg) {
        Ok(path) => {
            tracing::info!(path = %path.display(), "api/save-artifact: save message written to IPC");
            Ok(serde_json::json!({"ok": true, "ipc_message": path.display().to_string()}))
        }
        Err(e) => {
            tracing::error!(error = %e, "api/save-artifact: IPC write failed");
            Err(Box::new(err_json(
                hyper::StatusCode::INTERNAL_SERVER_ERROR,
                &format!("save to Artifacts failed: {e}"),
            )))
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// Extracts the first page name from document JSON (keeps page names stable
/// across iterations).
fn page_name_of(document_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(document_json)
        .ok()?
        .get("pages")?
        .get(0)?
        .get("name")?
        .as_str()
        .map(|s| s.to_string())
}

pub(crate) fn request_cap() -> usize {
    REQUEST_CAP
}

/// Artifacts landing-page query: parses the `/api/artifacts?...` query string.
/// tab: all (default) / mine (personal free artifacts) / shared (shared
/// artifacts, see handler docs).
pub fn artifacts_query(query: &str) -> (fd_ai_adapter::ArtifactQuery, String) {
    let get = |key: &str| {
        query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k == key).then(|| decode_component(v))
        })
    };
    let tab = get("tab").unwrap_or_else(|| "all".to_string());
    let page = get("page")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1)
        .max(1);
    let page_size = get("page_size")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(24)
        .clamp(1, 100);
    let sort = get("sort").unwrap_or_else(|| "updated_at".to_string());
    let allowed_sort = ["updated_at", "created_at", "name", "starred"];
    let sort = if allowed_sort.contains(&sort.as_str()) {
        sort
    } else {
        "updated_at".to_string()
    };
    let ownership_type = (tab == "mine").then(|| "free".to_string());
    let artifact_type = get("type").filter(|v| !v.is_empty());
    let name_search = get("q").filter(|v| !v.trim().is_empty());
    (
        fd_ai_adapter::ArtifactQuery {
            page,
            page_size,
            sort,
            artifact_type,
            ownership_type,
            name_search,
        },
        tab,
    )
}

fn decode_component(s: &str) -> String {
    let s = s.replace('+', " ");
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h = bytes.next().and_then(hex_val);
            let l = bytes.next().and_then(hex_val);
            if let (Some(h), Some(l)) = (h, l) {
                out.push(h * 16 + l);
                continue;
            }
        }
        out.push(b);
    }
    // percent-decoded multi-byte sequences (e.g. CJK search terms) must be
    // assembled as UTF-8; byte-by-byte char conversion would corrupt them.
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `GET /api/artifact/<id>?version=n`: viewer data (metadata + content).
/// Claude-style: clicking an artifact opens it locally, no external app.
pub async fn artifact_detail(id: &str, version: Option<u32>) -> Resp {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return err_json(hyper::StatusCode::BAD_REQUEST, "invalid artifact id");
    }
    let client = match fd_ai_adapter::ArtifactsClient::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "api/artifact: client construction failed");
            return ok_json(serde_json::json!({
                "degraded": true, "error": e.to_string(),
            }));
        }
    };
    match client.get(id).await {
        Ok(artifact) => match client.content(id, version).await {
            Ok(content) => {
                tracing::info!(id, bytes = content.len(), "api/artifact: content returned");
                ok_json(serde_json::json!({
                    "artifact": artifact, "content": content, "degraded": false,
                }))
            }
            Err(e) => {
                tracing::warn!(id, error = %e, "api/artifact: content read failed");
                ok_json(serde_json::json!({
                    "artifact": artifact, "content": "", "degraded": true, "error": e.to_string(),
                }))
            }
        },
        Err(e) => {
            tracing::warn!(id, error = %e, "api/artifact: metadata read failed");
            ok_json(serde_json::json!({ "degraded": true, "error": e.to_string() }))
        }
    }
}

/// `GET /api/artifacts`: landing-page list, sourced from the local
/// artifacts-engine.
/// When the engine is unreachable, degrades to "empty list + degraded"; the
/// landing page stays open and can create new content.
/// tab=shared: the engine has no share filter, so fetch 100 items per page and
/// locally retain entries with a non-empty share_id.
pub async fn artifacts(query: &str) -> Resp {
    let (mut q, tab) = artifacts_query(query);
    let shared_mode = tab == "shared";
    if shared_mode {
        q.page_size = 100;
        q.page = 1;
    }
    let client = match fd_ai_adapter::ArtifactsClient::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "api/artifacts: client construction failed");
            return ok_json(serde_json::json!({
                "artifacts": [], "total": 0, "page": q.page, "page_size": q.page_size,
                "degraded": true, "error": e.to_string(),
            }));
        }
    };
    match client.list(&q).await {
        Ok(page) => {
            let mut items = page.artifacts;
            let total = if shared_mode {
                items.retain(|a| a.share_id.as_deref().is_some_and(|s| !s.is_empty()));
                items.len() as u64
            } else {
                page.total
            };
            tracing::info!(
                tab = %tab,
                page = q.page,
                count = items.len(),
                total,
                "api/artifacts: list returned"
            );
            ok_json(serde_json::json!({
                "artifacts": items, "total": total, "page": q.page,
                "page_size": q.page_size, "degraded": false,
            }))
        }
        Err(e) => {
            tracing::warn!(error = %e, tab = %tab, "api/artifacts: engine unreachable, degrading to empty list");
            ok_json(serde_json::json!({
                "artifacts": [], "total": 0, "page": q.page, "page_size": q.page_size,
                "degraded": true, "error": e.to_string(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifacts_query_parsing() {
        // Defaults: all tab, updated_at, 24/page.
        let (q, tab) = artifacts_query("");
        assert_eq!(tab, "all");
        assert_eq!(q.sort, "updated_at");
        assert_eq!(q.page_size, 24);
        assert!(q.ownership_type.is_none());

        // mine tab -> ownership_type=free (an engine-filterable dimension).
        let (q, tab) = artifacts_query("tab=mine&page=2&sort=starred");
        assert_eq!(tab, "mine");
        assert_eq!(q.ownership_type.as_deref(), Some("free"));
        assert_eq!(q.page, 2);
        assert_eq!(q.sort, "starred");

        // Invalid sort falls back; page_size clamped; search term percent-decoded.
        let (q, _) = artifacts_query("sort=evil&page_size=999&q=%E7%99%BB%E5%BD%95+x");
        assert_eq!(q.sort, "updated_at");
        assert_eq!(q.page_size, 100);
        assert_eq!(q.name_search.as_deref(), Some("登录 x"));
    }

    #[test]
    fn resolve_model_chain() {
        // Explicit model wins.
        assert_eq!(resolve_model(&Some("m-explicit".into())), "m-explicit");
        // Empty string is treated as not provided; falls to env/default. Env is a
        // global race: this case only asserts a non-empty fallback.
        let m = resolve_model(&Some("  ".into()));
        assert!(!m.is_empty());
    }

    #[test]
    fn truncate_cjk() {
        assert_eq!(truncate("abc", 5), "abc");
        let long = "加暗黑模式环形进度带动画";
        let got = truncate(long, 5);
        assert!(got.ends_with('…') && got.chars().count() == 6);
    }

    #[test]
    fn page_name_of_extracts_first_page() {
        let json = serde_json::json!({"pages":[{"id":"p1","name":"home"}]}).to_string();
        assert_eq!(page_name_of(&json).as_deref(), Some("home"));
        assert_eq!(page_name_of("{}"), None);
    }

    #[test]
    fn request_cap_is_8mb() {
        assert_eq!(request_cap(), 8 * 1024 * 1024);
    }

    /// Reads the response body bytes (test helper).
    async fn body_of(resp: Resp) -> Vec<u8> {
        use http_body_util::BodyExt;
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    async fn json_of(resp: Resp) -> serde_json::Value {
        serde_json::from_slice(&body_of(resp).await).unwrap()
    }

    /// Offline export flow: seeded session -> html/react export (no model dependency).
    #[tokio::test]
    async fn export_html_and_react_from_seeded_session() {
        let state = std::sync::Arc::new(crate::AppState::new());
        let doc = serde_json::json!({
            "pages": [{
                "id": "p1", "name": "home", "width": 1440.0, "height": 900.0,
                "nodes": [
                    {"id": "n1", "kind": "Rect", "name": "button", "x": 10.0, "y": 10.0,
                     "w": 200.0, "h": 60.0, "text": "Hello"},
                    {"id": "n2", "kind": "Text", "name": "title", "x": 10.0, "y": 90.0,
                     "w": 200.0, "h": 30.0, "text": "Fusion"}
                ]
            }]
        })
        .to_string();
        let sid = state.sessions.create(doc.clone(), "seed");

        let body = serde_json::json!({"session": sid, "format": "html"}).to_string();
        let resp = export(&state, body.as_bytes());
        let v: serde_json::Value = json_of(resp).await;
        assert!(v["error"].is_null(), "html export should not error: {v}");
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("<!DOCTYPE html>"), "HTML export must be a complete document");
        assert!(content.contains("Fusion"), "HTML export must contain the Text node content");
        assert!(v["filename"].as_str().unwrap().ends_with(".html"));

        let body = serde_json::json!({"session": sid, "format": "react"}).to_string();
        let resp = export(&state, body.as_bytes());
        let v: serde_json::Value = json_of(resp).await;
        assert!(v["error"].is_null(), "react export should not error: {v}");
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("export function"), "React export must be a component");
        assert!(v["filename"].as_str().unwrap().ends_with(".tsx"));

        // Export a specific old version + reject invalid format.
        state
            .sessions
            .push_version(&sid, doc.clone(), "v2")
            .unwrap();
        let body = serde_json::json!({"session": sid, "format": "html", "version": 1}).to_string();
        let resp = export(&state, body.as_bytes());
        let v: serde_json::Value = json_of(resp).await;
        assert!(v["error"].is_null(), "export of a specific version should succeed");
        let body = serde_json::json!({"session": sid, "format": "swiftui"}).to_string();
        let resp = export(&state, body.as_bytes());
        assert_eq!(
            resp.status(),
            hyper::StatusCode::BAD_REQUEST,
            "invalid format should 400"
        );
    }

    /// Rollback + version query flow (no model dependency).
    #[tokio::test]
    async fn rollback_and_version_flow() {
        let state = std::sync::Arc::new(crate::AppState::new());
        let d1 = serde_json::json!({"pages": [{"id": "p", "name": "a", "width": 100.0, "height": 100.0, "nodes": []}]}).to_string();
        let d2 = serde_json::json!({"pages": [{"id": "p", "name": "a", "width": 100.0, "height": 100.0, "nodes": [
            {"id": "x", "kind": "Rect", "name": "dot", "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0}]}]}).to_string();
        let sid = state.sessions.create(d1, "seed");
        state.sessions.push_version(&sid, d2, "iter").unwrap();

        let body = serde_json::json!({"session": sid, "version": 1}).to_string();
        let resp = rollback(&state, body.as_bytes());
        let v: serde_json::Value = json_of(resp).await;
        assert_eq!(v["current_version"], 1, "current=1 after rollback");
        assert!(
            v["document"].as_str().unwrap().contains("\"nodes\":[]"),
            "document after rollback is v1"
        );

        let resp = version_doc(&state, &sid, 2);
        let v: serde_json::Value = json_of(resp).await;
        assert!(
            v["document"].as_str().unwrap().contains("x"),
            "v2 document still queryable"
        );
    }

    /// Ecosystem exits (issue#33/DS-2): send-to-Code / save-to-Artifacts disk
    /// verification (tempdir isolated).
    #[tokio::test]
    async fn send_to_code_and_save_artifact_write_ipc() {
        let state = std::sync::Arc::new(crate::AppState::new());
        let doc = serde_json::json!({
            "pages": [{
                "id": "p1", "name": "home", "width": 1440.0, "height": 900.0,
                "nodes": [
                    {"id": "n1", "kind": "Text", "name": "title", "x": 10.0, "y": 10.0,
                     "w": 200.0, "h": 30.0, "text": "Fusion"}
                ]
            }]
        })
        .to_string();
        let sid = state.sessions.create(doc, "seed");
        let base = tempfile::tempdir().unwrap();

        // Send to Code: HTML + TSX + notification message land in the fusion-code directory.
        let body = serde_json::json!({"session": sid}).to_string();
        let resp = send_to_code_with_base(&state, base.path(), body.as_bytes());
        let v: serde_json::Value = json_of(resp).await;
        assert!(v["error"].is_null(), "send-to-code should not error: {v}");
        assert_eq!(v["ok"], true);
        let files = v["files"].as_array().unwrap();
        assert_eq!(files.len(), 2, "must write HTML + TSX: {v}");
        for f in files {
            let p = std::path::Path::new(f.as_str().unwrap());
            assert!(p.exists(), "IPC files must actually be written: {}", p.display());
        }
        assert!(
            files
                .iter()
                .all(|f| f.as_str().unwrap().contains("fusion-code")),
            "files must land in the fusion-code convention directory"
        );

        // Custom component name + reject unknown session.
        let body = serde_json::json!({"session": sid, "component": "LoginCard"}).to_string();
        let resp = send_to_code_with_base(&state, base.path(), body.as_bytes());
        let v: serde_json::Value = json_of(resp).await;
        assert!(
            v["files"]
                .as_array()
                .is_some_and(|a| a.iter().any(|f| f.as_str().unwrap().contains("LoginCard"))),
            "custom component name takes effect: {v}"
        );
        let body = serde_json::json!({"session": "s-nope"}).to_string();
        let resp = send_to_code_with_base(&state, base.path(), body.as_bytes());
        assert_eq!(
            resp.status(),
            hyper::StatusCode::NOT_FOUND,
            "unknown session should 404"
        );

        // Save to Artifacts: IPC message lands in the fusion-artifacts-engine
        // directory and the payload carries provenance.
        let body = serde_json::json!({"session": sid, "title": "Login Page"}).to_string();
        let resp = save_artifact_with_base(&state, base.path(), body.as_bytes());
        let v: serde_json::Value = json_of(resp).await;
        assert!(v["error"].is_null(), "save-artifact should not error: {v}");
        assert_eq!(v["ok"], true);
        let msg_path = std::path::Path::new(v["ipc_message"].as_str().unwrap());
        assert!(msg_path.exists(), "IPC message must actually be written");
        assert!(
            msg_path
                .to_string_lossy()
                .contains("fusion-artifacts-engine"),
            "message must land in the artifacts-engine convention directory: {}",
            msg_path.display()
        );
        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(msg_path).unwrap()).unwrap();
        assert_eq!(payload["action"], "save_artifact");
        assert_eq!(payload["payload"]["source"], "fusion-design");
        assert_eq!(payload["payload"]["title"], "Login Page");
        assert!(
            payload["payload"]["document"]
                .as_str()
                .unwrap_or("")
                .contains("pages"),
            "message must carry the full document"
        );

        // Invalid request body rejected.
        let resp = save_artifact_with_base(&state, base.path(), b"not-json");
        assert_eq!(
            resp.status(),
            hyper::StatusCode::BAD_REQUEST,
            "invalid request body should 400"
        );
    }

    /// IPC fallback when the engine is unreachable: the consumer back-fills
    /// ingestion after the engine recovers; the response must fail visibly.
    #[tokio::test]
    async fn save_artifact_falls_back_to_ipc_when_engine_down() {
        let state = std::sync::Arc::new(crate::AppState::new());
        let doc = serde_json::json!({
            "pages": [{
                "id": "p1", "name": "home", "width": 1440.0, "height": 900.0,
                "nodes": [
                    {"id": "n1", "kind": "Text", "name": "title", "x": 10.0, "y": 10.0,
                     "w": 200.0, "h": 30.0, "text": "Fusion"}
                ]
            }]
        })
        .to_string();
        let sid = state.sessions.create(doc, "seed");
        let base = tempfile::tempdir().unwrap();
        let client = fd_ai_adapter::ArtifactsClient::new("http://127.0.0.1:1", "k").unwrap();
        let body = serde_json::json!({"session": sid, "title": "Login Page"}).to_string();
        let resp = save_artifact_dual(&state, base.path(), body.as_bytes(), Some(client)).await;
        let v: serde_json::Value = json_of(resp).await;
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["engine_degraded"], true, "{v}");
        assert_eq!(v["ipc_written"], true, "{v}");
        assert!(v["ipc_message"].as_str().is_some_and(|p| !p.is_empty()));
        assert!(v["error"].as_str().is_some_and(|e| !e.is_empty()));
    }

    /// issue#34 contract: region with no matching nodes -> 400 (no model call);
    /// on hit the prompt uses region assembly (TOC + changelog first, region
    /// JSON + instruction last).
    #[tokio::test]
    async fn iterate_region_contract() {
        let state = std::sync::Arc::new(crate::AppState::new());
        let doc = serde_json::json!({
            "pages": [{
                "id": "p1", "name": "home", "width": 1440.0, "height": 900.0,
                "nodes": [
                    {"id": "n1", "kind": "Text", "name": "title", "x": 10.0, "y": 10.0, "w": 200.0, "h": 30.0},
                    {"id": "n2", "kind": "Rect", "name": "bg", "x": 0.0, "y": 0.0, "w": 100.0, "h": 50.0}
                ]
            }]
        })
        .to_string();
        let sid = state.sessions.create(doc, "seed");
        state
            .sessions
            .push_version(
                &sid,
                serde_json::json!({"pages":[]}).to_string(),
                "iterate: add button",
            )
            .unwrap();

        // region with no match -> 400; request validation happens before any model call.
        let body =
            serde_json::json!({"session": sid, "instruction": "x", "region": ["nope"]}).to_string();
        let resp = iterate(&state, body.as_bytes()).await;
        assert_eq!(
            resp.status(),
            hyper::StatusCode::BAD_REQUEST,
            "no match must 400"
        );

        // Assembly contract (no model touched): ordering + region O(region) + changelog.
        let sess = state.sessions.get(&sid).unwrap();
        let user = crate::prompt::build_iterate_user(
            &sess.current_doc().unwrap().document,
            Some(&["n1".to_string()]),
            &crate::prompt::changelog_of(&sess),
            "make the title bigger",
        );
        assert!(user.contains("Document structure"), "{user}");
        assert!(user.contains("v2: iterate: add button"), "{user}");
        assert!(!user.contains("\"bg\""), "region JSON must not carry unselected nodes: {user}");
        assert!(user.contains("make the title bigger"), "{user}");
        let sys = crate::prompt::ITERATE_SYS;
        assert!(
            !sys.contains("Document structure") && !sys.contains("Recent changes"),
            "sys must not embed per-request blocks (prefix cache stable head)"
        );
    }
}

#[cfg(test)]
mod contract_tests {
    // issue#35 contract tests: pin the generate/iterate/rollback/restore/tokens
    // request/response shapes and field-targeted 4xx. Changing the contract
    // requires updating docs/API.md and this module together.
    use super::*;

    async fn body_of(resp: Resp) -> Vec<u8> {
        use http_body_util::BodyExt;
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    async fn json_of(resp: Resp) -> serde_json::Value {
        serde_json::from_slice(&body_of(resp).await).unwrap()
    }

    fn seeded_state() -> (std::sync::Arc<crate::AppState>, String, String) {
        let state = std::sync::Arc::new(crate::AppState::new());
        let doc = serde_json::json!({
            "pages": [{
                "id": "p1", "name": "home", "width": 1440.0, "height": 900.0,
                "nodes": [
                    {"id": "n1", "kind": "Text", "name": "title", "x": 10.0, "y": 10.0,
                     "w": 200.0, "h": 30.0, "text": "Hello"}
                ]
            }]
        })
        .to_string();
        let sid = state.sessions.create(doc.clone(), "seed");
        (state, sid, doc)
    }

    #[tokio::test]
    async fn contract_4xx_errors_are_field_located() {
        let (state, sid, doc) = seeded_state();

        // generate: empty prompt -> 400 + field=prompt.
        let v = json_of(generate(&state, br#"{"prompt":"  "}"#).await).await;
        assert_eq!(v["field"], "prompt", "{v}");

        // generate: bad body -> 400 + field=body.
        let v = json_of(generate(&state, br#"not json"#).await).await;
        assert_eq!(v["field"], "body", "{v}");

        // iterate: empty instruction -> 400 + field=instruction.
        let b = serde_json::json!({"session": sid, "instruction": ""});
        let v = json_of(iterate(&state, b.to_string().as_bytes()).await).await;
        assert_eq!(v["field"], "instruction", "{v}");

        // iterate: unknown session -> 404 + field=session.
        let b = serde_json::json!({"session": "s-nope", "instruction": "x"});
        let v = json_of(iterate(&state, b.to_string().as_bytes()).await).await;
        assert_eq!(v["field"], "session", "{v}");
        assert!(v.as_object().unwrap()["error"].is_string(), "{v}");

        // iterate: region with no match -> 400 + field=region.
        let b = serde_json::json!({"session": sid, "instruction": "x", "region": ["ghost"]});
        let v = json_of(iterate(&state, b.to_string().as_bytes()).await).await;
        assert_eq!(v["field"], "region", "{v}");

        // rollback: bad body -> 400 + field=body; unknown version -> 404 + field=version.
        let v = json_of(rollback(&state, br#"{"session":"x"}"#)).await;
        assert_eq!(v["field"], "body", "{v}");
        let b = serde_json::json!({"session": sid, "version": 99});
        let v = json_of(rollback(&state, b.to_string().as_bytes())).await;
        assert_eq!(v["field"], "version", "{v}");

        // restore: empty image -> 400 + field=image_base64.
        let v = json_of(restore(&state, br#"{"image_base64":" "}"#).await).await;
        assert_eq!(v["field"], "image_base64", "{v}");

        // export: invalid format -> 400 + field=format.
        let b = serde_json::json!({"session": sid, "format": "pdf"});
        let v = json_of(export(&state, b.to_string().as_bytes())).await;
        assert_eq!(v["field"], "format", "{v}");

        // tokens: unknown ds -> 404 + field=ds.
        let v = json_of(tokens(&state, "no-such-ds", "light")).await;
        assert_eq!(v["field"], "ds", "{v}");

        // session_info: unknown -> 404 + field=session.
        let v = json_of(session_info(&state, "s-nope")).await;
        assert_eq!(v["field"], "session", "{v}");
        let _ = doc;
    }

    #[tokio::test]
    async fn contract_tokens_shape() {
        let (state, _sid, _doc) = seeded_state();
        let v = json_of(tokens(&state, "apple-hig", "dark")).await;
        assert!(v["error"].is_null(), "{v}");
        assert_eq!(v["design_system"], "apple-hig");
        assert_eq!(v["theme"], "Dark");
        assert!(
            v["css"].as_str().unwrap_or_default().contains("--"),
            "CSS vars: {v}"
        );
        // issue#42: tokens array (name/value pairs; dark theme gives the dark merged view).
        let toks = v["tokens"].as_array().expect("tokens array");
        assert!(!toks.is_empty(), "{v}");
        let bg = toks
            .iter()
            .find(|t| t["name"] == "color.bg")
            .expect("color.bg");
        assert_eq!(bg["value"], "#1C1C1E", "dark overlay applied: {bg}");
    }

    #[tokio::test]
    async fn contract_design_intel_search() {
        // issue#44: GET /api/design-intel/search dataset search contract.

        // empty q -> 400 + field=q.
        let v = json_of(design_intel_search("", "all", 5)).await;
        assert_eq!(v["field"], "q", "{v}");
        let v = json_of(design_intel_search("   ", "all", 5)).await;
        assert_eq!(v["field"], "q", "{v}");

        // unknown kind -> 400 + field=kind (with valid values in the hint).
        let v = json_of(design_intel_search("dashboard", "nope", 5)).await;
        assert_eq!(v["field"], "kind", "{v}");
        assert!(v["error"].as_str().unwrap_or_default().contains("styles"));

        // Hit: {kind, q, count, hits[{kind,id,title,detail,score}]}, score descending.
        let v = json_of(design_intel_search("dashboard dark console", "styles", 3)).await;
        assert!(v["error"].is_null(), "{v}");
        assert_eq!(v["kind"], "styles");
        assert_eq!(v["q"], "dashboard dark console");
        let hits = v["hits"].as_array().expect("hits array");
        assert!(!hits.is_empty(), "{v}");
        assert!(hits.len() <= 3, "limit respected: {v}");
        let first = &hits[0];
        for k in ["kind", "id", "title", "detail", "score"] {
            assert!(!first[k].is_null(), "hit missing {k}: {v}");
        }
        for pair in hits.windows(2) {
            assert!(
                pair[0]["score"].as_i64().unwrap_or(0) >= pair[1]["score"].as_i64().unwrap_or(0)
            );
        }

        // all mixed search returns multiple kinds; limit defaults to 5, invalid values fall back.
        let v = json_of(design_intel_search("developer dashboard dark", "all", 20)).await;
        let hits = v["hits"].as_array().expect("hits array");
        let kinds: std::collections::HashSet<&str> =
            hits.iter().filter_map(|h| h["kind"].as_str()).collect();
        assert!(kinds.len() >= 2, "cross-kind hits: {kinds:?}");
    }

    #[tokio::test]
    async fn contract_node_mutate() {
        // issue#42: POST /api/session/<id>/mutate mutates the node directly and pushes a new version.
        let (state, sid, _doc) = seeded_state();

        // bad body -> 400 + field=body.
        let v = json_of(node_mutate(&state, &sid, b"not json")).await;
        assert_eq!(v["field"], "body", "{v}");

        // empty node_id -> 400 + field=node_id.
        let v = json_of(node_mutate(&state, &sid, br#"{"node_id":" "}"#)).await;
        assert_eq!(v["field"], "node_id", "{v}");

        // no change fields -> 400.
        let v = json_of(node_mutate(&state, &sid, br#"{"node_id":"n1"}"#)).await;
        assert_eq!(v["field"], "body", "{v}");

        // unknown session -> 404 + field=session.
        let b = serde_json::json!({"node_id": "n1", "x": 42.0});
        let v = json_of(node_mutate(&state, "s-nope", b.to_string().as_bytes())).await;
        assert_eq!(v["field"], "session", "{v}");

        // unknown node -> 404 + field=node_id.
        let b = serde_json::json!({"node_id": "ghost", "x": 42.0});
        let v = json_of(node_mutate(&state, &sid, b.to_string().as_bytes())).await;
        assert_eq!(v["field"], "node_id", "{v}");

        // Success: {session, current_version, document}, the new version contains the change.
        let b = serde_json::json!({"node_id": "n1", "x": 42.0, "fill": "#4F7CFF"});
        let v = json_of(node_mutate(&state, &sid, b.to_string().as_bytes())).await;
        assert!(v["error"].is_null(), "{v}");
        assert_eq!(v["current_version"], 2, "{v}");
        assert!(v["document"].is_string());
        let doc: serde_json::Value = serde_json::from_str(v["document"].as_str().unwrap()).unwrap();
        let node = &doc["pages"][0]["nodes"][0];
        assert_eq!(node["x"], 42.0);
        assert_eq!(node["style"]["fill"], "#4F7CFF");
    }

    #[tokio::test]
    async fn contract_success_shapes() {
        let (state, sid, doc) = seeded_state();

        // rollback: {session, current_version, document}.
        state
            .sessions
            .push_version(&sid, doc.clone(), "v2")
            .unwrap();
        let b = serde_json::json!({"session": sid, "version": 1});
        let v = json_of(rollback(&state, b.to_string().as_bytes())).await;
        assert!(v["error"].is_null(), "{v}");
        assert_eq!(v["session"], sid);
        assert_eq!(v["current_version"], 1);
        assert!(v["document"].is_string(), "document must be a JSON string: {v}");

        // export: {filename, content, mime}.
        let b = serde_json::json!({"session": sid, "format": "html"});
        let v = json_of(export(&state, b.to_string().as_bytes())).await;
        assert!(v["error"].is_null(), "{v}");
        for k in ["filename", "content", "mime"] {
            assert!(v[k].is_string(), "export missing {k}: {v}");
        }

        // session_info: {session, current_version, versions[]}.
        let v = json_of(session_info(&state, &sid)).await;
        assert_eq!(v["session"], sid);
        assert!(v["current_version"].is_u64());
        assert!(v["versions"].is_array());
        let first = &v["versions"][0];
        for k in ["version", "label", "created_at", "node_count"] {
            assert!(first.get(k).is_some(), "version entry missing {k}: {first}");
        }

        // version_doc: {version, document}.
        let v = json_of(version_doc(&state, &sid, 2)).await;
        assert!(v["error"].is_null(), "{v}");
        assert_eq!(v["version"], 2);
        assert!(v["document"].is_string());
    }
}
