//! fusion-artifacts-engine REST 客户端（Artifacts 落地页数据源）。
//!
//! 只读消费本机 `127.0.0.1:11451` 的 `/api/v1/artifacts` 列表接口：
//! 100% 离线约束不变（validate_localhost 强校验，公网 endpoint 直接拒绝）。
//! 鉴权双通道对齐引擎：`X-API-Key`（env `FUSION_ARTIFACTS_API_KEY` 或
//! `~/.fusion/artifacts/config.yaml` 的 `security.api_key`）与
//! `Authorization: Bearer`（同 key）。
//!
//! 降级契约：引擎不可达/未配 key 时返回 Err，调用方（fd-server）转成
//! 「空列表 + degraded」响应，绝不 500 打断落地页。

use std::time::Duration;

use serde::{Deserialize, Serialize};

const DEFAULT_ARTIFACTS_URL: &str = "http://127.0.0.1:11451";

/// 列表查询参数。字段名与引擎 filters 对齐，未设置的项不下发。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactQuery {
    pub page: u32,
    pub page_size: u32,
    pub sort: String,
    pub artifact_type: Option<String>,
    pub ownership_type: Option<String>,
    pub name_search: Option<String>,
}

impl Default for ArtifactQuery {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 24,
            sort: "updated_at".to_string(),
            artifact_type: None,
            ownership_type: None,
            name_search: None,
        }
    }
}

/// 落地页单条卡片所需的字段子集（引擎 model_dump 的超集会被忽略）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtifactSummary {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default, rename = "type")]
    pub artifact_type: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub current_version: u32,
    #[serde(default)]
    pub created_at: f64,
    #[serde(default)]
    pub updated_at: f64,
    #[serde(default)]
    pub is_starred: bool,
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub share_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactPage {
    pub artifacts: Vec<ArtifactSummary>,
    pub total: u64,
}

/// 新建产物入参（引擎 `artifact.create` 的 REST 形状）。
#[derive(Debug, Clone)]
pub struct NewArtifact {
    pub session_id: String,
    pub name: String,
    pub artifact_type: String,
    pub content: String,
    pub kind: Option<String>,
    pub summary: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct EngineListResponse {
    #[serde(default)]
    artifacts: Vec<ArtifactSummary>,
    #[serde(default)]
    total: u64,
}

/// 引擎客户端。每次请求构造（reqwest 连接池随构造重建），
/// 落地页并发量极低，换取 env/config 热改后即时生效。
#[derive(Debug, Clone)]
pub struct ArtifactsClient {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

impl ArtifactsClient {
    pub fn new(base_url: &str, api_key: &str) -> anyhow::Result<Self> {
        crate::validate_localhost(base_url)?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(800))
            .timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            http,
        })
    }

    /// env `FUSION_ARTIFACTS_URL` / `FUSION_ARTIFACTS_API_KEY`，key 缺省回退
    /// 引擎自身配置文件（`FUSION_ARTIFACTS_CONFIG` > `~/.fusion/artifacts/config.yaml`）。
    pub fn from_env() -> anyhow::Result<Self> {
        let base = std::env::var("FUSION_ARTIFACTS_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| DEFAULT_ARTIFACTS_URL.to_string());
        let key = std::env::var("FUSION_ARTIFACTS_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(read_engine_api_key);
        Self::new(&base, &key)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 查询串构造：filters 以 JSON 下发（引擎 `_parse_json_query` 解析）。
    pub fn build_url(&self, q: &ArtifactQuery) -> String {
        let mut url = format!(
            "{}/api/v1/artifacts?page={}&page_size={}&sort={}",
            self.base_url,
            q.page.max(1),
            q.page_size.clamp(1, 100),
            urlencode(&q.sort)
        );
        let mut filters = serde_json::Map::new();
        if let Some(t) = q.artifact_type.as_deref().filter(|s| !s.is_empty()) {
            filters.insert("type".into(), serde_json::Value::String(t.to_string()));
        }
        if let Some(o) = q.ownership_type.as_deref().filter(|s| !s.is_empty()) {
            filters.insert(
                "ownership_type".into(),
                serde_json::Value::String(o.to_string()),
            );
        }
        if let Some(n) = q.name_search.as_deref().filter(|s| !s.is_empty()) {
            filters.insert(
                "name_search".into(),
                serde_json::Value::String(n.to_string()),
            );
        }
        if !filters.is_empty() {
            let json = serde_json::Value::Object(filters);
            url.push_str(&format!("&filters={}", urlencode(&json.to_string())));
        }
        url
    }

    /// 新建产物（`POST /api/v1/artifacts/create`，引擎返回 201）。
    /// fd-server 的「存 Artifacts」走这条，产物才会出现在落地页列表里。
    pub async fn create(&self, n: &NewArtifact) -> anyhow::Result<ArtifactSummary> {
        let mut body = serde_json::json!({
            "session_id": n.session_id,
            "name": n.name,
            "type": n.artifact_type,
            "content": n.content,
        });
        if let Some(k) = n.kind.as_deref().filter(|s| !s.is_empty()) {
            body["kind"] = serde_json::Value::String(k.to_string());
        }
        if let Some(s) = n.summary.as_deref().filter(|s| !s.is_empty()) {
            body["summary"] = serde_json::Value::String(s.to_string());
        }
        if let Some(m) = &n.metadata {
            body["metadata"] = m.clone();
        }
        let mut req = self
            .http
            .post(format!("{}/api/v1/artifacts/create", self.base_url))
            .header("Content-Type", "application/json")
            .header("X-API-Key", &self.api_key);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = req.json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!(
                "artifacts-engine 新建失败 status={} body={}",
                status.as_u16(),
                text.chars().take(160).collect::<String>()
            );
        }
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("artifacts-engine 新建响应非 JSON: {e}"))?;
        let artifact = parsed
            .get("artifact")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("artifacts-engine 新建响应缺 artifact"))?;
        let summary: ArtifactSummary = serde_json::from_value(artifact)?;
        tracing::info!(id = %summary.id, name = %summary.name, "artifacts-engine 新建成功");
        Ok(summary)
    }

    pub async fn list(&self, q: &ArtifactQuery) -> anyhow::Result<ArtifactPage> {
        let url = self.build_url(q);
        let mut req = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .header("X-API-Key", &self.api_key);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "artifacts-engine 列表失败 status={} body={}",
                status.as_u16(),
                body.chars().take(160).collect::<String>()
            );
        }
        let parsed: EngineListResponse = resp.json().await?;
        tracing::debug!(
            page = q.page,
            page_size = q.page_size,
            got = parsed.artifacts.len(),
            total = parsed.total,
            "artifacts-engine 列表返回"
        );
        Ok(ArtifactPage {
            artifacts: parsed.artifacts,
            total: parsed.total,
        })
    }
}

