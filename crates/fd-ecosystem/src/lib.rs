//! Fusion-Design ecosystem integration — connects Fusion Code/Simulation/KB/CLI.
//!
//! Corresponds to PRD module 6 "Ecosystem integration". Uses local file IPC (JSON
//! message files in agreed directories) to connect the whole ecosystem:
//! send/list/consume, sync_to_code, template search/presets.
//!
//! [H-A3/P2-2] The self-built MCP protocol layer + watch_async have been removed (zero external calls, dead code).
//!
//! [Offline hard constraint] All integration goes through the local filesystem or 127.0.0.1; no public-network calls.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

// Process-level IPC message sequence number; combined with pid + nanos it keeps filenames globally
// unique, avoiding same-nanosecond collisions (audit C7).
static IPC_SEQ: AtomicU64 = AtomicU64::new(0);

/// Ecosystem integration target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EcosystemTarget {
    /// Fusion Code (forward code export, reverse style sync)
    FusionCode,
    /// Fusion-KB (save/retrieve design templates)
    FusionKB,
    /// Fusion CLI (command-line batch generation/export)
    FusionCLI,
    /// Extension target (plugins/vertical domains plug in via this variant; no hardcoded industries)
    Custom(String),
}

impl EcosystemTarget {
    /// Agreed IPC directory name.
    pub fn ipc_dir(&self) -> String {
        match self {
            Self::FusionCode => "fusion-code".to_string(),
            Self::FusionKB => "fusion-kb".to_string(),
            Self::FusionCLI => "fusion-cli".to_string(),
            Self::Custom(name) => name.clone(),
        }
    }
}

// Resolve base + sub into a safe subpath: reject `..` components and absolute paths, and verify the
// result stays within base.
// Used by send() to prevent EcosystemTarget::Custom from carrying `../` for path-traversal writes (audit C6).
fn sanitize_ipc_subpath(base: &Path, sub: &str) -> anyhow::Result<PathBuf> {
    if sub.is_empty() {
        anyhow::bail!("IPC subpath is empty");
    }
    let sub_path = Path::new(sub);
    if sub_path.is_absolute() {
        tracing::warn!(sub = %sub, "IPC subpath is absolute, rejecting");
        anyhow::bail!("IPC subpath must not be absolute: {sub}");
    }
    for comp in sub_path.components() {
        use std::path::Component;
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                tracing::warn!(sub = %sub, "IPC subpath contains a `..` component, rejecting");
                anyhow::bail!("IPC subpath must not contain `..` components: {sub}");
            }
            Component::RootDir | Component::Prefix(_) => {
                tracing::warn!(sub = %sub, "IPC subpath contains a root/prefix component, rejecting");
                anyhow::bail!("IPC subpath must not contain root or prefix components: {sub}");
            }
        }
    }
    let joined = base.join(sub_path);
    // Secondary check: if both base/joined can be canonicalized (already exist), compare canonical
    // paths to guard against symlink bypass; otherwise fall back — components already filter out
    // `..`/absolute paths, but **lexical starts_with is not sufficient against symlinks**
    // (some component of the joined path may be a symlink pointing outside base). The fallback only
    // checks whether new components below **base** are symlinks (excluding base itself and its
    // system ancestors, e.g. macOS /var→/private/var is a legitimate system symlink and must not
    // be wrongly rejected). Any new component below base that is a symlink is rejected (R-9).
    // The canonical path is not returned, keeping the path shape consistent with list()/callers.
    let canonical_base = base.canonicalize();
    let canonical_joined = joined.canonicalize();
    let within = match (canonical_base, canonical_joined) {
        (Ok(cb), Ok(cj)) => cj.starts_with(&cb),
        _ => {
            // Fallback branch: canonicalize failed (path does not exist yet, or insufficient permissions).
            // Only check new components below base, skipping base itself and system prefixes.
            reject_symlink_below_base(base, &joined)?;
            joined.starts_with(base)
        }
    };
    if !within {
        tracing::warn!(
            base = %base.display(),
            resolved = %joined.display(),
            "IPC subpath escapes base after resolution, rejecting"
        );
        anyhow::bail!("IPC subpath escapes base directory: {sub}");
    }
    Ok(joined)
}

/// R-9: detect whether new path components below base are symlinks. Skips base itself and its
/// system ancestors (e.g. macOS /var→/private/var is a legitimate system symlink); only scans
/// the components between base and joined.
fn reject_symlink_below_base(base: &Path, joined: &Path) -> anyhow::Result<()> {
    // joined = base + sub_path components. Build prefixes step by step starting after base.
    let mut acc = base.to_path_buf();
    let base_components = base.components().count();
    for (i, comp) in joined.components().enumerate() {
        if i < base_components {
            // base itself and ancestor components, skip (system symlinks are legitimate).
            continue;
        }
        acc.push(comp);
        if let Ok(meta) = std::fs::symlink_metadata(&acc) {
            if meta.file_type().is_symlink() {
                tracing::warn!(
                    component = %acc.display(),
                    "IPC subpath contains a symlink component below base, rejecting under fallback check to prevent traversal"
                );
                anyhow::bail!("IPC subpath contains a symlink component: {}", acc.display());
            }
        }
    }
    Ok(())
}

