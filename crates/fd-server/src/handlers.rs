//! API handlers：生成 / 迭代 / 截图还原 / 版本 / 导出 / 健康检查 / 设计系统。
//!
//! 模型调用全部经 fd-ai-adapter（gateway 11432 默认，FUSION_MLX_BASE_URL 可覆盖，
//! 鉴权 FUSION_CLIENT_KEY 别名链）。请求体上限 REQUEST_CAP 防 OOM。

use std::sync::Arc;

use serde::Deserialize;

use fd_codegen::Codegen;

use crate::state::VersionEntry;
use crate::AppState;

/// 请求体上限（字节）。截图 base64 可达数 MB，8MB 与 IPC 消息护栏同口径。
const REQUEST_CAP: usize = 8 * 1024 * 1024;

/// 模型生成/迭代尝试次数（首试 + 1 次重试）。4B 模型偶发 JSON 截断，重试显著降失败率。
const GEN_ATTEMPTS: usize = 2;

/// 视觉还原（restore）默认模型：须为 vision 能力模型。
const VISION_MODEL_DEFAULT: &str = "mlx-community--Qwen2.5-VL-7B-Instruct-4bit";

/// 解析请求模型 id：显式 > FUSION_MLX_MODEL > 内置缺省（本地已加载 4B 文本模型）。
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

/// 构建模型客户端（endpoint 解析链复用 fd-ai-adapter：env > gateway 缺省）。
fn build_client() -> anyhow::Result<fd_ai_adapter::FusionMlxClient> {
    fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint("")?,
    )
}

/// PenDocument 序列化（与 .fusiondesign 文件格式一致）。
fn doc_json(doc: &fd_canvas_core::PenDocument) -> anyhow::Result<String> {
    Ok(serde_json::to_string(doc)?)
}

fn doc_from_json(json: &str) -> anyhow::Result<fd_canvas_core::PenDocument> {
    serde_json::from_str(json).map_err(|e| anyhow::anyhow!("PenDocument 反序列化失败: {e}"))
}

/// 统一 JSON 响应封装。
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
    // 深探测走 ?deep=1：/v1/models 鉴权 + 列表（async health_check，3s 超时）。
    // 不得用 health_check_sync：同步阻塞客户端自带内部 runtime，
    // 在 async handler 内调用会死锁（异步上下文互斥）。
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

/// 设计系统 Token CSS（画布 ApplyTokens 消费；theme=light|dark）。
pub fn tokens(state: &Arc<AppState>, ds: &str, theme: &str) -> Resp {
    let mut reg = state.registry.lock().unwrap();
    let theme = match theme {
        "dark" => fd_design_system::Theme::Dark,
        _ => fd_design_system::Theme::Light,
    };
    reg.activate_theme(theme);
    match reg.get(ds) {
        Some(system) => ok_json(serde_json::json!({
            "design_system": ds,
            "theme": theme,
            "css": system.to_css_custom_properties_for_theme(theme),
        })),
        None => err_json(
            hyper::StatusCode::NOT_FOUND,
            &format!("设计系统 {ds} 不存在"),
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
            return err_json(
                hyper::StatusCode::BAD_REQUEST,
                &format!("请求体解析失败: {e}"),
            )
        }
    };
    if req.prompt.trim().is_empty() {
        return err_json(hyper::StatusCode::BAD_REQUEST, "prompt 不能为空");
    }
    let page = req.page.unwrap_or_else(|| "page-1".to_string());
    let model = resolve_model(&req.model);
    tracing::info!(page = %page, model = %model, "api/generate: 文生 UI");

    let client = match build_client() {
        Ok(c) => c,
        Err(e) => {
            return err_json(
                hyper::StatusCode::BAD_GATEWAY,
                &format!("模型客户端构造失败: {e}"),
            )
        }
    };
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    // 4B 模型偶发 JSON 截断（parse_ui_json 修复链兜不住），重试一次再判失败。
    let mut doc = None;
    let mut last_err = String::new();
    for attempt in 1..=GEN_ATTEMPTS {
        // 重试须变化 prompt：gateway 对同 prompt 有缓存，同串重试只会拿到同一份坏输出。
        let prompt = if attempt == 1 {
            req.prompt.clone()
        } else {
            format!(
                "{}\n（第 {attempt} 次尝试，请严格输出合法完整 JSON）",
                req.prompt
            )
        };
        match skills.text_to_ui_async(&prompt, &page).await {
            Ok(d) => {
                doc = Some(d);
                break;
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "api/generate: 生成失败，准备重试");
                last_err = e.to_string();
            }
        }
    }
    let Some(mut doc) = doc else {
        tracing::error!(error = %last_err, "api/generate: 生成失败（含重试）");
        return err_json(
            hyper::StatusCode::BAD_GATEWAY,
            &format!("生成失败: {last_err}"),
        );
    };
    if let Some(ds) = req.design_system.as_deref() {
        let reg = state.registry.lock().unwrap();
        if reg.get(ds).is_some() {
            doc.active_design_system = Some(ds.to_string());
        } else {
            tracing::warn!(design_system = %ds, "api/generate: 未知设计系统，忽略");
        }
    }
    match finalize_new_session(state, &doc, &format!("生成：{}", truncate(&req.prompt, 40))) {
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
}