/// 从引擎配置文件里取 security.api_key（避免为一个字段引入 yaml 依赖）。
/// 只做「首个 api_key: 行」扫描；admin_key / 注释行不误取。
fn read_engine_api_key() -> String {
    let path = std::env::var("FUSION_ARTIFACTS_CONFIG")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".fusion/artifacts/config.yaml"))
        });
    let Some(path) = path else {
        return String::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        tracing::debug!(path = %path.display(), "artifacts 配置文件不可读，无 API key");
        return String::new();
    };
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix("api_key:") {
            let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
            if !v.is_empty() {
                tracing::debug!(path = %path.display(), "读入 artifacts-engine API key");
                return v.to_string();
            }
        }
    }
    String::new()
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> ArtifactsClient {
        ArtifactsClient::new("http://127.0.0.1:11451", "k").expect("client")
    }

    #[test]
    fn url_encodes_filters_and_paging() {
        let q = ArtifactQuery {
            page: 2,
            page_size: 10,
            sort: "updated_at".into(),
            artifact_type: Some("html".into()),
            ownership_type: None,
            name_search: Some("登录 页".into()),
        };
        let url = client().build_url(&q);
        assert!(url.starts_with("http://127.0.0.1:11451/api/v1/artifacts?page=2&page_size=10"));
        assert!(url.contains("sort=updated_at"));
        assert!(url.contains("filters="));
        assert!(!url.contains(' '), "查询串须无裸空格: {url}");
        assert!(url.contains("%22type%22"), "filters 须 URL 编码后下发");
    }

    #[test]
    fn url_omits_empty_filters() {
        let url = client().build_url(&ArtifactQuery::default());
        assert!(!url.contains("filters="), "无过滤条件不下发 filters");
        assert!(url.contains("page_size=24"));
    }

    #[test]
    fn page_size_is_clamped() {
        let q = ArtifactQuery {
            page_size: 5000,
            ..ArtifactQuery::default()
        };
        assert!(client().build_url(&q).contains("page_size=100"));
    }

    #[test]
    fn non_local_endpoint_rejected() {
        let err = ArtifactsClient::new("http://example.com:11451", "k").expect_err("公网须拒");
        assert!(err.to_string().contains("离线硬约束"), "err={err}");
        assert!(ArtifactsClient::new("http://192.168.1.9:11451", "k").is_ok());
    }

    #[tokio::test]
    async fn unreachable_engine_returns_error() {
        // 端口 1 无监听：连接立刻拒绝，验证降级路径不会挂起。
        let c = ArtifactsClient::new("http://127.0.0.1:1", "k").expect("client");
        assert!(c.list(&ArtifactQuery::default()).await.is_err());
    }

    #[test]
    fn summary_deserializes_engine_shape() {
        let raw = r#"{"artifacts":[{"id":"art_1","name":"登录页","type":"html","kind":"app",
            "summary":"s","current_version":2,"created_at":1.5,"updated_at":2.5,
            "is_starred":true,"is_pinned":false,"share_id":null,
            "metadata":{"x":1},"owner_user_id":null}],"total":7}"#;
        let parsed: EngineListResponse = serde_json::from_str(raw).expect("parse");
        assert_eq!(parsed.total, 7);
        assert_eq!(parsed.artifacts[0].artifact_type, "html");
        assert_eq!(parsed.artifacts[0].current_version, 2);
        assert!(parsed.artifacts[0].is_starred);
        assert!(parsed.artifacts[0].share_id.is_none());
    }
}