/// R-8: decide whether a path is a valid IPC message file — only the `.json` extension is allowed.
/// Excludes code artifacts written by sync_to_code (`.html`/`.tsx`) and `.tmp*` leftovers (E-22).
fn is_ipc_message(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("json")
}

/// E-22: clean up `.tmp*` leftover files in the directory (left by a previous send that crashed
/// before rename). Failures only warn and do not block the main flow (cleanup is best-effort).
fn cleanup_stale_tmp(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if name.contains(".tmp") {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(file = %path.display(), error = %e, "cleanup_stale_tmp: failed to remove leftover .tmp file");
            } else {
                tracing::debug!(file = %path.display(), "cleanup_stale_tmp: cleaned leftover .tmp file");
            }
        }
    }
}

/// E-20: find an executable on PATH (which semantics, no external dependencies).
/// Splits the PATH env var and joins the candidate name per directory; returns it if it exists and is a file.
fn which_trainer(name: &str) -> Result<PathBuf, ()> {
    let path_env = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path_env) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(())
}

/// E-20: locate the Fusion monorepo shared .venv/bin directory.
/// Prefers the FUSION_VENV_ROOT env; otherwise walks up from the current executable's directory
/// to find an ancestor containing `.venv/bin`. Avoids hardcoding `/Users/dahai/fusion/.venv`,
/// keeping it portable across machines/deployments.
fn fusion_venv_bin_dir() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("FUSION_VENV_ROOT") {
        return Some(PathBuf::from(root).join("bin"));
    }
    // Walk up from the current executable path to find .venv/bin (target/debug under cargo run; the repo root is an ancestor).
    let exe = std::env::current_exe().ok()?;
    let mut cur = exe.parent()?.to_path_buf();
    for _ in 0..10 {
        let candidate = cur.join(".venv").join("bin");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

/// Link message body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkMessage {
    pub target: EcosystemTarget,
    pub action: String,
    pub payload: serde_json::Value,
}

/// fusion-trainer CLI subprocess wrapper (offline hard constraint: only calls the local .venv fusion-trainer).
#[derive(Debug, Clone)]
pub struct TrainerClient {
    bin: PathBuf,
}

impl TrainerClient {
    /// Resolve the fusion-trainer executable path: prefer the FUSION_TRAINER_BIN env, then a PATH
    /// lookup for `fusion-trainer`, then fall back to the shared .venv default (still contains the
    /// user home dir, last resort only).
    /// E-20: removed the `/Users/dahai/...` hardcoded sole source in favor of discovery-first.
    pub fn resolve_bin() -> PathBuf {
        if let Ok(b) = std::env::var("FUSION_TRAINER_BIN") {
            return PathBuf::from(b);
        }
        // Prefer a fusion-trainer on PATH (which semantics, portable).
        if let Ok(p) = which_trainer("fusion-trainer") {
            return p;
        }
        // Fallback: the .venv sibling of the current fusion-design project (Fusion monorepo convention).
        // Derived from FUSION_VENV_ROOT or by walking up to fusion/.venv, avoiding absolute path hardcoding.
        if let Some(venv) = fusion_venv_bin_dir() {
            let candidate = venv.join("fusion-trainer");
            if candidate.exists() {
                return candidate;
            }
        }
        // Final fallback: rely on PATH (spawn errors explicitly and hints at FUSION_TRAINER_BIN if missing).
        PathBuf::from("fusion-trainer")
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
                "fusion-trainer CLI not found: {} (install fusion-trainer or set FUSION_TRAINER_BIN)",
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

    /// Invoke fusion-trainer sft --dataset <jsonl> --model <id>
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

    /// Invoke fusion-trainer rlsl --method <m> --dataset <jsonl> --model <id>
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

/// E-16/P2-6: template search cache. Invalidated by the template directory mtime — after
/// save_template writes a file the directory mtime changes, so the next search rescans; if
/// unchanged, the in-memory parsed result is reused with zero disk IO. The old implementation
/// did read_dir + per-file read_to_string + from_str on every search, degrading linearly with N templates.
#[derive(Debug, Clone, Default)]
struct TemplateCache {
    /// Template directory mtime the cache corresponds to (second-level; any cross-second change invalidates it).
    dir_mtime_secs: u64,
    /// All parsed templates (descending by created_at).
    templates: Vec<DesignTemplate>,
}

/// Ecosystem integration client (based on local file IPC).
// E-16/P2-6: contains a Mutex cache, so Clone is no longer derived (Mutex is not Clone; no EcosystemLink::clone callers in the repo).
#[derive(Debug)]
pub struct EcosystemLink {
    base_dir: PathBuf,
    // E-16/P2-6: template search cache, Mutex-wrapped for thread-safe reuse.
    template_cache: Mutex<TemplateCache>,
}

impl EcosystemLink {
    /// Construct with a specific base directory (all IPC files land in `<base>/<target>/`).
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            template_cache: Mutex::new(TemplateCache::default()),
        }
    }

