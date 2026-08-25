//! Fusion-Design 生态联动 — 对接 Fusion Code/Simulation/KB/CLI。
//!
//! 对应 PRD 模块 6「生态联动能力」。通过本地文件 IPC（约定目录下的
//! JSON 消息文件）打通全系生态：send/list/consume、sync_to_code、模板检索/预置。
//!
//! 【H-A3/P2-2】自建 MCP 协议层 + watch_async 已移除（零外部调用，死代码）。
//!
//! 【离线硬约束】所有联动走本地文件系统或 127.0.0.1，无公网调用。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

// 进程级 IPC 消息序号，配合 pid + nanos 保证文件名全局唯一，避免同纳秒碰撞（审计 C7）。
static IPC_SEQ: AtomicU64 = AtomicU64::new(0);

/// 生态联动目标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EcosystemTarget {
    /// Fusion Code（正向导出代码、反向同步样式）
    FusionCode,
    /// Fusion-KB（保存/检索设计模板）
    FusionKB,
    /// Fusion CLI（命令行批量生成/导出）
    FusionCLI,
    /// 扩展目标（插件/垂直领域通过此变体接入，不硬编码具体行业）
    Custom(String),
}

impl EcosystemTarget {
    /// 约定的 IPC 目录名。
    pub fn ipc_dir(&self) -> String {
        match self {
            Self::FusionCode => "fusion-code".to_string(),
            Self::FusionKB => "fusion-kb".to_string(),
            Self::FusionCLI => "fusion-cli".to_string(),
            Self::Custom(name) => name.clone(),
        }
    }
}

// 将 base + sub 解析为安全子路径：拒绝 `..` 组件与绝对路径，并校验结果仍在 base 之内。
// 用于 send() 防 EcosystemTarget::Custom 携带 `../` 实施路径穿越写（审计 C6）。
fn sanitize_ipc_subpath(base: &Path, sub: &str) -> anyhow::Result<PathBuf> {
    if sub.is_empty() {
        anyhow::bail!("IPC 子路径为空");
    }
    let sub_path = Path::new(sub);
    if sub_path.is_absolute() {
        tracing::warn!(sub = %sub, "IPC 子路径为绝对路径，拒绝");
        anyhow::bail!("IPC 子路径不允许绝对路径: {sub}");
    }
    for comp in sub_path.components() {
        use std::path::Component;
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                tracing::warn!(sub = %sub, "IPC 子路径含 `..` 组件，拒绝");
                anyhow::bail!("IPC 子路径不允许 `..` 组件: {sub}");
            }
            Component::RootDir | Component::Prefix(_) => {
                tracing::warn!(sub = %sub, "IPC 子路径含根/前缀组件，拒绝");
                anyhow::bail!("IPC 子路径不允许根或前缀组件: {sub}");
            }
        }
    }
    let joined = base.join(sub_path);
    // 二次校验：若 base/joined 均可规范化（已存在），比较规范化路径以防符号链接绕过；
    // 否则退化为词法 starts_with——组件已过滤掉 `..`/绝对路径，词法校验即充分。
    // 不返回规范化路径，保持与 list()/调用方一致的路径形态。
    let canonical_base = base.canonicalize();
    let canonical_joined = joined.canonicalize();
    let within = match (canonical_base, canonical_joined) {
        (Ok(cb), Ok(cj)) => cj.starts_with(&cb),
        _ => joined.starts_with(base),
    };
    if !within {
        tracing::warn!(
            base = %base.display(),
            resolved = %joined.display(),
            "IPC 子路径解析后逃出 base，拒绝"
        );
        anyhow::bail!("IPC 子路径逃出基目录: {sub}");
    }
    Ok(joined)
}

/// 联动消息体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkMessage {
    pub target: EcosystemTarget,
    pub action: String,
    pub payload: serde_json::Value,
}

/// fusion-trainer CLI 子进程封装（离线硬约束：仅调用本地 .venv 的 fusion-trainer）。
#[derive(Debug, Clone)]
pub struct TrainerClient {
    bin: PathBuf,
}

impl TrainerClient {
    /// 解析 fusion-trainer 可执行路径：优先 FUSION_TRAINER_BIN，否则共享 .venv 默认值。
    pub fn resolve_bin() -> PathBuf {
        if let Ok(b) = std::env::var("FUSION_TRAINER_BIN") {
            return PathBuf::from(b);
        }
        PathBuf::from("/Users/dahai/fusion/.venv/bin/fusion-trainer")
    }

    pub fn new() -> Self {
        Self {
            bin: Self::resolve_bin(),
        }
    }

    pub fn with_bin(bin: impl Into<PathBuf>) -> Self {
        Self { bin: bin.into() }
    }

    fn spawn(&self, args: Vec<String>) -> anyhow::Result<std::process::ExitStatus> {
        if !self.bin.exists() {
            anyhow::bail!(
                "fusion-trainer CLI 未找到: {} (请安装 fusion-trainer 或设置 FUSION_TRAINER_BIN)",
                self.bin.display()
            );
        }
        tracing::info!(bin = %self.bin.display(), args = ?args, "spawn fusion-trainer");
        let status = std::process::Command::new(&self.bin)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()?;
        Ok(status)
    }

    /// 调用 fusion-trainer sft --dataset <jsonl> --model <id>
    pub fn run_sft(
        &self,
        dataset: &Path,
        model: &str,
        config: Option<&Path>,
    ) -> anyhow::Result<std::process::ExitStatus> {
        let mut args = vec![
            "sft".into(),
            "--dataset".into(),
            dataset.display().to_string(),
            "--model".into(),
            model.into(),
        ];
        if let Some(c) = config {
            args.push("--config".into());
            args.push(c.display().to_string());
        }
        self.spawn(args)
    }

    /// 调用 fusion-trainer rlsl --method <m> --dataset <jsonl> --model <id>
    pub fn run_rlsl(
        &self,
        method: &str,
        dataset: &Path,
        model: &str,
        config: Option<&Path>,
    ) -> anyhow::Result<std::process::ExitStatus> {
        let mut args = vec![
            "rlsl".into(),
            "--method".into(),
            method.into(),
            "--dataset".into(),
            dataset.display().to_string(),
            "--model".into(),
            model.into(),
        ];
        if let Some(c) = config {
            args.push("--config".into());
            args.push(c.display().to_string());
        }
        self.spawn(args)
    }
}

