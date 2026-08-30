//! A-3：fd-cli 共享逻辑（原 main.rs 内联函数下沉）。
//!
//! 子命令 handler 共用的读取/注册表/备份/NDJSON 成帧工具集中于此，
//! 供 src/commands/*.rs 与 main.rs 内联 handler 复用。

use std::path::{Path, PathBuf};

/// stdin 单次读取上限（字节）。设计文档体量大，但远小于此；
/// 超限即拒绝，防巨输入 OOM。50MB 足够任何合法 .fusiondesign。
const STDIN_READ_CAP: usize = 50 * 1024 * 1024;

// FAULT-2：纯函数，累加 bytes 一次性 from_utf8。供 read_stdin_capped 复用 + 单测。
pub fn decode_stdin_bytes(buf: Vec<u8>) -> anyhow::Result<String> {
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("stdin 输入非 UTF-8: {e}"))
}

/// 分块读 stdin 至 STDIN_READ_CAP，超限 bail + warn。
pub fn read_stdin_capped() -> anyhow::Result<String> {
    use std::io::Read;
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = handle.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        // FAULT-2：字节累加原始 bytes，不在循环内转 UTF-8。
        // chunk 可能在多字节字符中间切断，循环内 from_utf8_lossy 会用 U+FFFD 替换损坏字节。
        // 末尾一次性 from_utf8 保证边界完整，无效 UTF-8 返 Err fail visibly（不静默替换）。
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > STDIN_READ_CAP {
            tracing::warn!("stdin 超过 {STDIN_READ_CAP} 字节上限，拒绝读取");
            anyhow::bail!("stdin 输入超过 {STDIN_READ_CAP} 字节上限，拒绝读取防 OOM");
        }
    }
    decode_stdin_bytes(buf)
}

/// 文件读取上限（字节），复用 stdin 上限语义。
const FILE_READ_CAP: u64 = STDIN_READ_CAP as u64;

/// E-10：备份轮转——保留最近 3 份。命名 `.fusiondesign.bak`（最新）→ `.bak.1` → `.bak.2`。
/// 每次 fix 前将 .bak.2 删除、.bak.1→.bak.2、.bak→.bak.1，返回新 .bak 路径供本次写入。
/// rename 失败仅 warn（尽力轮转，不阻断主流程）。
pub fn rotate_backup(input: &Path) -> PathBuf {
    let bak = input.with_extension("fusiondesign.bak");
    let bak1 = input.with_extension("fusiondesign.bak.1");
    let bak2 = input.with_extension("fusiondesign.bak.2");
    if bak2.exists() {
        let _ = std::fs::remove_file(&bak2);
    }
    if bak1.exists() {
        let _ = std::fs::rename(&bak1, &bak2);
    }
    if bak.exists() {
        let _ = std::fs::rename(&bak, &bak1);
    }
    bak
}

/// 受限读文件：metadata 预检 + take(cap) 限读 + from_utf8。
/// 超限 bail + warn，防巨文件 OOM。单 fd 闭环，无 TOCTOU（take 封顶）。
pub fn read_file_capped(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| anyhow::anyhow!("打开文件失败 {path:?}: {e}"))?;
    let len = std::fs::metadata(path)
        .map_err(|e| anyhow::anyhow!("读取元数据失败 {path:?}: {e}"))?
        .len();
    if len > FILE_READ_CAP {
        tracing::warn!(path = %path.display(), len, cap = FILE_READ_CAP, "文件超过上限，拒绝读取");
        anyhow::bail!("文件 {path:?} 大小 {len} 超过 {FILE_READ_CAP} 字节上限，拒绝读取防 OOM");
    }
    let mut buf = Vec::with_capacity(len.min(FILE_READ_CAP) as usize);
    f.take(FILE_READ_CAP).read_to_end(&mut buf)?;
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("文件 {path:?} 非 UTF-8: {e}"))
}

/// MultiVariants 风格选择（E-27/P3）。
/// 旧实现 `<3 styles` 静默 `_ => default_styles` 丢弃全部用户输入。
/// 改为优先用用户提供的 styles，不足 3 用默认补齐并 warn；None/空用全部默认。
pub fn pick_multi_variant_styles(styles: Option<Vec<String>>) -> [String; 3] {
    let default_styles = ["极简风", "卡片风", "深色风"];
    match styles {
        Some(s) if s.len() >= 3 => [s[0].clone(), s[1].clone(), s[2].clone()],
        Some(s) if !s.is_empty() => {
            let mut out: Vec<String> = s.into_iter().take(3).collect();
            let user_count = out.len();
            for d in default_styles {
                if out.len() >= 3 {
                    break;
                }
                if !out.iter().any(|x| x == d) {
                    out.push(d.to_string());
                }
            }
            tracing::warn!(
                provided = user_count,
                filled = 3 - user_count,
                "MultiVariants: 用户 styles 不足 3 个，已用默认风格补齐至 3 个"
            );
            [out[0].clone(), out[1].clone(), out[2].clone()]
        }
        _ => {
            tracing::info!("MultiVariants: 未提供 styles，使用默认三种风格");
            default_styles.map(|s| s.to_string())
        }
    }
}