    /// Send one link message (writes a JSON file into the target directory).
    pub fn send(&self, msg: &LinkMessage) -> anyhow::Result<PathBuf> {
        let dir = sanitize_ipc_subpath(&self.base_dir, &msg.target.ipc_dir())?;
        std::fs::create_dir_all(&dir)?;
        // E-22: clean up .tmp* files left by a previous crashed send, preventing later list from
        // consuming them by mistake (is_ipc_message already filters, but leftovers pollute the
        // directory and waste inodes; clean proactively).
        cleanup_stale_tmp(&dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let pid = std::process::id();
        let seq = IPC_SEQ.fetch_add(1, Ordering::SeqCst);
        let file = dir.join(format!("{stamp}_{pid}_{seq}.json"));
        let json = serde_json::to_string_pretty(msg)?;
        // Atomic write: write tmp then rename, avoiding a half-written JSON after a crash (audit C7).
        let tmp = dir.join(format!("{stamp}_{pid}_{seq}.json.tmp.{pid}.{seq}"));
        std::fs::write(&tmp, &json)?;
        if let Err(e) = std::fs::rename(&tmp, &file) {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!(?tmp, error = %e, "send: rename failed, cleaning tmp");
            return Err(anyhow::Error::from(e));
        }
        tracing::info!(?file, target = msg.target.ipc_dir(), "link message sent");
        Ok(file)
    }

    /// List all pending message files for a target.
    ///
    /// R-8: only `.json` files are returned; code artifacts (`.html`/`.tsx`/`.tmp*`) are ignored.
    /// sync_to_code writes generated HTML/React into the same `fusion-code/` directory;
    /// without filtering, `list(FusionCode)` would consume `.tsx`/`.html` as messages and fail to parse.
    pub fn list(&self, target: EcosystemTarget) -> anyhow::Result<Vec<PathBuf>> {
        let dir = self.base_dir.join(target.ipc_dir());
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| is_ipc_message(p))
            .collect();
        files.sort();
        Ok(files)
    }

    /// Read and delete one message (consuming). Includes a size guard against OOM from malicious large files.
    ///
    /// R-7: atomic claim — first `rename(file, .processing/<name>)` to claim; rename is atomic within
    /// the same filesystem, so the winner consumes exclusively and losers (already claimed, file
    /// gone) skip. Fixes the old "read then delete" flaw where multiple consumers silently swallowed
    /// each other's messages. Behavior is unchanged with a single consumer (rename always succeeds).
    /// R-A2: on parse failure the processing file is **not deleted** (kept for diagnostics); returns
    /// Err with the file path. Callers must handle Err explicitly (see watch_code_changes).
    pub fn consume(&self, file: &Path) -> anyhow::Result<LinkMessage> {
        // R-7: atomic claim. .processing/ shares the parent with the message directory; same-filesystem rename is atomic.
        let proc_dir = file
            .parent()
            .ok_or_else(|| anyhow::anyhow!("IPC message file has no parent directory: {}", file.display()))?
            .join(".processing");
        std::fs::create_dir_all(&proc_dir)?;
        let proc_name = file
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("IPC message file has no file name: {}", file.display()))?;
        let proc_file = proc_dir.join(proc_name);
        match std::fs::rename(file, &proc_file) {
            Ok(()) => {
                tracing::debug!(
                    from = %file.display(),
                    to = %proc_file.display(),
                    "consume: rename claim succeeded, consuming exclusively"
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Already claimed by another consumer, skip (not an error).
                tracing::debug!(
                    file = %file.display(),
                    "consume: file already claimed by another consumer, skipping"
                );
                anyhow::bail!("IPC message already consumed by another consumer: {}", file.display());
            }
            Err(e) => {
                tracing::warn!(file = %file.display(), error = %e, "consume: rename claim failed");
                anyhow::bail!("failed to claim IPC message {}: {e}", file.display());
            }
        }
        // L-6: TOCTOU fix — single-fd closed loop, no metadata() precheck (the file can change between meta and read).
        // After open, take(cap) limits reads; the upper bound is validated by the actual bytes read.
        const MAX_IPC_FILE: u64 = 8 * 1024 * 1024;
        let f = std::fs::File::open(&proc_file)
            .map_err(|e| anyhow::anyhow!("failed to open IPC message file {}: {e}", proc_file.display()))?;
        use std::io::Read;
        let mut buf = Vec::new();
        f.take(MAX_IPC_FILE + 1)
            .read_to_end(&mut buf)
            .map_err(|e| anyhow::anyhow!("failed to read IPC message file {}: {e}", proc_file.display()))?;
        if buf.len() as u64 > MAX_IPC_FILE {
            tracing::warn!(size = buf.len(), "IPC message file exceeds the 8MB limit, rejecting read");
            anyhow::bail!("IPC message file {} exceeds the 8MB safety limit", proc_file.display());
        }
        let json = String::from_utf8(buf)
            .map_err(|e| anyhow::anyhow!("IPC message file is not UTF-8 {}: {e}", proc_file.display()))?;
        let msg: LinkMessage = serde_json::from_str(&json).map_err(|e| {
            // R-A2: on parse failure do not delete the processing file, keep it for diagnostics, report the path explicitly.
            anyhow::anyhow!(
                "failed to parse IPC message {} (file kept, not deleted): {e}",
                proc_file.display()
            )
        })?;
        std::fs::remove_file(&proc_file).map_err(|e| {
            anyhow::anyhow!("failed to delete IPC message file after consumption {}: {e}", proc_file.display())
        })?;
        Ok(msg)
    }