impl Default for TrainerClient {
    fn default() -> Self {
        Self::new()
    }
}

/// E-16/P2-6：模板检索缓存。按模板目录 mtime 失效——save_template 写文件后
/// 目录 mtime 变化，下次检索重新扫盘；未变则复用内存解析结果，零磁盘 IO。
/// 旧实现每次检索 read_dir + 逐文件 read_to_string + from_str，N 模板线性退化。
#[derive(Debug, Clone, Default)]
struct TemplateCache {
    /// 缓存对应的模板目录 mtime（秒级，跨秒变化即失效）。
    dir_mtime_secs: u64,
    /// 已解析的全部模板（按 created_at 降序）。
    templates: Vec<DesignTemplate>,
}

/// 生态联动客户端（基于本地文件 IPC）。
// E-16/P2-6：含 Mutex 缓存，不再 derive Clone（Mutex 非 Clone；全仓无 EcosystemLink::clone 调用）。
#[derive(Debug)]
pub struct EcosystemLink {
    base_dir: PathBuf,
    // E-16/P2-6：模板检索缓存，Mutex 包裹供多线程安全复用。
    template_cache: Mutex<TemplateCache>,
}

impl EcosystemLink {
    /// 用指定基目录构造（所有 IPC 文件落在 `<base>/<target>/`）。
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            template_cache: Mutex::new(TemplateCache::default()),
        }
    }

    /// 发送一条联动消息（写入目标目录的 JSON 文件）。
    pub fn send(&self, msg: &LinkMessage) -> anyhow::Result<PathBuf> {
        let dir = sanitize_ipc_subpath(&self.base_dir, &msg.target.ipc_dir())?;
        std::fs::create_dir_all(&dir)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let pid = std::process::id();
        let seq = IPC_SEQ.fetch_add(1, Ordering::SeqCst);
        let file = dir.join(format!("{stamp}_{pid}_{seq}.json"));
        let json = serde_json::to_string_pretty(msg)?;
        // 原子写：先写 tmp 再 rename，避免崩溃留下半截 JSON（审计 C7）。
        let tmp = dir.join(format!("{stamp}_{pid}_{seq}.json.tmp.{pid}.{seq}"));
        std::fs::write(&tmp, &json)?;
        if let Err(e) = std::fs::rename(&tmp, &file) {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!(?tmp, error = %e, "send: rename 失败，清理 tmp");
            return Err(anyhow::Error::from(e));
        }
        tracing::info!(?file, target = msg.target.ipc_dir(), "联动消息已发送");
        Ok(file)
    }

    /// 列出某目标的全部待处理消息文件。
    pub fn list(&self, target: EcosystemTarget) -> anyhow::Result<Vec<PathBuf>> {
        let dir = self.base_dir.join(target.ipc_dir());
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        files.sort();
        Ok(files)
    }

    /// 读取并删除一条消息（消费式）。含大小护栏，防恶意大文件 OOM。
    pub fn consume(&self, file: &Path) -> anyhow::Result<LinkMessage> {
        let meta = std::fs::metadata(file)?;
        const MAX_IPC_FILE: u64 = 8 * 1024 * 1024;
        if meta.len() > MAX_IPC_FILE {
            tracing::warn!(size = meta.len(), "IPC 消息文件超过 8MB 上限，拒绝读取");
            anyhow::bail!("IPC 消息文件 {} 超过 8MB 安全上限", file.display());
        }
        let json = std::fs::read_to_string(file)?;
        let msg: LinkMessage = serde_json::from_str(&json)?;
        std::fs::remove_file(file)?;
        Ok(msg)
    }

    /// 异步发送（供任务队列调用）。
    pub async fn send_async(&self, msg: LinkMessage) -> anyhow::Result<PathBuf> {
        let base = self.base_dir.clone();
        tokio::task::spawn_blocking(move || EcosystemLink::new(base).send(&msg)).await?
    }

    /// 正向同步：PenDocument → fd-codegen 生成代码 → 写入 Fusion Code 工程目录。
    ///
    /// 生成 HTML 和 React+Tailwind 两种格式，写入 `<base>/fusion-code/` 目录。
    pub fn sync_to_code(
        &self,
        doc: &fd_canvas_core::PenDocument,
        component_name: &str,
    ) -> anyhow::Result<Vec<PathBuf>> {
        let code_dir = self.base_dir.join(EcosystemTarget::FusionCode.ipc_dir());
        std::fs::create_dir_all(&code_dir)?;

        let mut written = vec![];

        // 生成 HTML
        let html_gen = fd_codegen::HtmlCodegen;
        let html = fd_codegen::Codegen::generate(&html_gen, doc);
        let html_path = code_dir.join(format!("{component_name}.html"));
        std::fs::write(&html_path, &html)?;
        tracing::info!(?html_path, "sync_to_code: HTML 写入完成");
        written.push(html_path);

        // 生成 React + Tailwind
        let react_gen = fd_codegen::ReactTailwindCodegen {
            component_name: component_name.to_string(),
        };
        let react = fd_codegen::Codegen::generate(&react_gen, doc);
        let react_path = code_dir.join(format!("{component_name}.tsx"));
        std::fs::write(&react_path, &react)?;
        tracing::info!(?react_path, "sync_to_code: React+Tailwind 写入完成");
        written.push(react_path);

        // 同时发送 IPC 消息通知 Fusion Code
        let msg = LinkMessage {
            target: EcosystemTarget::FusionCode,
            action: "sync-to-code".into(),
            payload: serde_json::json!({
                "component": component_name,
                "files": written.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
            }),
        };
        self.send(&msg)?;

        Ok(written)
    }

    /// 异步正向同步。
    pub async fn sync_to_code_async(
        &self,
        doc: fd_canvas_core::PenDocument,
        component_name: String,
    ) -> anyhow::Result<Vec<PathBuf>> {
        let base = self.base_dir.clone();
        tokio::task::spawn_blocking(move || {
            EcosystemLink::new(base).sync_to_code(&doc, &component_name)
        })
        .await?
    }

    /// 反向监听：扫描 Fusion Code 目录的 IPC 消息，提取样式变更。
    ///
    /// 消费所有待处理消息，过滤 action="style-change"，返回 MutateNodes 指令列表。
    /// 调用方负责将 MutateNodes 发往画布。
    pub fn watch_code_changes(&self) -> anyhow::Result<Vec<MutateNodeCommand>> {
        let messages = self.list(EcosystemTarget::FusionCode)?;
        let mut commands = vec![];
        for file in &messages {
            if let Ok(msg) = self.consume(file) {
                if msg.action == "style-change" {
                    if let Some(arr) = msg.payload.get("mutations").and_then(|v| v.as_array()) {
                        // E-18/P3：旧实现对 parse_mutate_command 返回 None 静默丢弃，
                        // 解析失败的 mutation 项无任何告警，调试无从定位。统计并显式告警。
                        let total = arr.len();
                        let mut dropped = 0usize;
                        for item in arr {
                            match parse_mutate_command(item) {
                                Some(cmd) => commands.push(cmd),
                                None => {
                                    dropped += 1;
                                    tracing::warn!(
                                        item = %item,
                                        "watch_code_changes: mutation 解析失败被丢弃（非对象或缺 node_id）"
                                    );
                                }
                            }
                        }
                        if dropped > 0 {
                            tracing::warn!(
                                total,
                                dropped,
                                "watch_code_changes: 本消息丢弃 {dropped}/{total} 个 mutation"
                            );
                        }
                    }
                }
            }
        }
        if !commands.is_empty() {
            tracing::info!(count = commands.len(), "watch_code_changes: 发现样式变更");
        } else {
            tracing::debug!("watch_code_changes: 无有效样式变更指令");
        }
        Ok(commands)
    }
}

