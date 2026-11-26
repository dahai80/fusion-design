//! fusion-artifacts-engine REST client (data source for the Artifacts landing page).
//!
//! Read-only consumption of the local `/api/v1/artifacts` list endpoint at `127.0.0.1:11451`:
//! the 100% offline constraint still holds (validate_localhost enforces it; public endpoints are
//! rejected outright). Auth dual-channel aligned with the engine: `X-API-Key` (env
//! `FUSION_ARTIFACTS_API_KEY` or `security.api_key` in `~/.fusion/artifacts/config.yaml`) and
//! `Authorization: Bearer` (same key).
//!
//! Degradation contract: when the engine is unreachable or no key is configured, return Err; the
//! caller (fd-server) converts that into an "empty list + degraded" response, never a 500 that
//! breaks the landing page.

use std::time::Duration;

use serde::{Deserialize, Serialize};

const DEFAULT_ARTIFACTS_URL: &str = "http://127.0.0.1:11451";

/// List query parameters. Field names align with the engine's filters; unset fields are not sent.
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

/// Field subset needed for a single landing-page card (extra fields beyond the engine's
/// model_dump are ignored).
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

/// Input for creating an artifact (REST shape of the engine's `artifact.create`).
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

/// Engine client. Rebuilt per request (the reqwest connection pool is recreated with it) — the
/// landing page has very low concurrency, traded for immediate effect after env/config hot changes.
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

    /// env `FUSION_ARTIFACTS_URL` / `FUSION_ARTIFACTS_API_KEY`; when the key is absent it falls
    /// back to the engine's own config file (`FUSION_ARTIFACTS_CONFIG` > `~/.fusion/artifacts/config.yaml`).
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

    /// Query-string construction: filters are sent as JSON (parsed by the engine's `_parse_json_query`).
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

    /// Creates an artifact (`POST /api/v1/artifacts/create`, the engine returns 201).
    /// fd-server's "save to Artifacts" goes through this so the artifact shows up in the
    /// landing-page list.
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
                "artifacts-engine create failed status={} body={}",
                status.as_u16(),
                text.chars().take(160).collect::<String>()
            );
        }
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("artifacts-engine create response is not JSON: {e}"))?;
        let artifact = parsed
            .get("artifact")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("artifacts-engine create response missing artifact"))?;
        let summary: ArtifactSummary = serde_json::from_value(artifact)?;
        tracing::info!(id = %summary.id, name = %summary.name, "artifacts-engine create succeeded");
        Ok(summary)
    }

    /// Metadata for a single artifact (viewer page header).
    pub async fn get(&self, id: &str) -> anyhow::Result<ArtifactSummary> {
        let parsed = self
            .get_json(&format!("/api/v1/artifacts/{}", urlencode(id)))
            .await?;
        let artifact = parsed
            .get("artifact")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("artifacts-engine response missing artifact"))?;
        Ok(serde_json::from_value(artifact)?)
    }

    /// Content of a given version (viewer content). When version is None, the latest is fetched.
    pub async fn content(&self, id: &str, version: Option<u32>) -> anyhow::Result<String> {
        let n = match version {
            Some(v) => v,
            None => self.get(id).await?.current_version.max(1),
        };
        let parsed = self
            .get_json(&format!(
                "/api/v1/artifacts/{}/versions/{}",
                urlencode(id),
                n
            ))
            .await?;
        let content = parsed
            .get("version")
            .and_then(|v| v.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| anyhow::anyhow!("artifacts-engine version response missing content"))?;
        Ok(content.to_string())
    }

    async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let mut req = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .header("Accept", "application/json")
            .header("X-API-Key", &self.api_key);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!(
                "artifacts-engine {} failed status={} body={}",
                path,
                status.as_u16(),
                text.chars().take(160).collect::<String>()
            );
        }
        serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("artifacts-engine response is not JSON: {e}"))
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
                "artifacts-engine list failed status={} body={}",
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
            "artifacts-engine list returned"
        );
        Ok(ArtifactPage {
            artifacts: parsed.artifacts,
            total: parsed.total,
        })
    }
}

/// Reads security.api_key from the engine config file (avoids pulling in a yaml dependency for a
/// single field). Only scans for the first `api_key:` line; admin_key / comment lines are not
/// mistaken for it.
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
        tracing::debug!(path = %path.display(), "artifacts config file unreadable, no API key");
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
                tracing::debug!(path = %path.display(), "read artifacts-engine API key");
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
            // i18n: user-facing
            name_search: Some("登录 页".into()),
        };
        let url = client().build_url(&q);
        assert!(url.starts_with("http://127.0.0.1:11451/api/v1/artifacts?page=2&page_size=10"));
        assert!(url.contains("sort=updated_at"));
        assert!(url.contains("filters="));
        assert!(
            !url.contains(' '),
            "query string must contain no bare spaces: {url}"
        );
        assert!(
            url.contains("%22type%22"),
            "filters must be URL-encoded when sent"
        );
    }

    #[test]
    fn url_omits_empty_filters() {
        let url = client().build_url(&ArtifactQuery::default());
        assert!(
            !url.contains("filters="),
            "filters must not be sent when there are no filters"
        );
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
        let err = ArtifactsClient::new("http://example.com:11451", "k")
            .expect_err("public endpoint must be rejected");
        assert!(
            err.to_string().contains("offline hard constraint"),
            "err={err}"
        );
        assert!(ArtifactsClient::new("http://192.168.1.9:11451", "k").is_ok());
    }

    #[tokio::test]
    async fn unreachable_engine_returns_error() {
        // Port 1 has no listener: the connection is refused immediately, verifying the
        // degradation path never hangs.
        let c = ArtifactsClient::new("http://127.0.0.1:1", "k").expect("client");
        assert!(c.list(&ArtifactQuery::default()).await.is_err());
    }

    #[test]
    fn summary_deserializes_engine_shape() {
        // i18n: user-facing
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
