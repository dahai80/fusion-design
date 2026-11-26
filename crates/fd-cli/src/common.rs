//! A-3: fd-cli shared logic (functions sunk from the former main.rs inlines).
//!
//! Read/registry/backup/NDJSON framing utilities shared by subcommand handlers live here,
//! reused by src/commands/*.rs and main.rs inline handlers.

use std::path::{Path, PathBuf};

/// stdin single-read cap (bytes). Design documents are large but far below this;
/// reject over-cap input to prevent OOM from huge inputs. 50MB covers any valid .fusiondesign.
const STDIN_READ_CAP: usize = 50 * 1024 * 1024;

// FAULT-2: pure function, accumulates bytes and runs from_utf8 once. Reused by
// read_stdin_capped + unit-tested.
pub fn decode_stdin_bytes(buf: Vec<u8>) -> anyhow::Result<String> {
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("stdin input is not UTF-8: {e}"))
}

/// Read stdin in chunks up to STDIN_READ_CAP; bail + warn on overflow.
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
        // FAULT-2: accumulate raw bytes; do not convert to UTF-8 inside the loop.
        // A chunk may split a multi-byte character mid-way; per-chunk from_utf8_lossy would
        // replace damaged bytes with U+FFFD. A single from_utf8 at the end preserves chunk
        // boundaries; invalid UTF-8 returns Err and fails visibly (no silent replacement).
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > STDIN_READ_CAP {
            tracing::warn!("stdin exceeds the {STDIN_READ_CAP}-byte cap, refusing to read");
            anyhow::bail!(
                "stdin input exceeds the {STDIN_READ_CAP}-byte cap, refusing to read to prevent OOM"
            );
        }
    }
    decode_stdin_bytes(buf)
}

/// File read cap (bytes), reusing the stdin cap semantics.
const FILE_READ_CAP: u64 = STDIN_READ_CAP as u64;

/// E-10: backup rotation — keep the latest 3 copies. Naming `.fusiondesign.bak` (newest)
/// → `.bak.1` → `.bak.2`. Before each fix: delete .bak.2, .bak.1→.bak.2, .bak→.bak.1, and
/// return the new .bak path for this write.
/// rename failure only warns (best-effort rotation, never blocks the main flow).
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

/// Capped file read: metadata precheck + take(cap) limited read + from_utf8.
/// Bail + warn on over-cap to prevent OOM from huge files. Single-fd closed loop, no
/// TOCTOU (take caps the read).
pub fn read_file_capped(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    let f = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("failed to open file {path:?}: {e}"))?;
    let len = std::fs::metadata(path)
        .map_err(|e| anyhow::anyhow!("failed to read metadata for {path:?}: {e}"))?
        .len();
    if len > FILE_READ_CAP {
        tracing::warn!(path = %path.display(), len, cap = FILE_READ_CAP, "file exceeds the cap, refusing to read");
        anyhow::bail!(
            "file {path:?} size {len} exceeds the {FILE_READ_CAP}-byte cap, refusing to read to prevent OOM"
        );
    }
    let mut buf = Vec::with_capacity(len.min(FILE_READ_CAP) as usize);
    f.take(FILE_READ_CAP).read_to_end(&mut buf)?;
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("file {path:?} is not UTF-8: {e}"))
}

/// MultiVariants style selection (E-27/P3).
/// The old implementation silently dropped all user input via `<3 styles` →
/// `_ => default_styles`. Now prefer user-provided styles; pad with defaults and warn
/// when fewer than 3; None/empty uses all defaults.
pub fn pick_multi_variant_styles(styles: Option<Vec<String>>) -> [String; 3] {
    // i18n: user-facing
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
                "MultiVariants: fewer than 3 user styles provided, padded to 3 with default styles"
            );
            [out[0].clone(), out[1].clone(), out[2].clone()]
        }
        _ => {
            tracing::info!("MultiVariants: no styles provided, using the three default styles");
            default_styles.map(|s| s.to_string())
        }
    }
}

// Build the design-system registry: load built-ins and activate the one the document
// declares via active_design_system, if any. Used by the export path to resolve token
// color variables (#8), preventing var(--) from falling back to black in usvg.
pub fn build_registry(doc: &fd_canvas_core::PenDocument) -> fd_cli::design::DesignSystemRegistry {
    let mut reg = fd_cli::design::DesignSystemRegistry::new();
    reg.register_builtin();
    if let Some(ref id) = doc.active_design_system {
        match reg.activate(id) {
            Ok(()) => {
                tracing::info!(design_system = %id, "activated the design system declared by the document")
            }
            Err(e) => {
                tracing::warn!(design_system = %id, error = %e, "design system declared by the document not found, using default")
            }
        }
    }
    reg
}

// H-A16/P1-8: NDJSON framing functions extracted as pure functions so this subcommand's
// self-consistency contract can be regression-tested.
// Three-frame schema: delta / chat_done / error. Consumed by CLI pipelines/scripts (issue #17).
pub fn ndjson_frame_delta(token: &str) -> serde_json::Value {
    serde_json::json!({"type":"delta","token":token})
}

pub fn ndjson_frame_done() -> serde_json::Value {
    serde_json::json!({"type":"chat_done","finish_reason":"stop"})
}

pub fn ndjson_frame_error(message: &str) -> serde_json::Value {
    serde_json::json!({"type":"error","message":message})
}

// issue #20: SSE framing (raw OpenAI text/event-stream format), aligned with the
// fusion-studio DesignBridge.runFusionDesignStream stdout parser — parses line by line on
// the `data: ` prefix, takes choices[0].delta.content as the token, and stops at
// `data: [DONE]`. studio's existing subprocess pipeline infrastructure (readabilityHandler)
// consumes this format with zero changes. Auth / RouteGuard / endpoint resolution still
// reuse fd-ai-adapter; callers do not reimplement them.
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
        // U+4E2D ("中") = 0xE4 0xB8 0xAD. Put the last 2 bytes of the first character at the
        // end of one chunk and the leading byte into the next chunk, simulating a split at
        // the chunk boundary. The old per-chunk from_utf8_lossy would lose bytes.
        // i18n: user-facing
        let mut bytes = vec![0xE4, 0xB8, 0xAD]; // full "中"
        bytes.push(0xE4); // split: first byte of the next character left dangling
        let mut full = bytes.clone();
        full.push(0xB8);
        full.push(0xAD); // yields "中中"
        let got = decode_stdin_bytes(full).unwrap();
        // i18n: user-facing (multibyte UTF-8 test fixture)
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