/// MutateNode 指令（从 Fusion Code 反向同步的样式变更）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutateNodeCommand {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
}

fn parse_mutate_command(v: &serde_json::Value) -> Option<MutateNodeCommand> {
    let obj = v.as_object()?;
    let node_id = obj.get("node_id")?.as_str()?.to_string();
    Some(MutateNodeCommand {
        node_id,
        x: obj.get("x").and_then(|v| v.as_f64()).map(|v| v as f32),
        y: obj.get("y").and_then(|v| v.as_f64()).map(|v| v as f32),
        w: obj.get("w").and_then(|v| v.as_f64()).map(|v| v as f32),
        h: obj.get("h").and_then(|v| v.as_f64()).map(|v| v as f32),
        fill: obj.get("fill").and_then(|v| v.as_str()).map(String::from),
        stroke: obj.get("stroke").and_then(|v| v.as_str()).map(String::from),
        radius: obj.get("radius").and_then(|v| v.as_f64()).map(|v| v as f32),
        opacity: obj
            .get("opacity")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32),
    })
}

// ── KB 模板系统 ──

/// 设计模板元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignTemplate {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub category: String,
    pub document_json: String,
    #[serde(default = "default_timestamp")]
    pub created_at: u64,
}

fn default_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl EcosystemLink {
    /// 保存设计模板到 Fusion-KB 目录。
    pub fn save_template(&self, tmpl: &DesignTemplate) -> anyhow::Result<PathBuf> {
        let kb_dir = self
            .base_dir
            .join(EcosystemTarget::FusionKB.ipc_dir())
            .join("templates");
        std::fs::create_dir_all(&kb_dir)?;
        let file = kb_dir.join(format!("{}.json", tmpl.id));
        let json = serde_json::to_string_pretty(tmpl)?;
        std::fs::write(&file, json)?;
        tracing::info!(?file, id = %tmpl.id, "save_template: 模板已保存");
        // E-16/P2-6：写入即失效缓存（mtime 秒级精度，同秒连续写可能漏判，显式清空更稳）。
        if let Ok(mut c) = self.template_cache.lock() {
            *c = TemplateCache::default();
        }
        // 同时发 IPC 消息通知
        let msg = LinkMessage {
            target: EcosystemTarget::FusionKB,
            action: "template-saved".into(),
            payload: serde_json::json!({
                "id": tmpl.id,
                "name": tmpl.name,
                "tags": tmpl.tags,
            }),
        };
        self.send(&msg)?;
        Ok(file)
    }

    /// E-16/P2-6：返回（降序）全部模板，命中缓存则零磁盘 IO。
    /// 缓存按模板目录 mtime（秒级）失效：save_template 写文件 → mtime 变 → 重扫。
    /// 旧实现每次检索都 read_dir + 逐文件 read_to_string + from_str，N 模板线性退化。
    fn load_all_templates_cached(&self) -> anyhow::Result<Vec<DesignTemplate>> {
        let kb_dir = self
            .base_dir
            .join(EcosystemTarget::FusionKB.ipc_dir())
            .join("templates");
        if !kb_dir.exists() {
            // 目录不存在：清缓存，返回空。
            if let Ok(mut c) = self.template_cache.lock() {
                *c = TemplateCache::default();
            }
            return Ok(vec![]);
        }
        // 目录 mtime（秒级）作缓存键。失败（权限等）则强制重扫，不信任旧缓存。
        let mtime_secs = std::fs::metadata(&kb_dir)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Ok(c) = self.template_cache.lock() {
            if c.dir_mtime_secs == mtime_secs && mtime_secs != 0 {
                tracing::debug!(
                    dir_mtime = mtime_secs,
                    count = c.templates.len(),
                    "load_all_templates_cached: 缓存命中（零磁盘 IO）"
                );
                return Ok(c.templates.clone());
            }
        }
        // 未命中：扫盘解析。
        let mut templates = vec![];
        for entry in std::fs::read_dir(&kb_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(data) = std::fs::read_to_string(&path) {
                if let Ok(tmpl) = serde_json::from_str::<DesignTemplate>(&data) {
                    templates.push(tmpl);
                }
            }
        }
        templates.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        tracing::info!(
            dir_mtime = mtime_secs,
            count = templates.len(),
            "load_all_templates_cached: 缓存未命中，已扫盘填充"
        );
        if let Ok(mut c) = self.template_cache.lock() {
            c.dir_mtime_secs = mtime_secs;
            c.templates = templates.clone();
        }
        Ok(templates)
    }

    /// 检索模板：按关键词匹配 name/tags/category。
    pub fn search_templates(&self, query: &str) -> anyhow::Result<Vec<DesignTemplate>> {
        let all = self.load_all_templates_cached()?;
        let q = query.to_lowercase();
        // E-16/P2-6：内存过滤，无磁盘 IO（all 已降序，保序过滤即可）。
        let results: Vec<DesignTemplate> = all
            .iter()
            .filter(|tmpl| {
                if q.is_empty() {
                    return true;
                }
                let name_match = tmpl.name.to_lowercase().contains(&q);
                let tag_match = tmpl.tags.iter().any(|t| t.to_lowercase().contains(&q));
                let cat_match = tmpl.category.to_lowercase().contains(&q);
                name_match || tag_match || cat_match
            })
            .cloned()
            .collect();
        tracing::info!(query = %query, count = results.len(), "search_templates: 检索完成");
        Ok(results)
    }

    /// 按 tag 精确匹配检索模板（多 tag 取交集）。
    pub fn search_templates_by_tags(&self, tags: &[String]) -> anyhow::Result<Vec<DesignTemplate>> {
        let all = self.load_all_templates_cached()?;
        let lower_tags: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();
        // E-16/P2-6：内存过滤，无磁盘 IO。
        let results: Vec<DesignTemplate> = all
            .iter()
            .filter(|tmpl| {
                let tmpl_tags_lower: Vec<String> =
                    tmpl.tags.iter().map(|t| t.to_lowercase()).collect();
                lower_tags
                    .iter()
                    .all(|t| tmpl_tags_lower.iter().any(|tt| tt == t))
            })
            .cloned()
            .collect();
        tracing::info!(tags = ?tags, count = results.len(), "search_templates_by_tags: 检索完成");
        Ok(results)
    }
}