/// 迭代：当前版本文档 + 指令 → 全页重写为新一版。
///
/// 走与 text_to_ui 相同的 parse_ui_json 路径（已验证 schema），prompt 携带
/// 当前文档 JSON 要求保持未提及内容不变。4B 模型全页重写上限 4096 tokens
/// （DEFAULT_MAX_TOKENS 同口径）。
pub async fn iterate(state: &Arc<AppState>, body: &[u8]) -> Resp {
    let req: IterateRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return err_json(
                hyper::StatusCode::BAD_REQUEST,
                &format!("请求体解析失败: {e}"),
            )
        }
    };
    if req.instruction.trim().is_empty() {
        return err_json(hyper::StatusCode::BAD_REQUEST, "instruction 不能为空");
    }
    let Some(sess) = state.sessions.get(&req.session) else {
        return err_json(
            hyper::StatusCode::NOT_FOUND,
            &format!("session {} 不存在", req.session),
        );
    };
    let Some(cur) = sess.current_doc() else {
        return err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, "会话无当前版本");
    };
    let model = resolve_model(&req.model);
    tracing::info!(session = %req.session, model = %model, "api/iterate: 对话迭代");

    let client = match build_client() {
        Ok(c) => c,
        Err(e) => {
            return err_json(
                hyper::StatusCode::BAD_GATEWAY,
                &format!("模型客户端构造失败: {e}"),
            )
        }
    };
    let sys = "你是 fusion-design UI 迭代器。输入当前页面 JSON 与修改指令，\
输出修改后的完整页面 JSON（{\"page\":{...}} 同 schema）。保持 width/height 与\
未提及节点不变，仅落实指令要求。只输出 JSON，禁止额外文字。";
    let user_base = format!(
        "当前页面：\n{doc}\n\n修改指令：{ins}",
        doc = cur.document,
        ins = req.instruction
    );
    let page_name = page_name_of(&cur.document).unwrap_or_else(|| "page-1".to_string());
    // 同 generate：模型输出偶发非 schema JSON，调用+解析整体重试一次。
    let mut doc = None;
    let mut last_err = String::new();
    for attempt in 1..=GEN_ATTEMPTS {
        let user = if attempt == 1 {
            user_base.clone()
        } else {
            format!("{user_base}\n（第 {attempt} 次尝试，请严格输出合法完整 JSON）")
        };
        let resp = match client.chat_async(&model, sys, &user, 4096).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(attempt, error = %e, "api/iterate: 模型调用失败，准备重试");
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
                tracing::warn!(attempt, error = %e, "api/iterate: 输出解析失败，准备重试");
                last_err = "模型输出未按页面 schema 返回".to_string();
            }
        }
    }
    let Some(mut doc) = doc else {
        tracing::error!(error = %last_err, "api/iterate: 迭代失败（含重试）");
        return err_json(
            hyper::StatusCode::BAD_GATEWAY,
            &format!("迭代失败: {last_err}"),
        );
    };
    // 迭代保留设计系统声明：请求显式指定 > 当前版沿用。
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
        &format!("迭代：{}", truncate(&req.instruction, 40)),
    ) {
        Ok(v) => v,
        Err(e) => err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct RestoreRequest {
    /// 图片 base64（不带 data: 前缀，前端剥掉）。
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

/// 截图/草图还原：fusion-mlx 多模态视觉输入 → PenDocument（issue#31 验收项）。
pub async fn restore(state: &Arc<AppState>, body: &[u8]) -> Resp {
    let req: RestoreRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return err_json(
                hyper::StatusCode::BAD_REQUEST,
                &format!("请求体解析失败: {e}"),
            )
        }
    };
    if req.image_base64.trim().is_empty() {
        return err_json(hyper::StatusCode::BAD_REQUEST, "image_base64 不能为空");
    }
    let page = req.page.unwrap_or_else(|| "restored".to_string());
    let hint = req
        .hint
        .unwrap_or_else(|| "还原此界面为可编辑原型".to_string());
    // 视觉还原默认用 VL 模型：4B 文本模型的视觉输出结构性损坏率高（实测，
    // 缺引号/缺冒号成片），Qwen2.5-VL-7B 输出结构可靠。请求 model 字段可覆盖。
    let model = req
        .model
        .unwrap_or_else(|| VISION_MODEL_DEFAULT.to_string());
    tracing::info!(page = %page, model = %model, b64_len = req.image_base64.len(), "api/restore: 截图还原");

    // base64 → 临时文件 → screenshot_to_ui（多模态视觉路径），用后即删（过程数据清理）。
    let bytes = match base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        req.image_base64.trim(),
    ) {
        Ok(b) => b,
        Err(e) => {
            return err_json(
                hyper::StatusCode::BAD_REQUEST,
                &format!("image_base64 解码失败: {e}"),
            )
        }
    };
    if bytes.is_empty() {
        return err_json(hyper::StatusCode::BAD_REQUEST, "图片内容为空");
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
            &format!("临时图片写入失败: {e}"),
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
    // 视觉模型输出偶发结构性损坏，hint 变体重试一次（同串重试拿同一份坏输出无意义）。
    let mut result = Err(anyhow::anyhow!("未执行"));
    for attempt in 1..=GEN_ATTEMPTS {
        let attempt_hint = if attempt == 1 {
            hint.clone()
        } else {
            format!("{hint}（第 {attempt} 次尝试，请严格输出合法完整 JSON，勿加多余引号）")
        };
        result = run_restore(&img_path, &model, &attempt_hint, &page).await;
        if result.is_ok() {
            break;
        }
        tracing::warn!(attempt, "api/restore: 还原失败，准备重试");
    }
    let _ = std::fs::remove_file(&img_path);

    let mut doc = match result {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "api/restore: 多模态还原失败（含重试）");
            return err_json(hyper::StatusCode::BAD_GATEWAY, &format!("还原失败: {e}"));
        }
    };
    if let Some(ds) = req.design_system.as_deref() {
        let reg = state.registry.lock().unwrap();
        if reg.get(ds).is_some() {
            doc.active_design_system = Some(ds.to_string());
        } else {
            tracing::warn!(design_system = %ds, "api/restore: 未知设计系统，忽略");
        }
    }
    match req.session {
        Some(sid) => {
            match push_version(state, &sid, &doc, &format!("还原：{}", truncate(&hint, 40))) {
                Ok(v) => v,
                Err(e) => err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        None => match finalize_new_session(state, &doc, &format!("还原：{}", truncate(&hint, 40)))
        {
            Ok(v) => v,
            Err(e) => err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
    }
}

pub fn session_info(state: &Arc<AppState>, id: &str) -> Resp {
    let Some(sess) = state.sessions.get(id) else {
        return err_json(
            hyper::StatusCode::NOT_FOUND,
            &format!("session {id} 不存在"),
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
            &format!("session {id} 不存在"),
        );
    };
    match sess.versions.iter().find(|v| v.version == version) {
        Some(v) => {
            ok_json(serde_json::json!({"session": id, "version": version, "document": v.document}))
        }
        None => err_json(
            hyper::StatusCode::NOT_FOUND,
            &format!("session {id} 无版本 v{version}"),
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
            return err_json(
                hyper::StatusCode::BAD_REQUEST,
                &format!("请求体解析失败: {e}"),
            )
        }
    };
    if let Err(e) = state.sessions.rollback(&req.session, req.version) {
        return err_json(hyper::StatusCode::NOT_FOUND, &e.to_string());
    }
    let sess = match state.sessions.get(&req.session) {
        Some(s) => s,
        None => return err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, "回退后会话丢失"),
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
    /// html | react。
    pub format: String,
    #[serde(default)]
    pub version: Option<u32>,
}

/// 导出：fd-codegen HTML / React+Tailwind（issue#31 验收：HTML 可打开、React 可编译）。
pub fn export(state: &Arc<AppState>, body: &[u8]) -> Resp {
    let req: ExportRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return err_json(
                hyper::StatusCode::BAD_REQUEST,
                &format!("请求体解析失败: {e}"),
            )
        }
    };
    let Some(sess) = state.sessions.get(&req.session) else {
        return err_json(
            hyper::StatusCode::NOT_FOUND,
            &format!("session {} 不存在", req.session),
        );
    };
    let target_version = req.version.unwrap_or(sess.current);
    let Some(entry) = sess.versions.iter().find(|v| v.version == target_version) else {
        return err_json(
            hyper::StatusCode::NOT_FOUND,
            &format!("session {} 无版本 v{}", req.session, target_version),
        );
    };
    let doc = match doc_from_json(&entry.document) {
        Ok(d) => d,
        Err(e) => return err_json(hyper::StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    // token 颜色解析：声明设计系统时把 var(--*) 落成具体色（#8 口径，导出件免依赖）。
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
            return err_json(
                hyper::StatusCode::BAD_REQUEST,
                &format!("不支持的导出格式 {other}（支持 html / react）"),
            );
        }
    };
    tracing::info!(session = %req.session, version = target_version, format = %req.format, "api/export: 导出");
    ok_json(serde_json::json!({
        "filename": filename,
        "content": content,
        "mime": mime,
    }))
}