    /// Async send (for task queues).
    pub async fn send_async(&self, msg: LinkMessage) -> anyhow::Result<PathBuf> {
        let base = self.base_dir.clone();
        tokio::task::spawn_blocking(move || EcosystemLink::new(base).send(&msg)).await?
    }

    /// Forward sync: PenDocument → fd-codegen generates code → writes into the Fusion Code project directory.
    ///
    /// Generates both HTML and React+Tailwind formats, written into the `<base>/fusion-code/` directory.
    pub fn sync_to_code(
        &self,
        doc: &fd_canvas_core::PenDocument,
        component_name: &str,
    ) -> anyhow::Result<Vec<PathBuf>> {
        let code_dir = self.base_dir.join(EcosystemTarget::FusionCode.ipc_dir());
        std::fs::create_dir_all(&code_dir)?;

        let mut written = vec![];

        // Generate HTML
        let html_gen = fd_codegen::HtmlCodegen;
        let html = fd_codegen::Codegen::generate(&html_gen, doc);
        let html_path = code_dir.join(format!("{component_name}.html"));
        std::fs::write(&html_path, &html)?;
        tracing::info!(?html_path, "sync_to_code: HTML written");
        written.push(html_path);

        // Generate React + Tailwind
        let react_gen = fd_codegen::ReactTailwindCodegen {
            component_name: component_name.to_string(),
        };
        let react = fd_codegen::Codegen::generate(&react_gen, doc);
        let react_path = code_dir.join(format!("{component_name}.tsx"));
        std::fs::write(&react_path, &react)?;
        tracing::info!(?react_path, "sync_to_code: React+Tailwind written");
        written.push(react_path);

        // Also send an IPC message to notify Fusion Code
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

    /// Async forward sync.
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

    /// Reverse watch: scan the Fusion Code directory's IPC messages and extract style changes.
    ///
    /// Consumes all pending messages, filters action="style-change", and returns a list of
    /// MutateNodes commands. The caller is responsible for sending MutateNodes to the canvas.
    pub fn watch_code_changes(&self) -> anyhow::Result<Vec<MutateNodeCommand>> {
        let messages = self.list(EcosystemTarget::FusionCode)?;
        let mut commands = vec![];
        for file in &messages {
            // R-A2/P2-3: the old implementation's `if let Ok` silently swallowed all consume Errs —
            // corrupt files were neither logged nor cleaned, staying in the directory and failing
            // forever (zombie messages), violating fail visibly. Changed to an explicit match; on
            // Err, warn with the file path (consume already keeps corrupt files for diagnostics).
            match self.consume(file) {
                Ok(msg) => {
                    if msg.action == "style-change" {
                        if let Some(arr) = msg.payload.get("mutations").and_then(|v| v.as_array()) {
                            // E-18/P3: the old implementation silently dropped None from parse_mutate_command;
                            // failed mutation items raised no alert at all, making debugging impossible.
                            // Count and warn explicitly.
                            let total = arr.len();
                            let mut dropped = 0usize;
                            for item in arr {
                                match parse_mutate_command(item) {
                                    Some(cmd) => commands.push(cmd),
                                    None => {
                                        dropped += 1;
                                        tracing::warn!(
                                            item = %item,
                                            "watch_code_changes: mutation parse failed and was dropped (non-object or missing node_id)"
                                        );
                                    }
                                }
                            }
                            if dropped > 0 {
                                tracing::warn!(
                                    total,
                                    dropped,
                                    "watch_code_changes: dropped {dropped}/{total} mutations in this message"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        file = %file.display(),
                        error = %e,
                        "watch_code_changes: failed to consume message (file kept, not deleted; may be corrupt/oversized/race)"
                    );
                }
            }
        }
        if !commands.is_empty() {
            tracing::info!(count = commands.len(), "watch_code_changes: found style changes");
        } else {
            tracing::debug!("watch_code_changes: no valid style-change commands");
        }
        Ok(commands)
    }
}

/// MutateNode command (style changes reverse-synced from Fusion Code).
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

// ── KB template system ──

/// Design template metadata.
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
    /// Save a design template into the Fusion-KB directory.
    pub fn save_template(&self, tmpl: &DesignTemplate) -> anyhow::Result<PathBuf> {
        let kb_dir = self
            .base_dir
            .join(EcosystemTarget::FusionKB.ipc_dir())
            .join("templates");
        std::fs::create_dir_all(&kb_dir)?;
        let file = kb_dir.join(format!("{}.json", tmpl.id));
        let json = serde_json::to_string_pretty(tmpl)?;
        std::fs::write(&file, json)?;
        tracing::info!(?file, id = %tmpl.id, "save_template: template saved");
        // E-16/P2-6: invalidate the cache on write (mtime has second-level precision; consecutive
        // writes within the same second may be missed, so explicit clearing is safer).
        if let Ok(mut c) = self.template_cache.lock() {
            *c = TemplateCache::default();
        }
        // Also send an IPC message to notify
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

    /// E-16/P2-6: return all templates (descending); on cache hit, zero disk IO.
    /// Cache invalidated by the template directory mtime (second-level): save_template writes a
    /// file → mtime changes → rescan. The old implementation did read_dir + per-file
    /// read_to_string + from_str on every search, degrading linearly with N templates.
    fn load_all_templates_cached(&self) -> anyhow::Result<Vec<DesignTemplate>> {
        let kb_dir = self
            .base_dir
            .join(EcosystemTarget::FusionKB.ipc_dir())
            .join("templates");
        if !kb_dir.exists() {
            // Directory missing: clear the cache and return empty.
            if let Ok(mut c) = self.template_cache.lock() {
                *c = TemplateCache::default();
            }
            return Ok(vec![]);
        }
        // Directory mtime (second-level) as the cache key. On failure (permissions etc.), force a rescan; do not trust the old cache.
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
                    "load_all_templates_cached: cache hit (zero disk IO)"
                );
                return Ok(c.templates.clone());
            }
        }
        // Miss: scan disk and parse.
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
            "load_all_templates_cached: cache miss, rescanned and filled"
        );
        if let Ok(mut c) = self.template_cache.lock() {
            c.dir_mtime_secs = mtime_secs;
            c.templates = templates.clone();
        }
        Ok(templates)
    }