// H-A3/P2-2：watch_async（异步文件监听，notify）+ 自建 MCP 协议层（McpServer/
// McpTool/ListTargetsTool/SearchTemplatesTool/SendMessageTool + WatchEvent/Kind）
// 已移除——零外部调用，展示性死代码。
// MCP 层不合规（仅 initialize/tools 列表/call 三方法，缺握手/resources/progress，
// 无法被标准 MCP client 接入，见审计 H-A4）；watch_async 无消费方。
// EcosystemLink IPC 核心（send/list/consume/sync_to_code/search_templates/templates）保留。
// 如需恢复，见 git 历史。

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn send_and_list_and_consume() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let msg = LinkMessage {
            target: EcosystemTarget::FusionCode,
            action: "export-code".into(),
            payload: serde_json::json!({"page": "login"}),
        };
        let file = link.send(&msg).unwrap();
        let listed = link.list(EcosystemTarget::FusionCode).unwrap();
        assert_eq!(listed, vec![file.clone()]);
        let consumed = link.consume(&file).unwrap();
        assert_eq!(consumed.action, "export-code");
        // consume 后文件删除
        assert!(link.list(EcosystemTarget::FusionCode).unwrap().is_empty());
    }

    #[test]
    fn list_empty_when_dir_absent() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        assert!(link.list(EcosystemTarget::FusionKB).unwrap().is_empty());
    }

    #[test]
    fn ipc_dir_names() {
        assert_eq!(EcosystemTarget::FusionCode.ipc_dir(), "fusion-code");
        assert_eq!(EcosystemTarget::FusionKB.ipc_dir(), "fusion-kb");
        assert_eq!(
            EcosystemTarget::Custom("fusion-simulation".into()).ipc_dir(),
            "fusion-simulation"
        );
    }

    #[test]
    fn send_rejects_path_traversal_custom_target() {
        // 审计 C6：Custom("../evil") 必须被拒绝，不得逃出 base 目录。
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let msg = LinkMessage {
            target: EcosystemTarget::Custom("../evil".into()),
            action: "traversal".into(),
            payload: serde_json::json!({}),
        };
        let err = link.send(&msg).unwrap_err();
        assert!(
            format!("{}", err).contains(".."),
            "错误信息应提及 `..` 组件，实际: {err}"
        );
        // 确认没有文件被写到 base 之外
        assert!(std::fs::read_dir(tmp.path()).unwrap().count() == 0);
    }

    #[test]
    fn send_filename_contains_pid_and_seq() {
        // 审计 C7：文件名应含 pid + seq，避免同纳秒碰撞。
        // 静态计数器是进程级，不假定具体 seq 值，只校验 <stamp>_<pid>_<seq>.json 形态。
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let msg = LinkMessage {
            target: EcosystemTarget::FusionCode,
            action: "check".into(),
            payload: serde_json::json!({}),
        };
        let file = link.send(&msg).unwrap();
        let fname = file.file_name().unwrap().to_string_lossy().to_string();
        let pid = std::process::id().to_string();
        assert!(fname.contains(&pid), "文件名应含 pid={pid}，实际: {fname}");
        // 形态：<数字纳秒>_<pid>_<数字seq>.json
        let stem = fname
            .strip_suffix(".json")
            .unwrap_or_else(|| panic!("文件名应以 .json 结尾: {fname}"));
        let parts: Vec<&str> = stem.rsplitn(3, '_').collect();
        // rsplitn 产出顺序：seq, pid, stamp（从右往左切 3 段）
        assert_eq!(parts.len(), 3, "文件名应有 stamp_pid_seq 三段: {fname}");
        assert!(parts[0].parse::<u64>().is_ok(), "seq 应为数字: {fname}");
        assert_eq!(parts[1], pid, "pid 段应匹配当前进程: {fname}");
        assert!(parts[2].parse::<u128>().is_ok(), "stamp 应为数字: {fname}");
    }

    #[test]
    fn send_concurrent_does_not_collide() {
        // 审计 C7：连续两次 send 应产生不同文件名（seq 递增）。
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let mk = || LinkMessage {
            target: EcosystemTarget::FusionCLI,
            action: "burst".into(),
            payload: serde_json::json!({}),
        };
        let f1 = link.send(&mk()).unwrap();
        let f2 = link.send(&mk()).unwrap();
        assert_ne!(f1, f2, "两次 send 不应碰撞到同一文件名");
        assert!(f1.exists());
        assert!(f2.exists());
    }

    #[tokio::test]
    async fn send_async_works() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let msg = LinkMessage {
            target: EcosystemTarget::FusionCLI,
            action: "batch-export".into(),
            payload: serde_json::json!({}),
        };
        let file = link.send_async(msg).await.unwrap();
        assert!(file.exists());
    }

    #[test]
    fn sync_to_code_writes_html_and_tsx() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let mut doc = fd_canvas_core::PenDocument::new();
        let mut page = fd_canvas_core::Page::new("p1", "Home", 100.0, 100.0);
        page.add(fd_canvas_core::PenNode::rect("n1", 0.0, 0.0, 50.0, 30.0));
        doc.add_page(page);
        let paths = link.sync_to_code(&doc, "Home").unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths[0].to_string_lossy().ends_with("Home.html"));
        assert!(paths[1].to_string_lossy().ends_with("Home.tsx"));
        let html = std::fs::read_to_string(&paths[0]).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        let tsx = std::fs::read_to_string(&paths[1]).unwrap();
        assert!(tsx.contains("export function Home()"));
    }

    #[tokio::test]
    async fn sync_to_code_async_works() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let mut doc = fd_canvas_core::PenDocument::new();
        let mut page = fd_canvas_core::Page::new("p", "Test", 100.0, 100.0);
        page.add(fd_canvas_core::PenNode::rect("r", 0.0, 0.0, 10.0, 10.0));
        doc.add_page(page);
        let paths = link.sync_to_code_async(doc, "Test".into()).await.unwrap();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn watch_code_changes_extracts_mutations() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        // 放一条 style-change 消息
        let msg = LinkMessage {
            target: EcosystemTarget::FusionCode,
            action: "style-change".into(),
            payload: serde_json::json!({
                "mutations": [
                    { "node_id": "n1", "fill": "#ff0000", "w": 200.0 },
                    { "node_id": "n2", "stroke": "#000", "opacity": 0.5 },
                ]
            }),
        };
        link.send(&msg).unwrap();
        // 放一条无关消息
        let noise = LinkMessage {
            target: EcosystemTarget::FusionCode,
            action: "other-action".into(),
            payload: serde_json::json!({}),
        };
        link.send(&noise).unwrap();

        let cmds = link.watch_code_changes().unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].node_id, "n1");
        assert_eq!(cmds[0].fill.as_deref(), Some("#ff0000"));
        assert_eq!(cmds[0].w, Some(200.0));
        assert_eq!(cmds[1].node_id, "n2");
        assert_eq!(cmds[1].stroke.as_deref(), Some("#000"));
        assert_eq!(cmds[1].opacity, Some(0.5));
    }

    #[test]
    fn watch_code_changes_empty_when_no_messages() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let cmds = link.watch_code_changes().unwrap();
        assert!(cmds.is_empty());
    }

    #[test]
    fn watch_code_changes_drops_invalid_mutations_keeps_valid() {
        // E-18/P3：混合有效/无效 mutation 项。旧实现静默丢全部无效项；
        // 修复后须保留有效项、丢弃无效项（非对象/缺 node_id/非数组元素），不 panic。
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let msg = LinkMessage {
            target: EcosystemTarget::FusionCode,
            action: "style-change".into(),
            payload: serde_json::json!({
                "mutations": [
                    { "node_id": "good1", "fill": "#abc" },
                    "not-an-object",
                    { "fill": "#fff" },
                    42,
                    { "node_id": "good2", "w": 100.0 },
                    null
                ]
            }),
        };
        link.send(&msg).unwrap();
        let cmds = link.watch_code_changes().unwrap();
        let ids: Vec<&str> = cmds.iter().map(|c| c.node_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["good1", "good2"],
            "仅保留有 node_id 的有效项: {ids:?}"
        );
    }

    #[test]
    fn save_and_search_template() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let tmpl = DesignTemplate {
            id: "login-v1".into(),
            name: "登录页面".into(),
            tags: vec!["login".into(), "auth".into()],
            category: "表单".into(),
            document_json: "{}".into(),
            created_at: 1000,
        };
        let path = link.save_template(&tmpl).unwrap();
        assert!(path.exists());

        // 按名称检索
        let results = link.search_templates("登录").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "login-v1");

        // 按 tag 检索
        let results = link.search_templates("auth").unwrap();
        assert_eq!(results.len(), 1);

        // 按分类检索
        let results = link.search_templates("表单").unwrap();
        assert_eq!(results.len(), 1);

        // 空查询返回全部
        let results = link.search_templates("").unwrap();
        assert_eq!(results.len(), 1);

        // 不匹配返回空
        let results = link.search_templates("dashboard").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_templates_empty_when_no_dir() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let results = link.search_templates("anything").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_templates_by_tags_intersection() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());

        let tmpl1 = DesignTemplate {
            id: "t1".into(),
            name: "Login".into(),
            tags: vec!["auth".into(), "form".into()],
            category: "表单".into(),
            document_json: "{}".into(),
            created_at: 1000,
        };
        let tmpl2 = DesignTemplate {
            id: "t2".into(),
            name: "Signup".into(),
            tags: vec!["auth".into(), "register".into()],
            category: "表单".into(),
            document_json: "{}".into(),
            created_at: 2000,
        };
        link.save_template(&tmpl1).unwrap();
        link.save_template(&tmpl2).unwrap();

        // 单 tag 匹配两个
        let results = link.search_templates_by_tags(&["auth".into()]).unwrap();
        assert_eq!(results.len(), 2);

        // 双 tag 交集只匹配 tmpl1
        let results = link
            .search_templates_by_tags(&["auth".into(), "form".into()])
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "t1");

        // 不存在的 tag
        let results = link
            .search_templates_by_tags(&["nonexistent".into()])
            .unwrap();
        assert!(results.is_empty());

        // 空 tags 返回全部
        let results = link.search_templates_by_tags(&[]).unwrap();
        assert_eq!(results.len(), 2);
    }

    // E-16/P2-6 回归：缓存命中——重复检索返回一致结果（缓存生效，无重扫）。
    #[test]
    fn template_search_cache_repeated_query_consistent() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        link.save_template(&DesignTemplate {
            id: "c1".into(),
            name: "Cache".into(),
            tags: vec!["x".into()],
            category: "cat".into(),
            document_json: "{}".into(),
            created_at: 1000,
        })
        .unwrap();
        // 多次检索，缓存填充后应命中（结果一致）。
        let r1 = link.search_templates("Cache").unwrap();
        let r2 = link.search_templates("Cache").unwrap();
        let r3 = link.search_templates_by_tags(&["x".into()]).unwrap();
        assert_eq!(r1.len(), 1);
        assert_eq!(r2.len(), 1, "缓存命中应返回一致结果");
        assert_eq!(r3.len(), 1, "by_tags 同样走缓存");
        assert_eq!(r1[0].id, "c1");
        assert_eq!(r2[0].id, "c1");
    }

    // E-16/P2-6 回归：save_template 后缓存即时失效——
    // 同秒连续写也必须让下次检索看到新模板（mtime 秒级精度漏洞的兜底）。
    #[test]
    fn template_cache_invalidated_on_save_same_second() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        link.save_template(&DesignTemplate {
            id: "s1".into(),
            name: "First".into(),
            tags: vec![],
            category: "c".into(),
            document_json: "{}".into(),
            created_at: 1000,
        })
        .unwrap();
        // 填充缓存。
        assert_eq!(link.search_templates("").unwrap().len(), 1);
        // 同秒再存一个（save_template 显式清缓存）。
        link.save_template(&DesignTemplate {
            id: "s2".into(),
            name: "Second".into(),
            tags: vec![],
            category: "c".into(),
            document_json: "{}".into(),
            created_at: 2000,
        })
        .unwrap();
        // 若缓存未失效，此处仍返回 1（旧缓存）——必须返回 2。
        let results = link.search_templates("").unwrap();
        assert_eq!(
            results.len(),
            2,
            "save 后缓存应失效，检索须见新模板（同秒写兜底）"
        );
    }

    // E-16/P2-6 回归：目录不存在时缓存被清空，不返回陈旧结果。
    #[test]
    fn template_cache_cleared_when_dir_absent() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        // 先存模板填充缓存。
        link.save_template(&DesignTemplate {
            id: "d1".into(),
            name: "D".into(),
            tags: vec![],
            category: "c".into(),
            document_json: "{}".into(),
            created_at: 1000,
        })
        .unwrap();
        assert_eq!(link.search_templates("").unwrap().len(), 1);
        // 删除整个模板目录。
        let kb_dir = tmp
            .path()
            .join(EcosystemTarget::FusionKB.ipc_dir())
            .join("templates");
        std::fs::remove_dir_all(&kb_dir).unwrap();
        // 缓存应被清空，检索返回空而非陈旧的 1。
        assert_eq!(
            link.search_templates("").unwrap().len(),
            0,
            "目录删除后缓存应清空，不返回陈旧结果"
        );
    }
}

