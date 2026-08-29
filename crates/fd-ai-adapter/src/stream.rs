// SSE stream parse helper. Extracted from chat_stream_messages
// unfold (ARCH-10 round-2). Triplicated parse logic -> single source.

use serde_json::Value;

use crate::MlxStreamDelta;

pub fn parse_sse_line(line: &str) -> Option<MlxStreamDelta> {
    let data = line.strip_prefix("data: ")?;
    if data == "[DONE]" {
        return Some(MlxStreamDelta {
            token: String::new(),
            finished: true,
        });
    }
    let parsed: Value = serde_json::from_str(data).ok()?;
    let content = parsed["choices"][0]["delta"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if content.is_empty() {
        return None;
    }
    Some(MlxStreamDelta {
        token: content,
        finished: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_marker_yields_finished_delta() {
        let d = parse_sse_line("data: [DONE]").expect("DONE -> Some");
        assert!(d.finished);
        assert!(d.token.is_empty());
    }

    #[test]
    fn content_line_yields_token_delta() {
        let body = r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#;
        let d = parse_sse_line(body).expect("content -> Some");
        assert!(!d.finished);
        assert_eq!(d.token, "hi");
    }

    #[test]
    fn empty_content_returns_none() {
        let body = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        assert!(parse_sse_line(body).is_none());
    }

    #[test]
    fn non_data_line_returns_none() {
        assert!(parse_sse_line(": keepalive comment").is_none());
        assert!(parse_sse_line("").is_none());
        assert!(parse_sse_line("event: ping").is_none());
    }

    #[test]
    fn bad_json_returns_none() {
        assert!(parse_sse_line("data: {not json").is_none());
    }

    #[test]
    fn cjk_content_preserved() {
        let body = r#"data: {"choices":[{"delta":{"content":"登录页"}}]}"#;
        let d = parse_sse_line(body).expect("cjk -> Some");
        assert_eq!(d.token, "登录页");
    }
}