    /// Search templates: match name/tags/category by keyword.
    pub fn search_templates(&self, query: &str) -> anyhow::Result<Vec<DesignTemplate>> {
        let all = self.load_all_templates_cached()?;
        let q = query.to_lowercase();
        // E-16/P2-6: in-memory filtering, no disk IO (all is already descending; an order-preserving filter suffices).
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
        tracing::info!(query = %query, count = results.len(), "search_templates: search done");
        Ok(results)
    }

    /// Search templates by exact tag match (multiple tags intersect).
    pub fn search_templates_by_tags(&self, tags: &[String]) -> anyhow::Result<Vec<DesignTemplate>> {
        let all = self.load_all_templates_cached()?;
        let lower_tags: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();
        // E-16/P2-6: in-memory filtering, no disk IO.
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
        tracing::info!(tags = ?tags, count = results.len(), "search_templates_by_tags: search done");
        Ok(results)
    }
}

// H-A3/P2-2: watch_async (async file watching, notify) + the self-built MCP protocol layer (McpServer/
// McpTool/ListTargetsTool/SearchTemplatesTool/SendMessageTool + WatchEvent/Kind)
// have been removed — zero external calls, presentational dead code.
// The MCP layer was non-compliant (only the initialize/tools-list/call three methods, missing
// handshake/resources/progress; cannot be consumed by standard MCP clients, see audit H-A4);
// watch_async had no consumers.
// The EcosystemLink IPC core (send/list/consume/sync_to_code/search_templates/templates) is kept.
// To restore, see git history.

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
        // file deleted after consume
        assert!(link.list(EcosystemTarget::FusionCode).unwrap().is_empty());
    }

    // R-A2 + R-7: consuming a corrupt file should fail. R-7 first renames into .processing/ to claim,
    // and after parse failure the file is kept in .processing/ for diagnostics (no longer silently dropping data).
    #[test]
    fn consume_corrupt_file_preserves_and_errors() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let dir = tmp.path().join(EcosystemTarget::FusionCode.ipc_dir());
        std::fs::create_dir_all(&dir).unwrap();
        let corrupt = dir.join("corrupt.json");
        // Half-written invalid JSON (simulating an interrupted rename / full disk)
        std::fs::write(&corrupt, "{ not valid json").unwrap();
        let result = link.consume(&corrupt);
        assert!(result.is_err(), "corrupt file should fail to parse and return Err");
        // R-7: the original path was renamed away by the claim; the file is kept in .processing/ for diagnostics (not silently dropped).
        assert!(!corrupt.exists(), "original file should have been renamed away by the claim");
        let proc_file = dir.join(".processing").join("corrupt.json");
        assert!(
            proc_file.exists(),
            "corrupt file should be kept in .processing/ for diagnostics, not deleted by consume"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("not deleted"), "Err message should state the file is kept");
    }

    // R-A2 + R-7: consuming an oversized file should be rejected; the file is kept in .processing/ for diagnostics.
    #[test]
    fn consume_oversize_file_rejects_preserves() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let dir = tmp.path().join(EcosystemTarget::FusionCode.ipc_dir());
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.json");
        // Write 9MB, exceeding the 8MB limit
        std::fs::write(&big, "x".repeat(9 * 1024 * 1024)).unwrap();
        let result = link.consume(&big);
        assert!(result.is_err(), "oversized file should be rejected");
        let proc_file = dir.join(".processing").join("big.json");
        assert!(proc_file.exists(), "oversized file should be kept in .processing/, not deleted");
    }

    // R-A2/P2-3 + R-7: watch_code_changes must neither panic nor stay silent on corrupt files — it
    // returns empty commands, and the corrupt file is claimed via rename into .processing/ and kept
    // (fail visibly, visible in caller logs).
    #[test]
    fn watch_code_changes_corrupt_file_does_not_silently_swallow() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let dir = tmp.path().join(EcosystemTarget::FusionCode.ipc_dir());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.json"), "{ broken").unwrap();
        // Should return Ok (empty commands) without panic; the corrupt file is claimed into .processing/ and kept for diagnostics.
        let cmds = link.watch_code_changes().expect("corrupt file should not cause overall failure");
        assert!(cmds.is_empty());
        assert!(
            dir.join(".processing").join("bad.json").exists(),
            "corrupt file should be kept in .processing/"
        );
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
        // Audit C6: Custom("../evil") must be rejected and must not escape the base directory.
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
            "error message should mention the `..` component, actual: {err}"
        );
        // Confirm no file was written outside base
        assert!(std::fs::read_dir(tmp.path()).unwrap().count() == 0);
    }

    #[test]
    fn send_filename_contains_pid_and_seq() {
        // Audit C7: the filename should contain pid + seq, avoiding same-nanosecond collisions.
        // The static counter is process-level; do not assume a specific seq value, only verify the <stamp>_<pid>_<seq>.json shape.
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
        assert!(fname.contains(&pid), "filename should contain pid={pid}, actual: {fname}");
        // Shape: <numeric nanoseconds>_<pid>_<numeric seq>.json
        let stem = fname
            .strip_suffix(".json")
            .unwrap_or_else(|| panic!("filename should end with .json: {fname}"));
        let parts: Vec<&str> = stem.rsplitn(3, '_').collect();
        // rsplitn output order: seq, pid, stamp (3 segments split right-to-left)
        assert_eq!(parts.len(), 3, "filename should have stamp_pid_seq segments: {fname}");
        assert!(parts[0].parse::<u64>().is_ok(), "seq should be numeric: {fname}");
        assert_eq!(parts[1], pid, "pid segment should match the current process: {fname}");
        assert!(parts[2].parse::<u128>().is_ok(), "stamp should be numeric: {fname}");
    }

    #[test]
    fn send_concurrent_does_not_collide() {
        // Audit C7: two consecutive sends should produce different filenames (seq increments).
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let mk = || LinkMessage {
            target: EcosystemTarget::FusionCLI,
            action: "burst".into(),
            payload: serde_json::json!({}),
        };
        let f1 = link.send(&mk()).unwrap();
        let f2 = link.send(&mk()).unwrap();
        assert_ne!(f1, f2, "two sends should not collide on the same filename");
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
        // Place one style-change message
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
        // Place one unrelated message
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
        // E-18/P3: mix of valid/invalid mutation items. The old implementation silently dropped all
        // invalid ones; after the fix, valid items must be kept and invalid ones dropped
        // (non-object/missing node_id/non-array element), without panic.
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
            "only items with node_id are kept: {ids:?}"
        );
    }

    #[test]
    fn save_and_search_template() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        let tmpl = DesignTemplate {
            id: "login-v1".into(),
            // i18n: user-facing
            name: "登录页面".into(),
            tags: vec!["login".into(), "auth".into()],
            // i18n: user-facing
            category: "表单".into(),
            document_json: "{}".into(),
            created_at: 1000,
        };
        let path = link.save_template(&tmpl).unwrap();
        assert!(path.exists());

        // Search by name
        // i18n: user-facing (query matches user-facing template data)
        let results = link.search_templates("登录").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "login-v1");

        // Search by tag
        let results = link.search_templates("auth").unwrap();
        assert_eq!(results.len(), 1);

        // Search by category
        // i18n: user-facing (query matches user-facing template data)
        let results = link.search_templates("表单").unwrap();
        assert_eq!(results.len(), 1);

        // Empty query returns all
        let results = link.search_templates("").unwrap();
        assert_eq!(results.len(), 1);

        // No match returns empty
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
            // i18n: user-facing
            category: "表单".into(),
            document_json: "{}".into(),
            created_at: 1000,
        };
        let tmpl2 = DesignTemplate {
            id: "t2".into(),
            name: "Signup".into(),
            tags: vec!["auth".into(), "register".into()],
            // i18n: user-facing
            category: "表单".into(),
            document_json: "{}".into(),
            created_at: 2000,
        };
        link.save_template(&tmpl1).unwrap();
        link.save_template(&tmpl2).unwrap();

        // Single tag matches both
        let results = link.search_templates_by_tags(&["auth".into()]).unwrap();
        assert_eq!(results.len(), 2);

        // Two-tag intersection matches only tmpl1
        let results = link
            .search_templates_by_tags(&["auth".into(), "form".into()])
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "t1");

        // Nonexistent tag
        let results = link
            .search_templates_by_tags(&["nonexistent".into()])
            .unwrap();
        assert!(results.is_empty());

        // Empty tags returns all
        let results = link.search_templates_by_tags(&[]).unwrap();
        assert_eq!(results.len(), 2);
    }

    // E-16/P2-6 regression: cache hit — repeated searches return consistent results (cache effective, no rescan).
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
        // Multiple searches; after the cache is filled it should hit (consistent results).
        let r1 = link.search_templates("Cache").unwrap();
        let r2 = link.search_templates("Cache").unwrap();
        let r3 = link.search_templates_by_tags(&["x".into()]).unwrap();
        assert_eq!(r1.len(), 1);
        assert_eq!(r2.len(), 1, "cache hit should return consistent results");
        assert_eq!(r3.len(), 1, "by_tags also goes through the cache");
        assert_eq!(r1[0].id, "c1");
        assert_eq!(r2[0].id, "c1");
    }

    // E-16/P2-6 regression: the cache is invalidated immediately after save_template —
    // consecutive writes within the same second must still let the next search see the new
    // template (backstop for the mtime second-level precision gap).
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
        // Fill the cache.
        assert_eq!(link.search_templates("").unwrap().len(), 1);
        // Save another within the same second (save_template clears the cache explicitly).
        link.save_template(&DesignTemplate {
            id: "s2".into(),
            name: "Second".into(),
            tags: vec![],
            category: "c".into(),
            document_json: "{}".into(),
            created_at: 2000,
        })
        .unwrap();
        // If the cache were not invalidated, this would still return 1 (old cache) — it must return 2.
        let results = link.search_templates("").unwrap();
        assert_eq!(
            results.len(),
            2,
            "cache should be invalidated after save; the search must see the new template (same-second write backstop)"
        );
    }

    // E-16/P2-6 regression: when the directory is missing, the cache is cleared and no stale results are returned.
    #[test]
    fn template_cache_cleared_when_dir_absent() {
        let tmp = tempdir().unwrap();
        let link = EcosystemLink::new(tmp.path());
        // First save a template to fill the cache.
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
        // Delete the entire template directory.
        let kb_dir = tmp
            .path()
            .join(EcosystemTarget::FusionKB.ipc_dir())
            .join("templates");
        std::fs::remove_dir_all(&kb_dir).unwrap();
        // The cache should be cleared; the search returns empty rather than a stale 1.
        assert_eq!(
            link.search_templates("").unwrap().len(),
            0,
            "cache should be cleared after the directory is deleted; no stale results returned"
        );
    }
}