// ── 内置场景模板预置 ──

/// 返回 4 类内置场景模板：移动端应用、B 端后台、营销网站、小程序。
pub fn builtin_scene_templates() -> Vec<DesignTemplate> {
    let templates = vec![
        // 1. 移动端应用
        DesignTemplate {
            id: "builtin-mobile-app".into(),
            name: "移动端应用".into(),
            tags: vec!["mobile".into(), "app".into(), "iOS".into(), "Android".into()],
            category: "mobile".into(),
            document_json: serde_json::json!({
                "pages": [{
                    "id": "home", "name": "首页", "width": 390.0, "height": 844.0,
                    "nodes": [
                        {"id":"nav","kind":"rect","x":0.0,"y":0.0,"w":390.0,"h":88.0,"fill":"#FFFFFF","stroke":"#E5E5E5"},
                        {"id":"title","kind":"text","x":16.0,"y":44.0,"w":200.0,"h":32.0,"text":"首页","fill":"#000000"},
                        {"id":"banner","kind":"rect","x":16.0,"y":104.0,"w":358.0,"h":180.0,"fill":"#F5F5F5","stroke":"#E0E0E0"},
                        {"id":"card1","kind":"rect","x":16.0,"y":300.0,"w":358.0,"h":100.0,"fill":"#FFFFFF","stroke":"#EEEEEE"},
                        {"id":"card2","kind":"rect","x":16.0,"y":416.0,"w":358.0,"h":100.0,"fill":"#FFFFFF","stroke":"#EEEEEE"},
                        {"id":"tab-bar","kind":"rect","x":0.0,"y":760.0,"w":390.0,"h":84.0,"fill":"#FFFFFF","stroke":"#E5E5E5"}
                    ]
                }]
            }).to_string(),
            created_at: default_timestamp(),
        },
        // 2. B 端后台
        DesignTemplate {
            id: "builtin-admin-dashboard".into(),
            name: "B端后台".into(),
            tags: vec!["admin".into(), "dashboard".into(), "B端".into(), "后台".into()],
            category: "admin".into(),
            document_json: serde_json::json!({
                "pages": [{
                    "id": "dashboard", "name": "仪表盘", "width": 1440.0, "height": 900.0,
                    "nodes": [
                        {"id":"sidebar","kind":"rect","x":0.0,"y":0.0,"w":240.0,"h":900.0,"fill":"#1A1A2E"},
                        {"id":"logo","kind":"text","x":24.0,"y":24.0,"w":180.0,"h":32.0,"text":"Admin","fill":"#FFFFFF"},
                        {"id":"nav-1","kind":"text","x":24.0,"y":80.0,"w":180.0,"h":24.0,"text":"仪表盘","fill":"#A0A0C0"},
                        {"id":"nav-2","kind":"text","x":24.0,"y":120.0,"w":180.0,"h":24.0,"text":"用户管理","fill":"#A0A0C0"},
                        {"id":"nav-3","kind":"text","x":24.0,"y":160.0,"w":180.0,"h":24.0,"text":"数据分析","fill":"#A0A0C0"},
                        {"id":"header","kind":"rect","x":240.0,"y":0.0,"w":1200.0,"h":64.0,"fill":"#FFFFFF","stroke":"#E5E5E5"},
                        {"id":"stat-card-1","kind":"rect","x":272.0,"y":96.0,"w":260.0,"h":120.0,"fill":"#FFFFFF","stroke":"#E5E5E5"},
                        {"id":"stat-card-2","kind":"rect","x":556.0,"y":96.0,"w":260.0,"h":120.0,"fill":"#FFFFFF","stroke":"#E5E5E5"},
                        {"id":"stat-card-3","kind":"rect","x":840.0,"y":96.0,"w":260.0,"h":120.0,"fill":"#FFFFFF","stroke":"#E5E5E5"},
                        {"id":"stat-card-4","kind":"rect","x":1124.0,"y":96.0,"w":260.0,"h":120.0,"fill":"#FFFFFF","stroke":"#E5E5E5"},
                        {"id":"chart-area","kind":"rect","x":272.0,"y":240.0,"w":544.0,"h":320.0,"fill":"#FFFFFF","stroke":"#E5E5E5"},
                        {"id":"table-area","kind":"rect","x":840.0,"y":240.0,"w":544.0,"h":320.0,"fill":"#FFFFFF","stroke":"#E5E5E5"}
                    ]
                }]
            }).to_string(),
            created_at: default_timestamp(),
        },
        // 3. 营销网站
        DesignTemplate {
            id: "builtin-marketing-site".into(),
            name: "营销网站".into(),
            tags: vec!["website".into(), "marketing".into(), "landing".into(), "营销".into()],
            category: "website".into(),
            document_json: serde_json::json!({
                "pages": [{
                    "id": "landing", "name": "落地页", "width": 1440.0, "height": 900.0,
                    "nodes": [
                        {"id":"navbar","kind":"rect","x":0.0,"y":0.0,"w":1440.0,"h":72.0,"fill":"#FFFFFF","stroke":"#E5E5E5"},
                        {"id":"brand","kind":"text","x":64.0,"y":20.0,"w":160.0,"h":32.0,"text":"Brand","fill":"#000000"},
                        {"id":"hero-section","kind":"rect","x":0.0,"y":72.0,"w":1440.0,"h":480.0,"fill":"#F8F9FA"},
                        {"id":"hero-title","kind":"text","x":200.0,"y":200.0,"w":600.0,"h":48.0,"text":"让设计更简单","fill":"#1A1A2E"},
                        {"id":"hero-subtitle","kind":"text","x":200.0,"y":264.0,"w":500.0,"h":32.0,"text":"AI 驱动的本地设计工具","fill":"#666666"},
                        {"id":"cta-button","kind":"rect","x":200.0,"y":320.0,"w":160.0,"h":48.0,"fill":"#007AFF"},
                        {"id":"cta-text","kind":"text","x":216.0,"y":328.0,"w":128.0,"h":32.0,"text":"立即开始","fill":"#FFFFFF"},
                        {"id":"features-section","kind":"rect","x":0.0,"y":552.0,"w":1440.0,"h":348.0,"fill":"#FFFFFF"},
                        {"id":"feature-1","kind":"rect","x":64.0,"y":592.0,"w":400.0,"h":240.0,"fill":"#F5F5F7","stroke":"#E5E5E5"},
                        {"id":"feature-2","kind":"rect","x":520.0,"y":592.0,"w":400.0,"h":240.0,"fill":"#F5F5F7","stroke":"#E5E5E5"},
                        {"id":"feature-3","kind":"rect","x":976.0,"y":592.0,"w":400.0,"h":240.0,"fill":"#F5F5F7","stroke":"#E5E5E5"}
                    ]
                }]
            }).to_string(),
            created_at: default_timestamp(),
        },
        // 4. 小程序
        DesignTemplate {
            id: "builtin-mini-program".into(),
            name: "小程序".into(),
            tags: vec!["mini-program".into(), "wechat".into(), "小程序".into(), "微信".into()],
            category: "mini-program".into(),
            document_json: serde_json::json!({
                "pages": [{
                    "id": "index", "name": "首页", "width": 375.0, "height": 812.0,
                    "nodes": [
                        {"id":"status-bar","kind":"rect","x":0.0,"y":0.0,"w":375.0,"h":44.0,"fill":"#FFFFFF"},
                        {"id":"search-bar","kind":"rect","x":12.0,"y":52.0,"w":351.0,"h":36.0,"fill":"#F5F5F5","stroke":"#E5E5E5"},
                        {"id":"swiper","kind":"rect","x":0.0,"y":100.0,"w":375.0,"h":160.0,"fill":"#E8E8E8"},
                        {"id":"grid-nav","kind":"rect","x":12.0,"y":276.0,"w":351.0,"h":160.0,"fill":"#FFFFFF","stroke":"#EEEEEE"},
                        {"id":"product-1","kind":"rect","x":12.0,"y":452.0,"w":168.0,"h":220.0,"fill":"#FFFFFF","stroke":"#EEEEEE"},
                        {"id":"product-2","kind":"rect","x":195.0,"y":452.0,"w":168.0,"h":220.0,"fill":"#FFFFFF","stroke":"#EEEEEE"},
                        {"id":"product-3","kind":"rect","x":12.0,"y":688.0,"w":168.0,"h":220.0,"fill":"#FFFFFF","stroke":"#EEEEEE"},
                        {"id":"product-4","kind":"rect","x":195.0,"y":688.0,"w":168.0,"h":220.0,"fill":"#FFFFFF","stroke":"#EEEEEE"},
                        {"id":"tab-bar","kind":"rect","x":0.0,"y":728.0,"w":375.0,"h":84.0,"fill":"#FFFFFF","stroke":"#E5E5E5"}
                    ]
                }]
            }).to_string(),
            created_at: default_timestamp(),
        },
    ];
    tracing::info!(
        count = templates.len(),
        "builtin_scene_templates: 返回内置模板"
    );
    templates
}