// ── 内部工具 ──

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

/// 新会话 + 首版 + 回包（生成/还原共用）。
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

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// 从文档 JSON 提取首个页面名（迭代保持页面名稳定）。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_model_chain() {
        // 显式优先。
        assert_eq!(resolve_model(&Some("m-explicit".into())), "m-explicit");
        // 空串视为未提供，落 env/缺省。env 全局竞态：本用例只断言非空回退。
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

    /// 取响应体字节（测试辅助）。
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

    /// 离线导出链路：种子会话 → html/react 导出（无模型依赖）。
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
        assert!(v["error"].is_null(), "html 导出不应报错: {v}");
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("<!DOCTYPE html>"), "HTML 导出须为完整文档");
        assert!(content.contains("Fusion"), "HTML 导出须含 Text 节点文本");
        assert!(v["filename"].as_str().unwrap().ends_with(".html"));

        let body = serde_json::json!({"session": sid, "format": "react"}).to_string();
        let resp = export(&state, body.as_bytes());
        let v: serde_json::Value = json_of(resp).await;
        assert!(v["error"].is_null(), "react 导出不应报错: {v}");
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("export function"), "React 导出须为组件");
        assert!(v["filename"].as_str().unwrap().ends_with(".tsx"));

        // 指定旧版导出 + 非法格式拒绝。
        state
            .sessions
            .push_version(&sid, doc.clone(), "v2")
            .unwrap();
        let body = serde_json::json!({"session": sid, "format": "html", "version": 1}).to_string();
        let resp = export(&state, body.as_bytes());
        let v: serde_json::Value = json_of(resp).await;
        assert!(v["error"].is_null(), "指定版本导出应成功");
        let body = serde_json::json!({"session": sid, "format": "swiftui"}).to_string();
        let resp = export(&state, body.as_bytes());
        assert_eq!(
            resp.status(),
            hyper::StatusCode::BAD_REQUEST,
            "非法格式应 400"
        );
    }

    /// 回退 + 版本查询链路（无模型依赖）。
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
        assert_eq!(v["current_version"], 1, "回退后 current=1");
        assert!(
            v["document"].as_str().unwrap().contains("\"nodes\":[]"),
            "回退后文档为 v1"
        );

        let resp = version_doc(&state, &sid, 2);
        let v: serde_json::Value = json_of(resp).await;
        assert!(
            v["document"].as_str().unwrap().contains("x"),
            "v2 文档仍可查询"
        );
    }
}