// ── Built-in scene template presets ──

/// Returns the 4 built-in scene templates: mobile app, B-side admin, marketing site, mini program.
pub fn builtin_scene_templates() -> Vec<DesignTemplate> {
    let templates = vec![
        // 1. Mobile app
        DesignTemplate {
            id: "builtin-mobile-app".into(),
            // i18n: user-facing
            name: "移动端应用".into(),
            tags: vec!["mobile".into(), "app".into(), "iOS".into(), "Android".into()],
            category: "mobile".into(),
            document_json: serde_json::json!({
                "pages": [{
                    // i18n: user-facing
                    "id": "home", "name": "首页", "width": 390.0, "height": 844.0,
                    "nodes": [
                        {"id":"nav","kind":"rect","x":0.0,"y":0.0,"w":390.0,"h":88.0,"fill":"#FFFFFF","stroke":"#E5E5E5"},
                        // i18n: user-facing
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
        // 2. B-side admin dashboard
        DesignTemplate {
            id: "builtin-admin-dashboard".into(),
            // i18n: user-facing
            name: "B端后台".into(),
            // i18n: user-facing
            tags: vec!["admin".into(), "dashboard".into(), "B端".into(), "后台".into()],
            category: "admin".into(),
            document_json: serde_json::json!({
                "pages": [{
                    // i18n: user-facing
                    "id": "dashboard", "name": "仪表盘", "width": 1440.0, "height": 900.0,
                    "nodes": [
                        {"id":"sidebar","kind":"rect","x":0.0,"y":0.0,"w":240.0,"h":900.0,"fill":"#1A1A2E"},
                        {"id":"logo","kind":"text","x":24.0,"y":24.0,"w":180.0,"h":32.0,"text":"Admin","fill":"#FFFFFF"},
                        // i18n: user-facing
                        {"id":"nav-1","kind":"text","x":24.0,"y":80.0,"w":180.0,"h":24.0,"text":"仪表盘","fill":"#A0A0C0"},
                        // i18n: user-facing
                        {"id":"nav-2","kind":"text","x":24.0,"y":120.0,"w":180.0,"h":24.0,"text":"用户管理","fill":"#A0A0C0"},
                        // i18n: user-facing
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
        // 3. Marketing site
        DesignTemplate {
            id: "builtin-marketing-site".into(),
            // i18n: user-facing
            name: "营销网站".into(),
            // i18n: user-facing
            tags: vec!["website".into(), "marketing".into(), "landing".into(), "营销".into()],
            category: "website".into(),
            document_json: serde_json::json!({
                "pages": [{
                    // i18n: user-facing
                    "id": "landing", "name": "落地页", "width": 1440.0, "height": 900.0,
                    "nodes": [
                        {"id":"navbar","kind":"rect","x":0.0,"y":0.0,"w":1440.0,"h":72.0,"fill":"#FFFFFF","stroke":"#E5E5E5"},
                        {"id":"brand","kind":"text","x":64.0,"y":20.0,"w":160.0,"h":32.0,"text":"Brand","fill":"#000000"},
                        {"id":"hero-section","kind":"rect","x":0.0,"y":72.0,"w":1440.0,"h":480.0,"fill":"#F8F9FA"},
                        // i18n: user-facing
                        {"id":"hero-title","kind":"text","x":200.0,"y":200.0,"w":600.0,"h":48.0,"text":"让设计更简单","fill":"#1A1A2E"},
                        // i18n: user-facing
                        {"id":"hero-subtitle","kind":"text","x":200.0,"y":264.0,"w":500.0,"h":32.0,"text":"AI 驱动的本地设计工具","fill":"#666666"},
                        {"id":"cta-button","kind":"rect","x":200.0,"y":320.0,"w":160.0,"h":48.0,"fill":"#007AFF"},
                        // i18n: user-facing
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
        // 4. Mini program
        DesignTemplate {
            id: "builtin-mini-program".into(),
            // i18n: user-facing
            name: "小程序".into(),
            // i18n: user-facing
            tags: vec!["mini-program".into(), "wechat".into(), "小程序".into(), "微信".into()],
            category: "mini-program".into(),
            document_json: serde_json::json!({
                "pages": [{
                    // i18n: user-facing
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
        "builtin_scene_templates: returning builtin templates"
    );
    templates
}

impl EcosystemLink {
    /// Save the built-in scene templates into the KB directory (idempotent; skip if already present).
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
                // E-17/P3: the old implementation only checked file.exists() and skipped, leaving
                // corrupt/empty files unusable forever. Validate parseability first; overwrite and
                // reinstall if corrupt; skip only if valid.
                let need_overwrite = match std::fs::read_to_string(&file) {
                    Ok(content) => match serde_json::from_str::<DesignTemplate>(&content) {
                        Ok(existing) => existing.id != tmpl.id,
                        Err(e) => {
                            tracing::warn!(id = %tmpl.id, error = %e, "install_builtin_templates: existing file corrupt, overwriting and reinstalling");
                            true
                        }
                    },
                    Err(e) => {
                        tracing::warn!(id = %tmpl.id, error = %e, "install_builtin_templates: existing file unreadable, overwriting and reinstalling");
                        true
                    }
                };
                if !need_overwrite {
                    tracing::debug!(id = %tmpl.id, "install_builtin_templates: template already exists and is valid, skipping");
                    continue;
                }
            }
            let json = serde_json::to_string_pretty(&tmpl)?;
            std::fs::write(&file, json)?;
            installed += 1;
            tracing::info!(id = %tmpl.id, name = %tmpl.name, "install_builtin_templates: template installed");
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
            assert!(v.get("pages").is_some(), "template {} is missing pages", tmpl.id);
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
        // E-17/P3: the old implementation only checked existence and skipped, so a corrupt file
        // blocked reinstall forever. Install normally first, then write empty/corrupt content to
        // the first template file; reinstall must overwrite it with count > 0.
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
        assert_eq!(reinstalled, 1, "corrupt file must be overwritten and reinstalled (1 template)");
        let restored = std::fs::read_to_string(&corrupt_file).unwrap();
        let parsed: DesignTemplate = serde_json::from_str(&restored).unwrap();
        assert_eq!(parsed.id, *first_id, "reinstalled content is a valid builtin template");
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
        // Environment variable takes precedence over the default .venv path
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
        // A missing bin should fail visibly (fail-visible), without actually spawning the subprocess
        let err = client
            .run_sft(
                std::path::Path::new("/tmp/ds.jsonl"),
                "qwen2.5-7b-4bit",
                None,
            )
            .unwrap_err();
        assert!(format!("{}", err).contains("fusion-trainer CLI not found"));
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
        assert!(format!("{}", err).contains("fusion-trainer CLI not found"));
    }
}