impl EcosystemLink {
    /// 将内置场景模板保存到 KB 目录（幂等，已存在则跳过）。
    pub fn install_builtin_templates(&self) -> anyhow::Result<usize> {
        let mut installed = 0usize;
        for tmpl in builtin_scene_templates() {
            let kb_dir = self
                .base_dir
                .join(EcosystemTarget::FusionKB.ipc_dir())
                .join("templates");
            std::fs::create_dir_all(&kb_dir)?;
            let file = kb_dir.join(format!("{}.json", tmpl.id));
            if file.exists() {
                // E-17/P3：旧实现仅判 file.exists() 跳过，损坏/空文件永久不可用。
                // 先校验可解析，corrupt 则覆盖重装；有效才跳过。
                let need_overwrite = match std::fs::read_to_string(&file) {
                    Ok(content) => match serde_json::from_str::<DesignTemplate>(&content) {
                        Ok(existing) => existing.id != tmpl.id,
                        Err(e) => {
                            tracing::warn!(id = %tmpl.id, error = %e, "install_builtin_templates: 已存在文件损坏，覆盖重装");
                            true
                        }
                    },
                    Err(e) => {
                        tracing::warn!(id = %tmpl.id, error = %e, "install_builtin_templates: 已存在文件不可读，覆盖重装");
                        true
                    }
                };
                if !need_overwrite {
                    tracing::debug!(id = %tmpl.id, "install_builtin_templates: 模板已存在且有效，跳过");
                    continue;
                }
            }
            let json = serde_json::to_string_pretty(&tmpl)?;
            std::fs::write(&file, json)?;
            installed += 1;
            tracing::info!(id = %tmpl.id, name = %tmpl.name, "install_builtin_templates: 模板已安装");
        }
        Ok(installed)
    }
}