// 构建设计规范注册表：加载内置规范，若文档声明了 active_design_system 则激活之。
// 用于导出路径解析 token 颜色变量（#8），避免 var(--) 被 usvg 回退成黑色。
pub fn build_registry(doc: &fd_canvas_core::PenDocument) -> fd_cli::design::DesignSystemRegistry {
    let mut reg = fd_cli::design::DesignSystemRegistry::new();
    reg.register_builtin();
    if let Some(ref id) = doc.active_design_system {
        match reg.activate(id) {
            Ok(()) => tracing::info!(design_system = %id, "已激活文档声明的设计规范"),
            Err(e) => {
                tracing::warn!(design_system = %id, error = %e, "文档声明的设计规范不存在，使用默认")
            }
        }
    }
    reg
}

// H-A16/P1-8：NDJSON 成帧函数抽出为纯函数，便于回归测试本子命令自洽契约。
// 三帧 schema：delta / chat_done / error。供 CLI 管道/脚本消费（issue #17）。
pub fn ndjson_frame_delta(token: &str) -> serde_json::Value {
    serde_json::json!({"type":"delta","token":token})
}

pub fn ndjson_frame_done() -> serde_json::Value {
    serde_json::json!({"type":"chat_done","finish_reason":"stop"})
}

pub fn ndjson_frame_error(message: &str) -> serde_json::Value {
    serde_json::json!({"type":"error","message":message})
}

// issue #20：SSE 成帧（raw OpenAI text/event-stream 格式），对齐 fusion-studio
// DesignBridge.runFusionDesignStream 的 stdout 解析器——按 `data: ` 前缀逐行解析，
// 取 choices[0].delta.content 为 token，遇 `data: [DONE]` 结束。studio 现有子进程管道
// 基础设施（readabilityHandler）零改动即可消费此格式。鉴权 / RouteGuard / endpoint
// 解析仍复用 fd-ai-adapter，调用方不重实现。
pub fn sse_frame_delta(token: &str) -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({"choices":[{"index":0,"delta":{"content":token}}]})
    )
}

pub fn sse_frame_done() -> String {
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})
    )
}

pub fn sse_frame_error(message: &str) -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({"error":{"message":message}})
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_stdin_bytes_multibyte_split() {
        // 「中」= 0xE4 0xB8 0xAD。把首字符的后 2 字节放 chunk 末、前 1 字节放下 chunk 首，
        // 模拟 chunk 边界切断。旧 from_utf8_lossy 逐 chunk 转会丢字节。
        let mut bytes = vec![0xE4, 0xB8, 0xAD]; // 完整「中」
        bytes.push(0xE4); // 切断：下字符首字节孤立
        let mut full = bytes.clone();
        full.push(0xB8);
        full.push(0xAD); // 拼「中中」
        let got = decode_stdin_bytes(full).unwrap();
        assert_eq!(got, "中中");
    }

    #[test]
    fn sse_frame_delta_shape() {
        let f = sse_frame_delta("hello");
        assert!(
            f.starts_with("data: "),
            "delta must start with data: prefix"
        );
        assert!(f.contains("choices"), "delta must contain choices array");
        assert!(f.contains("hello"), "delta must carry token content");
        assert_eq!(
            f.lines().count(),
            2,
            "delta = one data line + trailing blank line"
        );
    }

    #[test]
    fn sse_frame_done_shape() {
        let f = sse_frame_done();
        assert!(f.contains("finish_reason"), "done must carry finish_reason");
        assert!(f.contains("stop"), "done finish_reason must be stop");
        assert!(
            f.contains("[DONE]"),
            "done must terminate with data: [DONE]"
        );
        assert_eq!(
            f.lines().count(),
            4,
            "done = json line + blank + [DONE] line + blank"
        );
    }

    #[test]
    fn sse_frame_error_shape() {
        let f = sse_frame_error("boom");
        assert!(
            f.starts_with("data: "),
            "error must start with data: prefix"
        );
        assert!(f.contains("error"), "error must contain error object");
        assert!(f.contains("boom"), "error must carry message");
        assert_eq!(
            f.lines().count(),
            2,
            "error = one data line + trailing blank line"
        );
    }
}