#[cfg(test)]
mod builtin_template_tests {
    use super::*;

    #[test]
    fn builtin_templates_has_four() {
        let templates = builtin_scene_templates();
        assert_eq!(templates.len(), 4);
    }

    #[test]
    fn builtin_templates_categories() {
        let templates = builtin_scene_templates();
        let categories: Vec<&str> = templates.iter().map(|t| t.category.as_str()).collect();
        assert!(categories.contains(&"mobile"));
        assert!(categories.contains(&"admin"));
        assert!(categories.contains(&"website"));
        assert!(categories.contains(&"mini-program"));
    }

    #[test]
    fn builtin_templates_valid_json() {
        for tmpl in builtin_scene_templates() {
            let v: serde_json::Value = serde_json::from_str(&tmpl.document_json).unwrap();
            assert!(v.get("pages").is_some(), "模板 {} 缺少 pages", tmpl.id);
        }
    }

    #[test]
    fn install_builtin_templates_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let link = EcosystemLink::new(dir.path());
        let count1 = link.install_builtin_templates().unwrap();
        assert_eq!(count1, 4);
        let count2 = link.install_builtin_templates().unwrap();
        assert_eq!(count2, 0);
    }

    #[test]
    fn install_builtin_templates_reinstalls_corrupt_file() {
        // E-17/P3：旧实现仅判 exists 跳过，损坏文件永久阻塞重装。
        // 先正常安装，再写空/损坏内容到首个模板文件，重装须覆盖且 count > 0。
        let dir = tempfile::tempdir().unwrap();
        let link = EcosystemLink::new(dir.path());
        link.install_builtin_templates().unwrap();
        let templates_dir = dir
            .path()
            .join(EcosystemTarget::FusionKB.ipc_dir())
            .join("templates");
        let first_id = &builtin_scene_templates()[0].id;
        let corrupt_file = templates_dir.join(format!("{first_id}.json"));
        std::fs::write(&corrupt_file, "{ corrupted not json").unwrap();
        let reinstalled = link.install_builtin_templates().unwrap();
        assert_eq!(reinstalled, 1, "损坏文件须覆盖重装 1 个");
        let restored = std::fs::read_to_string(&corrupt_file).unwrap();
        let parsed: DesignTemplate = serde_json::from_str(&restored).unwrap();
        assert_eq!(parsed.id, *first_id, "重装内容为有效内置模板");
    }

    #[test]
    fn search_builtin_by_category() {
        let dir = tempfile::tempdir().unwrap();
        let link = EcosystemLink::new(dir.path());
        link.install_builtin_templates().unwrap();
        let mobile = link.search_templates("mobile").unwrap();
        assert!(!mobile.is_empty());
    }

    #[test]
    fn trainer_resolve_bin_honors_env() {
        // 环境变量优先于默认 .venv 路径
        let prev = std::env::var("FUSION_TRAINER_BIN").ok();
        std::env::set_var("FUSION_TRAINER_BIN", "/tmp/ft-test-bin");
        let bin = TrainerClient::resolve_bin();
        assert_eq!(bin, std::path::PathBuf::from("/tmp/ft-test-bin"));
        match prev {
            Some(v) => std::env::set_var("FUSION_TRAINER_BIN", v),
            None => std::env::remove_var("FUSION_TRAINER_BIN"),
        }
    }

    #[test]
    fn trainer_with_bin_stores_path() {
        let client = TrainerClient::with_bin("/nonexistent/fusion-trainer");
        // bin 不存在时应失败可见（fail-visible），不真正 spawn 子进程
        let err = client
            .run_sft(
                std::path::Path::new("/tmp/ds.jsonl"),
                "qwen2.5-7b-4bit",
                None,
            )
            .unwrap_err();
        assert!(format!("{}", err).contains("fusion-trainer CLI 未找到"));
    }

    #[test]
    fn trainer_rlsl_missing_bin_bails() {
        let client = TrainerClient::with_bin("/nonexistent/fusion-trainer");
        let err = client
            .run_rlsl(
                "grpo",
                std::path::Path::new("/tmp/ds.jsonl"),
                "qwen2.5-7b-4bit",
                None,
            )
            .unwrap_err();
        assert!(format!("{}", err).contains("fusion-trainer CLI 未找到"));
    }
}
