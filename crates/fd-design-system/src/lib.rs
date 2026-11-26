//! Fusion-Design design system — three built-in presets + Token management.
//!
//! Corresponds to PRD module 3 "Local design system and component library".
//! Global tokens (color/font size/spacing/radius/shadow) are defined centrally and
//! synced to all pages with one click.

use std::collections::HashMap;

use fd_canvas_core::sanitize_css_value;
use serde::{Deserialize, Serialize};

/// P-5: token reference chain depth cap. Prevents stack overflow from very long non-cyclic
/// chains (visited only guards cycles, not depth).
const MAX_REFERENCE_DEPTH: u32 = 64;
/// P-5: sane cap for import_json. Design systems have far fewer tokens than this; anything
/// over the cap is treated as malicious/erroneous input.
const MAX_IMPORT_TOKENS: usize = 10000;
/// P-5: per-token string value length cap (bytes). Prevents huge strings from hogging memory.
const MAX_TOKEN_STR_LEN: usize = 4096;

/// Design token value type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TokenValue {
    Color(String),  // hex, e.g. "#FFFFFF"
    Number(f32),    // font size/spacing/radius
    Shadow(String), // CSS box-shadow
    String(String), // font family etc. / token:xxx reference
}

impl TokenValue {
    /// Convert to a CSS property value string.
    /// - Color → outputs the hex directly
    /// - Number → outputs `Npx` (font size/spacing/radius default to px)
    /// - Shadow → outputs the CSS box-shadow directly
    /// - String → outputs directly (font family etc.); a `token:xxx` reference outputs `var(--xxx)`
    pub fn to_css_value(&self) -> String {
        match self {
            TokenValue::Color(c) => sanitize_css_value(c, "transparent"),
            TokenValue::Number(n) => format!("{}px", n),
            TokenValue::Shadow(s) => sanitize_css_value(s, "transparent"),
            TokenValue::String(s) => {
                if let Some(ref_name) = s.strip_prefix("token:") {
                    format!("var(--{})", ref_name)
                } else {
                    sanitize_css_value(s, "")
                }
            }
        }
    }

    /// Check whether this is a token reference (`token:xxx` format).
    pub fn is_reference(&self) -> bool {
        match self {
            TokenValue::String(s) => s.starts_with("token:"),
            _ => false,
        }
    }

    /// Extract the reference target name (`token:xxx` → `xxx`).
    pub fn reference_target(&self) -> Option<&str> {
        match self {
            TokenValue::String(s) => s.strip_prefix("token:"),
            _ => None,
        }
        .filter(|s| !s.is_empty())
    }
}

/// A single token definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub name: String,
    pub value: TokenValue,
    pub description: String,
}

/// Design system (a set of tokens).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignSystem {
    pub id: String,
    pub name: String,
    pub tokens: Vec<Token>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark_tokens: Option<Vec<Token>>,
    // A-8: name of the parent system to inherit from (e.g. "apple-hig"). A custom system may
    // override only individual tokens, inheriting the rest from the parent. None means no parent
    // (builtin systems / fully resolved systems).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
}

/// Theme mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

impl DesignSystem {
    /// Generate CSS Custom Properties output: `:root { --token-name: value; ... }`
    /// `.` in token names is replaced with `-` to comply with CSS custom property naming.
    /// Reference tokens (`token:xxx`) are recursively resolved to their final value.
    /// P-5: build the token map once up front; resolution does O(1) lookups, avoiding an
    /// O(n) find inside the loop = O(n²).
    pub fn to_css_custom_properties(&self) -> String {
        let map = self.token_map();
        let mut lines = vec![":root {".to_string()];
        let mut visited = std::collections::HashSet::new();
        for token in &self.tokens {
            let css_name = token.name.replace('.', "-");
            let resolved = self.resolve_reference_inner(&token.value, &mut visited, &map, 0);
            lines.push(format!("  --{}: {};", css_name, resolved));
            visited.clear();
        }
        lines.push("}".to_string());
        lines.join("\n")
    }

    /// Resolve a token reference: if the value is `token:xxx`, look up the target token's value
    /// and resolve recursively. Guards against circular references (visited set) with a depth
    /// cap of 64 (prevents stack overflow from very long non-cyclic chains).
    pub fn resolve_reference(
        &self,
        value: &TokenValue,
        visited: &mut std::collections::HashSet<String>,
    ) -> String {
        let map = self.token_map();
        self.resolve_reference_inner(value, visited, &map, 0)
    }

    /// Reference resolution internals: O(1) token map lookup + explicit depth counting.
    fn resolve_reference_inner(
        &self,
        value: &TokenValue,
        visited: &mut std::collections::HashSet<String>,
        map: &std::collections::HashMap<&str, &TokenValue>,
        depth: u32,
    ) -> String {
        if let Some(target) = value.reference_target() {
            if depth >= MAX_REFERENCE_DEPTH {
                tracing::warn!(
                    target,
                    depth = MAX_REFERENCE_DEPTH,
                    "token reference chain exceeded the depth cap, falling back to the original CSS var"
                );
                return value.to_css_value();
            }
            if visited.contains(target) {
                tracing::warn!(
                    "circular token reference detected: {:?}, visited: {:?}",
                    target,
                    visited
                );
                return value.to_css_value();
            }
            visited.insert(target.to_string());
            if let Some(resolved) = map.get(target).copied() {
                self.resolve_reference_inner(resolved, visited, map, depth + 1)
            } else {
                tracing::warn!("token reference target not found: {}", target);
                value.to_css_value()
            }
        } else {
            value.to_css_value()
        }
    }

    /// P-5: pre-build a token name→value map (O(1) lookup). Borrows self.tokens; lifetime tied to self.
    fn token_map(&self) -> std::collections::HashMap<&str, &TokenValue> {
        self.tokens
            .iter()
            .map(|t| (t.name.as_str(), &t.value))
            .collect()
    }

    /// Generate CSS Custom Properties for a theme mode.
    /// Light uses self.tokens; Dark uses self.dark_tokens (falling back to tokens if absent).
    pub fn to_css_custom_properties_for_theme(&self, theme: Theme) -> String {
        // E-15: the old Dark-theme implementation used dark_tokens directly (if only colors were
        // overridden and font_size/radius were not listed, those tokens were lost → missing CSS
        // variables). Changed to use the light tokens as the base with a dark overlay on top.
        let merged: Vec<Token> = match theme {
            Theme::Light => self.tokens.clone(),
            Theme::Dark => {
                let mut by_name: std::collections::HashMap<String, Token> = self
                    .tokens
                    .iter()
                    .map(|t| (t.name.clone(), t.clone()))
                    .collect();
                if let Some(dark) = &self.dark_tokens {
                    for t in dark {
                        by_name.insert(t.name.clone(), t.clone());
                    }
                }
                // Keep the light tokens' declaration order; dark-only tokens are appended at the end.
                let mut merged: Vec<Token> = self
                    .tokens
                    .iter()
                    .map(|t| by_name.remove(&t.name).unwrap_or_else(|| t.clone()))
                    .collect();
                if let Some(dark) = &self.dark_tokens {
                    for t in dark {
                        if !self.tokens.iter().any(|lt| lt.name == t.name) {
                            merged.push(t.clone());
                        }
                    }
                }
                merged
            }
        };
        let tokens = &merged;
        // P-5: build the map from the merged slice, reusing the inner resolver.
        let map: std::collections::HashMap<&str, &TokenValue> =
            tokens.iter().map(|t| (t.name.as_str(), &t.value)).collect();
        let mut lines = vec![":root {".to_string()];
        let mut visited = std::collections::HashSet::new();
        for token in tokens {
            let css_name = token.name.replace('.', "-");
            let resolved = self.resolve_reference_inner(&token.value, &mut visited, &map, 0);
            lines.push(format!("  --{}: {};", css_name, resolved));
            visited.clear();
        }
        lines.push("}".to_string());
        lines.join("\n")
    }

    // A-8: use the parent system's tokens as the base, overlaying self's tokens on top
    // (self wins on name collisions). Only used for inheritance-chain resolution; does not
    // change self's extends field.
    // light tokens: parent first + self overrides, keeping the parent's declaration order,
    // self-only tokens appended at the end.
    // dark_tokens: each side's dark overlay applied first, then overridden (None = empty dark set).
    pub fn merge_parent(&self, parent: &DesignSystem) -> DesignSystem {
        let merged_tokens = overlay_tokens(&parent.tokens, &self.tokens);
        let merged_dark = match (&parent.dark_tokens, &self.dark_tokens) {
            (Some(pd), Some(sd)) => Some(overlay_tokens(pd, sd)),
            (Some(pd), None) => Some(pd.clone()),
            (None, Some(sd)) => Some(sd.clone()),
            (None, None) => None,
        };
        DesignSystem {
            id: self.id.clone(),
            name: self.name.clone(),
            tokens: merged_tokens,
            dark_tokens: merged_dark,
            // The merged result has no parent dependency; clear extends (preventing the resolved
            // product from being treated as inheritable again).
            extends: None,
        }
    }
}

/// A-8: base first, overlay overrides same-name tokens, overlay-only tokens appended at the end.
/// Preserves the base declaration order.
fn overlay_tokens(base: &[Token], overlay: &[Token]) -> Vec<Token> {
    let mut by_name: HashMap<String, Token> =
        base.iter().map(|t| (t.name.clone(), t.clone())).collect();
    for t in overlay {
        by_name.insert(t.name.clone(), t.clone());
    }
    let mut merged: Vec<Token> = base
        .iter()
        .map(|t| by_name.remove(&t.name).unwrap_or_else(|| t.clone()))
        .collect();
    for t in overlay {
        if !base.iter().any(|b| b.name == t.name) {
            merged.push(t.clone());
        }
    }
    merged
}

/// Design system registry (manages multiple systems, supports one-click switching).
#[derive(Debug, Default)]
pub struct DesignSystemRegistry {
    systems: HashMap<String, DesignSystem>,
    active_id: Option<String>,
    active_theme: Theme,
}

impl DesignSystemRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a design system.
    pub fn register(&mut self, system: DesignSystem) -> Result<(), DuplicateError> {
        if self.systems.contains_key(&system.id) {
            return Err(DuplicateError(system.id));
        }
        self.systems.insert(system.id.clone(), system);
        Ok(())
    }

    /// Activate a system (switches the global tokens).
    pub fn activate(&mut self, id: &str) -> Result<(), NotFoundError> {
        if !self.systems.contains_key(id) {
            return Err(NotFoundError(id.to_string()));
        }
        self.active_id = Some(id.to_string());
        Ok(())
    }

    /// Switch the theme mode (Light/Dark).
    pub fn activate_theme(&mut self, theme: Theme) {
        self.active_theme = theme;
        tracing::info!(theme = ?theme, "theme switched");
    }

    /// Get the current theme mode.
    pub fn current_theme(&self) -> Theme {
        self.active_theme
    }

    /// Get the currently active system.
    pub fn active(&self) -> Option<&DesignSystem> {
        self.active_id.as_ref().and_then(|id| self.systems.get(id))
    }

    /// List all registered system IDs.
    pub fn list(&self) -> Vec<&str> {
        self.systems.keys().map(|s| s.as_str()).collect()
    }

    /// Get a system by ID.
    pub fn get(&self, id: &str) -> Option<&DesignSystem> {
        self.systems.get(id)
    }

    // A-8: recursively resolve the inheritance chain, returning a full DesignSystem (parent
    // tokens as base, child overlaid layer by layer).
    // Cycle detection: the visited set records visited ids; a repeat visit → Err(ResolveError::Circular).
    // Parent system not registered → Err(ResolveError::NotFound).
    // No extends → returns a clone of self (extends dependency removed; result has extends=None).
    pub fn resolve(&self, id: &str) -> Result<DesignSystem, ResolveError> {
        let mut visited: Vec<String> = Vec::new();
        self.resolve_inner(id, &mut visited)
    }

    fn resolve_inner(
        &self,
        id: &str,
        visited: &mut Vec<String>,
    ) -> Result<DesignSystem, ResolveError> {
        if visited.iter().any(|v| v == id) {
            tracing::warn!(system_id = %id, chain = ?visited, "inheritance cycle detected");
            return Err(ResolveError::Circular {
                id: id.to_string(),
                chain: visited.clone(),
            });
        }
        let system = self
            .systems
            .get(id)
            .ok_or_else(|| ResolveError::NotFound(id.to_string()))?;
        visited.push(id.to_string());
        match &system.extends {
            Some(parent_id) => {
                let parent = self.resolve_inner(parent_id, visited)?;
                Ok(system.merge_parent(&parent))
            }
            None => {
                // Leaf system: clone and clear extends (self-contained result, no parent dependency).
                let mut out = system.clone();
                out.extends = None;
                Ok(out)
            }
        }
    }

    /// Register the builtin systems (Apple HIG / minimal dashboard / robot-sim console / Desigma Aligned).
    /// Idempotent: existing IDs are skipped with a warning and never overwrite user-customized or
    /// currently active systems (E-32/P3).
    pub fn register_builtin(&mut self) {
        let builtins = [
            ("apple-hig".to_string(), builtin_apple_hig()),
            ("minimal-dashboard".to_string(), builtin_minimal_dashboard()),
            ("robot-sim".to_string(), builtin_robot_sim()),
            ("desigma-aligned".to_string(), builtin_desigma_aligned()),
        ];
        let total = builtins.len();
        let mut skipped = 0usize;
        for (id, system) in builtins {
            if self.systems.contains_key(&id) {
                tracing::warn!(system_id = %id, "register_builtin: system already exists, skipping without overwrite");
                skipped += 1;
                continue;
            }
            self.systems.insert(id, system);
        }
        if skipped > 0 {
            tracing::warn!(
                skipped,
                total,
                "register_builtin: skipped {skipped}/{total} existing builtin systems"
            );
        } else {
            tracing::info!("register_builtin: registered {total} builtin systems");
        }
    }

    /// Look up a token value by name (within the currently active system).
    pub fn lookup(&self, token_name: &str) -> Option<&TokenValue> {
        self.active()?
            .tokens
            .iter()
            .find(|t| t.name == token_name)
            .map(|t| &t.value)
    }

    /// Export as JSON (for local team sharing of design systems).
    pub fn export_json(&self) -> serde_json::Result<String> {
        let active = self.active();
        serde_json::to_string_pretty(&active)
    }

    /// Import a system from JSON.
    /// P-5: token count / per-value length / reference depth are capped to guard against
    /// malicious or erroneous oversized input.
    pub fn import_json(&mut self, json: &str) -> Result<(), ImportError> {
        let system: DesignSystem = serde_json::from_str(json).map_err(ImportError::Parse)?;
        if system.tokens.len() > MAX_IMPORT_TOKENS {
            tracing::warn!(
                count = system.tokens.len(),
                cap = MAX_IMPORT_TOKENS,
                "import_json: token count exceeds the cap, rejecting import"
            );
            return Err(ImportError::TooManyTokens(system.tokens.len()));
        }
        for tk in &system.tokens {
            validate_token_value(&tk.value)?;
        }
        if let Some(dark) = &system.dark_tokens {
            if dark.len() > MAX_IMPORT_TOKENS {
                tracing::warn!(
                    count = dark.len(),
                    cap = MAX_IMPORT_TOKENS,
                    "import_json: dark token count exceeds the cap, rejecting import"
                );
                return Err(ImportError::TooManyTokens(dark.len()));
            }
            for tk in dark {
                validate_token_value(&tk.value)?;
            }
        }
        self.systems.insert(system.id.clone(), system);
        Ok(())
    }
}

/// P-5: validate a single token value's string length (Color/Shadow/String variants).
fn validate_token_value(v: &TokenValue) -> Result<(), ImportError> {
    let s: Option<&str> = match v {
        TokenValue::Color(s) | TokenValue::Shadow(s) | TokenValue::String(s) => Some(s),
        TokenValue::Number(_) => None,
    };
    if let Some(s) = s {
        if s.len() > MAX_TOKEN_STR_LEN {
            tracing::warn!(
                len = s.len(),
                cap = MAX_TOKEN_STR_LEN,
                "token string value too long, rejecting import"
            );
            return Err(ImportError::ValueTooLong(s.len()));
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("design system {0} already registered")]
pub struct DuplicateError(pub String);

#[derive(Debug, thiserror::Error)]
#[error("design system {0} not found")]
pub struct NotFoundError(pub String);

// A-8: inheritance-chain resolution errors.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("design system {0} not found")]
    NotFound(String),
    #[error("inheritance cycle detected: system {id} appears repeatedly, chain {chain:?}")]
    Circular { id: String, chain: Vec<String> },
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("failed to parse imported JSON: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("token count {0} exceeds the cap {MAX_IMPORT_TOKENS}")]
    TooManyTokens(usize),
    #[error("token string value length {0} exceeds the cap {MAX_TOKEN_STR_LEN}")]
    ValueTooLong(usize),
}

// ── Builtin systems ──

pub fn builtin_apple_hig() -> DesignSystem {
    DesignSystem {
        id: "apple-hig".into(),
        name: "Apple HIG".into(),
        tokens: vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#F2F2F7".into()),
                // i18n: user-facing
                description: "系统背景".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#1C1C1E".into()),
                // i18n: user-facing
                description: "主前景".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#007AFF".into()),
                // i18n: user-facing
                description: "系统强调色".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(17.0),
                // i18n: user-facing
                description: "正文字号".into(),
            },
            Token {
                name: "radius.card".into(),
                value: TokenValue::Number(10.0),
                // i18n: user-facing
                description: "卡片圆角".into(),
            },
        ],
        extends: None,
        dark_tokens: Some(vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#1C1C1E".into()),
                // i18n: user-facing
                description: "暗色背景".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#F2F2F7".into()),
                // i18n: user-facing
                description: "暗色前景".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#0A84FF".into()),
                // i18n: user-facing
                description: "暗色强调".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(17.0),
                // i18n: user-facing
                description: "正文字号".into(),
            },
            Token {
                name: "radius.card".into(),
                value: TokenValue::Number(10.0),
                // i18n: user-facing
                description: "卡片圆角".into(),
            },
        ]),
    }
}

fn builtin_minimal_dashboard() -> DesignSystem {
    DesignSystem {
        id: "minimal-dashboard".into(),
        // i18n: user-facing
        name: "极简后台".into(),
        tokens: vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#FFFFFF".into()),
                // i18n: user-facing
                description: "纯白背景".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#374151".into()),
                // i18n: user-facing
                description: "深灰前景".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#10B981".into()),
                // i18n: user-facing
                description: "绿色强调".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(14.0),
                // i18n: user-facing
                description: "正文字号".into(),
            },
            Token {
                name: "radius.card".into(),
                value: TokenValue::Number(6.0),
                // i18n: user-facing
                description: "卡片圆角".into(),
            },
        ],
        extends: None,
        dark_tokens: None,
    }
}

/// Robot-simulation console preset (PRD module 3, third builtin system).
/// Aimed at Fusion-Simulation integration: dark engineering UI, status/alert/telemetry
/// semantic colors, monospace data font.
pub fn builtin_robot_sim() -> DesignSystem {
    DesignSystem {
        id: "robot-sim".into(),
        // i18n: user-facing
        name: "机器人仿真控制台".into(),
        tokens: vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#0F1115".into()),
                // i18n: user-facing
                description: "控制台深色背景".into(),
            },
            Token {
                name: "color.surface".into(),
                value: TokenValue::Color("#1A1F26".into()),
                // i18n: user-facing
                description: "面板/卡片表面".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#E6EDF3".into()),
                // i18n: user-facing
                description: "主前景（高对比浅色）".into(),
            },
            Token {
                name: "color.muted".into(),
                value: TokenValue::Color("#7D8590".into()),
                // i18n: user-facing
                description: "次要文本/标注".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#2F81F7".into()),
                // i18n: user-facing
                description: "主交互强调（蓝）".into(),
            },
            Token {
                name: "color.success".into(),
                value: TokenValue::Color("#3FB950".into()),
                // i18n: user-facing
                description: "运行正常/在线".into(),
            },
            Token {
                name: "color.warning".into(),
                value: TokenValue::Color("#D29922".into()),
                // i18n: user-facing
                description: "告警/阈值临近".into(),
            },
            Token {
                name: "color.danger".into(),
                value: TokenValue::Color("#F85149".into()),
                // i18n: user-facing
                description: "故障/急停/离线".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(13.0),
                // i18n: user-facing
                description: "正文字号（紧凑工程界面）".into(),
            },
            Token {
                name: "font.size.mono".into(),
                value: TokenValue::Number(12.0),
                // i18n: user-facing
                description: "遥测/日志等宽数据字号".into(),
            },
            Token {
                name: "font.mono".into(),
                value: TokenValue::String("SF Mono, Menlo, monospace".into()),
                // i18n: user-facing
                description: "等宽字体族（遥测/坐标/日志）".into(),
            },
            Token {
                name: "radius.panel".into(),
                value: TokenValue::Number(4.0),
                // i18n: user-facing
                description: "面板圆角（锐利工程风）".into(),
            },
            Token {
                name: "spacing.gauge".into(),
                value: TokenValue::Number(8.0),
                // i18n: user-facing
                description: "仪表/控件间距基准".into(),
            },
            Token {
                name: "shadow.panel".into(),
                value: TokenValue::Shadow("0 1px 0 rgba(0,0,0,0.4)".into()),
                // i18n: user-facing
                description: "面板内阴影（深色浮起）".into(),
            },
        ],
        extends: None,
        dark_tokens: Some(vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#010409".into()),
                // i18n: user-facing
                description: "夜间极深背景".into(),
            },
            Token {
                name: "color.surface".into(),
                value: TokenValue::Color("#0D1117".into()),
                // i18n: user-facing
                description: "夜间面板表面".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#F0F6FC".into()),
                // i18n: user-facing
                description: "夜间高对比前景".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#1F6FEB".into()),
                // i18n: user-facing
                description: "夜间强调".into(),
            },
            Token {
                name: "color.success".into(),
                value: TokenValue::Color("#238636".into()),
                // i18n: user-facing
                description: "夜间运行正常".into(),
            },
            Token {
                name: "color.danger".into(),
                value: TokenValue::Color("#DA3633".into()),
                // i18n: user-facing
                description: "夜间故障".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(13.0),
                // i18n: user-facing
                description: "正文字号".into(),
            },
            Token {
                name: "radius.panel".into(),
                value: TokenValue::Number(4.0),
                // i18n: user-facing
                description: "面板圆角".into(),
            },
        ]),
    }
}

// Issue #41: recommended preset for desigma-sourced customer designs. Semantic
// names mirror the shell design language (brand #4F7CFF, 8px spacing scale);
// every color token ships a dark pair so the desigma gate's missing_dark_pair
// rule passes out of the box. Desigma Theme collection groups map 1:1:
// Surface -> color.bg/color.surface, Text -> color.fg(.muted), Border ->
// color.border, Brand -> color.accent, Status -> color.success/error.
pub fn builtin_desigma_aligned() -> DesignSystem {
    DesignSystem {
        id: "desigma-aligned".into(),
        name: "Desigma Aligned".into(),
        tokens: vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#FFFFFF".into()),
                description: "Page background".into(),
            },
            Token {
                name: "color.surface".into(),
                value: TokenValue::Color("#F5F6F8".into()),
                description: "Card / panel surface".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#1C1E21".into()),
                description: "Primary text".into(),
            },
            Token {
                name: "color.fg.muted".into(),
                value: TokenValue::Color("#65676D".into()),
                description: "Secondary text".into(),
            },
            Token {
                name: "color.border".into(),
                value: TokenValue::Color("#E3E5E9".into()),
                description: "Hairline border".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#4F7CFF".into()),
                description: "Brand accent (shell-aligned)".into(),
            },
            Token {
                name: "color.success".into(),
                value: TokenValue::Color("#34C759".into()),
                description: "Positive status".into(),
            },
            Token {
                name: "color.error".into(),
                value: TokenValue::Color("#FF453A".into()),
                description: "Error status".into(),
            },
            Token {
                name: "spacing.sm".into(),
                value: TokenValue::Number(8.0),
                description: "Compact spacing".into(),
            },
            Token {
                name: "spacing.md".into(),
                value: TokenValue::Number(16.0),
                description: "Base spacing".into(),
            },
            Token {
                name: "spacing.lg".into(),
                value: TokenValue::Number(24.0),
                description: "Section spacing".into(),
            },
            Token {
                name: "radius.sm".into(),
                value: TokenValue::Number(4.0),
                description: "Small radius".into(),
            },
            Token {
                name: "radius.md".into(),
                value: TokenValue::Number(8.0),
                description: "Card radius (shell-aligned)".into(),
            },
            Token {
                name: "radius.lg".into(),
                value: TokenValue::Number(16.0),
                description: "Sheet / modal radius".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(16.0),
                description: "Body font size".into(),
            },
        ],
        extends: None,
        dark_tokens: Some(vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#0F1115".into()),
                description: "Dark page background".into(),
            },
            Token {
                name: "color.surface".into(),
                value: TokenValue::Color("#161A22".into()),
                description: "Dark card surface".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#E8EAF0".into()),
                description: "Dark primary text".into(),
            },
            Token {
                name: "color.fg.muted".into(),
                value: TokenValue::Color("#9AA0AB".into()),
                description: "Dark secondary text".into(),
            },
            Token {
                name: "color.border".into(),
                value: TokenValue::Color("#2A2F3A".into()),
                description: "Dark hairline border".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#6D93FF".into()),
                description: "Dark brand accent".into(),
            },
            Token {
                name: "color.success".into(),
                value: TokenValue::Color("#30D158".into()),
                description: "Dark positive status".into(),
            },
            Token {
                name: "color.error".into(),
                value: TokenValue::Color("#FF6961".into()),
                description: "Dark error status".into(),
            },
            Token {
                name: "spacing.sm".into(),
                value: TokenValue::Number(8.0),
                description: "Compact spacing".into(),
            },
            Token {
                name: "spacing.md".into(),
                value: TokenValue::Number(16.0),
                description: "Base spacing".into(),
            },
            Token {
                name: "spacing.lg".into(),
                value: TokenValue::Number(24.0),
                description: "Section spacing".into(),
            },
            Token {
                name: "radius.sm".into(),
                value: TokenValue::Number(4.0),
                description: "Small radius".into(),
            },
            Token {
                name: "radius.md".into(),
                value: TokenValue::Number(8.0),
                description: "Card radius".into(),
            },
            Token {
                name: "radius.lg".into(),
                value: TokenValue::Number(16.0),
                description: "Sheet / modal radius".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(16.0),
                description: "Body font size".into(),
            },
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_register_and_activate() {
        let mut reg = DesignSystemRegistry::new();
        reg.register(builtin_apple_hig()).unwrap();
        reg.activate("apple-hig").unwrap();
        assert!(reg.active().is_some());
        assert_eq!(reg.active().unwrap().name, "Apple HIG");
    }

    #[test]
    fn registry_duplicate_rejected() {
        let mut reg = DesignSystemRegistry::new();
        reg.register(builtin_apple_hig()).unwrap();
        assert!(reg.register(builtin_apple_hig()).is_err());
    }

    #[test]
    fn registry_activate_unknown_rejected() {
        let mut reg = DesignSystemRegistry::new();
        assert!(reg.activate("nope").is_err());
    }

    #[test]
    fn register_builtin_loads_all() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        assert_eq!(reg.list().len(), 4);
        reg.activate("apple-hig").unwrap();
        assert!(reg.lookup("color.bg").is_some());
    }

    #[test]
    fn desigma_preset_fully_paired_with_dark() {
        let system = builtin_desigma_aligned();
        let dark = system.dark_tokens.as_ref().unwrap();
        for token in &system.tokens {
            let pair = dark.iter().find(|t| t.name == token.name);
            assert!(
                pair.is_some(),
                "desigma-aligned token '{}' missing dark pair",
                token.name
            );
        }
    }

    #[test]
    fn robot_sim_preset_registered_and_lookup() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("robot-sim").unwrap();
        match reg.lookup("color.bg").unwrap() {
            TokenValue::Color(c) => assert_eq!(c, "#0F1115"),
            _ => panic!("expected Color"),
        }
        match reg.lookup("color.danger").unwrap() {
            TokenValue::Color(c) => assert_eq!(c, "#F85149"),
            _ => panic!("expected Color"),
        }
        match reg.lookup("font.mono").unwrap() {
            TokenValue::String(s) => assert!(s.contains("SF Mono")),
            _ => panic!("expected String"),
        }
    }

    #[test]
    fn lookup_returns_token_value() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("apple-hig").unwrap();
        match reg.lookup("color.accent").unwrap() {
            TokenValue::Color(c) => assert_eq!(c, "#007AFF"),
            _ => panic!("expected Color"),
        }
    }

    #[test]
    fn lookup_missing_returns_none() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("apple-hig").unwrap();
        assert!(reg.lookup("no.such.token").is_none());
    }

    #[test]
    fn lookup_without_active_returns_none() {
        let reg = DesignSystemRegistry::new();
        assert!(reg.lookup("color.bg").is_none());
    }

    #[test]
    fn export_import_roundtrip() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("minimal-dashboard").unwrap();
        let json = reg.export_json().unwrap();
        let mut reg2 = DesignSystemRegistry::new();
        reg2.import_json(&json).unwrap();
        assert_eq!(reg2.list().len(), 1);
    }

    #[test]
    fn import_invalid_json_rejected() {
        let mut reg = DesignSystemRegistry::new();
        assert!(reg.import_json("not json").is_err());
    }

    #[test]
    fn token_value_serde_roundtrip() {
        let v = TokenValue::Color("#FFF".into());
        let s = serde_json::to_string(&v).unwrap();
        let v2: TokenValue = serde_json::from_str(&s).unwrap();
        assert_eq!(v, v2);
    }

    // ── TokenValue::to_css_value tests ──

    #[test]
    fn token_value_color_to_css() {
        let v = TokenValue::Color("#007AFF".into());
        assert_eq!(v.to_css_value(), "#007AFF");
    }

    #[test]
    fn token_value_number_to_css() {
        let v = TokenValue::Number(17.0);
        assert_eq!(v.to_css_value(), "17px");
    }

    #[test]
    fn token_value_shadow_to_css() {
        let v = TokenValue::Shadow("0 2px 4px rgba(0,0,0,0.1)".into());
        assert_eq!(v.to_css_value(), "0 2px 4px rgba(0,0,0,0.1)");
    }

    #[test]
    fn token_value_string_to_css() {
        let v = TokenValue::String("SF Pro, sans-serif".into());
        assert_eq!(v.to_css_value(), "SF Pro, sans-serif");
    }

    #[test]
    fn token_value_reference_to_css_var() {
        let v = TokenValue::String("token:color.accent".into());
        assert_eq!(v.to_css_value(), "var(--color.accent)");
    }

    #[test]
    fn token_value_is_reference() {
        assert!(TokenValue::String("token:color.accent".into()).is_reference());
        assert!(!TokenValue::String("SF Pro".into()).is_reference());
        assert!(!TokenValue::Color("#FFF".into()).is_reference());
    }

    #[test]
    fn token_value_reference_target() {
        let v = TokenValue::String("token:color.accent".into());
        assert_eq!(v.reference_target(), Some("color.accent"));
        let v2 = TokenValue::String("no-ref".into());
        assert_eq!(v2.reference_target(), None);
    }

    // ── DesignSystem::to_css_custom_properties tests ──

    #[test]
    fn design_system_css_custom_properties() {
        let ds = builtin_apple_hig();
        let css = ds.to_css_custom_properties();
        assert!(css.starts_with(":root {"));
        assert!(css.contains("--color-bg: #F2F2F7;"));
        assert!(css.contains("--color-fg: #1C1C1E;"));
        assert!(css.contains("--color-accent: #007AFF;"));
        assert!(css.contains("--font-size-body: 17px;"));
        assert!(css.contains("--radius-card: 10px;"));
        assert!(css.ends_with("}"));
    }

    #[test]
    fn design_system_css_with_references() {
        let ds = DesignSystem {
            id: "ref-test".into(),
            name: "Ref Test".into(),
            tokens: vec![
                Token {
                    name: "color.primary".into(),
                    value: TokenValue::Color("#007AFF".into()),
                    // i18n: user-facing
                    description: "主色".into(),
                },
                Token {
                    name: "color.button".into(),
                    value: TokenValue::String("token:color.primary".into()),
                    // i18n: user-facing
                    description: "按钮色引用主色".into(),
                },
            ],
            extends: None,
            dark_tokens: None,
        };
        let css = ds.to_css_custom_properties();
        // the reference token resolves to the actual value
        assert!(css.contains("--color-button: #007AFF;"));
    }

    #[test]
    fn design_system_circular_reference_safe() {
        let ds = DesignSystem {
            id: "circular".into(),
            name: "Circular".into(),
            tokens: vec![
                Token {
                    name: "a".into(),
                    value: TokenValue::String("token:b".into()),
                    // i18n: user-facing
                    description: "循环A".into(),
                },
                Token {
                    name: "b".into(),
                    value: TokenValue::String("token:a".into()),
                    // i18n: user-facing
                    description: "循环B".into(),
                },
            ],
            extends: None,
            dark_tokens: None,
        };
        let css = ds.to_css_custom_properties();
        // circular references must not panic; falls back to the original CSS var output
        assert!(css.contains("--a:") || css.contains("--b:"));
    }

    // ── DesignSystem::resolve_reference tests ──

    #[test]
    fn resolve_direct_value() {
        let ds = builtin_apple_hig();
        let mut visited = std::collections::HashSet::new();
        let val = ds.resolve_reference(&TokenValue::Color("#FFF".into()), &mut visited);
        assert_eq!(val, "#FFF");
    }

    #[test]
    fn resolve_single_reference() {
        let ds = builtin_apple_hig();
        let mut visited = std::collections::HashSet::new();
        let val = ds.resolve_reference(
            &TokenValue::String("token:color.accent".into()),
            &mut visited,
        );
        assert_eq!(val, "#007AFF");
    }

    #[test]
    fn resolve_chained_reference() {
        let ds = DesignSystem {
            id: "chain".into(),
            name: "Chain".into(),
            tokens: vec![
                Token {
                    name: "color.base".into(),
                    value: TokenValue::Color("#007AFF".into()),
                    // i18n: user-facing
                    description: "基础色".into(),
                },
                Token {
                    name: "color.mid".into(),
                    value: TokenValue::String("token:color.base".into()),
                    // i18n: user-facing
                    description: "中间引用".into(),
                },
                Token {
                    name: "color.top".into(),
                    value: TokenValue::String("token:color.mid".into()),
                    // i18n: user-facing
                    description: "顶层引用".into(),
                },
            ],
            extends: None,
            dark_tokens: None,
        };
        let mut visited = std::collections::HashSet::new();
        let val = ds.resolve_reference(&TokenValue::String("token:color.top".into()), &mut visited);
        assert_eq!(val, "#007AFF");
    }

    #[test]
    fn resolve_missing_reference_falls_back() {
        let ds = builtin_apple_hig();
        let mut visited = std::collections::HashSet::new();
        let val = ds.resolve_reference(
            &TokenValue::String("token:nonexistent".into()),
            &mut visited,
        );
        // falls back to the original CSS var output
        assert_eq!(val, "var(--nonexistent)");
    }

    // ── P-5: import caps + reference depth cap ──

    #[test]
    fn import_json_rejects_too_many_tokens() {
        let mut reg = DesignSystemRegistry::new();
        let sys = DesignSystem {
            id: "huge".into(),
            name: "Huge".into(),
            tokens: (0..MAX_IMPORT_TOKENS + 1)
                .map(|i| Token {
                    name: format!("t{i}"),
                    value: TokenValue::Number(1.0),
                    description: "x".into(),
                })
                .collect(),
            extends: None,
            dark_tokens: None,
        };
        let json = serde_json::to_string(&sys).unwrap();
        match reg.import_json(&json) {
            Err(ImportError::TooManyTokens(n)) => assert_eq!(n, MAX_IMPORT_TOKENS + 1),
            other => panic!(
                "expected TooManyTokens, got {:?}",
                other.map_err(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn import_json_rejects_value_too_long() {
        let mut reg = DesignSystemRegistry::new();
        let sys = DesignSystem {
            id: "longstr".into(),
            name: "Long".into(),
            tokens: vec![Token {
                name: "x".into(),
                value: TokenValue::Color("0".repeat(MAX_TOKEN_STR_LEN + 1)),
                description: "x".into(),
            }],
            extends: None,
            dark_tokens: None,
        };
        let json = serde_json::to_string(&sys).unwrap();
        match reg.import_json(&json) {
            Err(ImportError::ValueTooLong(n)) => assert_eq!(n, MAX_TOKEN_STR_LEN + 1),
            other => panic!(
                "expected ValueTooLong, got {:?}",
                other.map_err(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn resolve_deep_non_cyclic_chain_hits_depth_cap() {
        // 100-level non-cyclic chain: a1→a2→…→a100→color. Depth 64 should cap and fall back to the CSS var.
        let tokens: Vec<Token> = (1..=100u32)
            .map(|i| Token {
                name: format!("a{i}"),
                value: if i == 100 {
                    TokenValue::Color("#000000".into())
                } else {
                    TokenValue::String(format!("token:a{}", i + 1))
                },
                description: "chain".into(),
            })
            .collect();
        let ds = DesignSystem {
            id: "deep".into(),
            name: "Deep".into(),
            tokens,
            extends: None,
            dark_tokens: None,
        };
        let mut visited = std::collections::HashSet::new();
        let val = ds.resolve_reference(&TokenValue::String("token:a1".into()), &mut visited);
        // Depth cap: the end value #000000 was not reached; falls back to some var(--aN) level.
        assert!(
            val.starts_with("var(--a"),
            "depth cap should fall back to var(--aN), got {val}"
        );
    }

    #[test]
    fn register_builtin_idempotent_no_overwrite() {
        // E-32/P3: calling register_builtin repeatedly must not overwrite already-registered or
        // user-customized systems.
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        assert_eq!(reg.list().len(), 4);
        // Second call: count unchanged, no error.
        reg.register_builtin();
        assert_eq!(
            reg.list().len(),
            4,
            "repeated registration must not double or overwrite"
        );
    }

    #[test]
    fn register_builtin_preserves_user_customized_system() {
        // E-32/P3: after a user customizes a same-name system, register_builtin must not overwrite
        // the user's version with the builtin.
        let mut reg = DesignSystemRegistry::new();
        // The user first registers a custom apple-hig (kept as a user-visible system name).
        let mut custom = builtin_apple_hig();
        // i18n: user-facing
        custom.name = "我的自定义 HIG".into();
        reg.register(custom).unwrap();
        // Then register builtins: apple-hig should be skipped and the rest filled in.
        reg.register_builtin();
        assert_eq!(
            reg.list().len(),
            4,
            "builtins fill in the other three; apple-hig is skipped"
        );
        // Verify apple-hig is still the user-customized version.
        reg.activate("apple-hig").unwrap();
        assert_eq!(
            reg.active().unwrap().name,
            // i18n: user-facing (matches the user-customized system name above)
            "我的自定义 HIG",
            "user-customized system must not be overwritten by builtins"
        );
    }

    // E-15 regression: when dark_tokens only overrides some tokens, the old implementation lost
    // the non-overridden light tokens (font.size.body/radius.card) because dark_tokens replaced
    // the set wholesale (missing tokens → missing CSS variables).
    // After the fix: light as base + dark overlay, all tokens kept, dark values override light values.
    #[test]
    fn dark_theme_css_merges_light_base_with_dark_overlay() {
        let system = DesignSystem {
            id: "test-dark-merge".into(),
            name: "Test Dark Merge".into(),
            tokens: vec![
                Token {
                    name: "color.bg".into(),
                    value: TokenValue::Color("#FFFFFF".into()),
                    description: "".into(),
                },
                Token {
                    name: "font.size.body".into(),
                    value: TokenValue::Number(16.0),
                    description: "".into(),
                },
            ],
            extends: None,
            dark_tokens: Some(vec![Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#000000".into()),
                description: "".into(),
            }]),
        };
        let css = system.to_css_custom_properties_for_theme(Theme::Dark);
        // dark override: color-bg uses the dark value
        assert!(
            css.contains("--color-bg: #000000;"),
            "dark theme color.bg should be overridden by the dark value"
        );
        // non-overridden light tokens must be kept (lost before the E-15 fix)
        assert!(
            css.contains("--font-size-body: 16px;"),
            "dark theme non-overridden font.size.body should be kept from the light base (E-15)"
        );
    }

    // ── A-8: DesignSystem inheritance-chain resolve ──

    fn make_system(id: &str, extends: Option<&str>, tokens: Vec<Token>) -> DesignSystem {
        DesignSystem {
            id: id.into(),
            name: id.into(),
            tokens,
            dark_tokens: None,
            extends: extends.map(|s| s.to_string()),
        }
    }

    #[test]
    fn resolve_no_extends_returns_self() {
        let mut reg = DesignSystemRegistry::new();
        reg.register(make_system(
            "base",
            None,
            vec![Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#FFF".into()),
                description: "".into(),
            }],
        ))
        .unwrap();
        let resolved = reg.resolve("base").unwrap();
        assert_eq!(resolved.extends, None);
        assert_eq!(resolved.tokens.len(), 1);
        assert_eq!(resolved.tokens[0].name, "color.bg");
    }

    #[test]
    fn resolve_extends_overlays_child_tokens() {
        let mut reg = DesignSystemRegistry::new();
        // Parent: apple-hig base tokens.
        reg.register(builtin_apple_hig()).unwrap();
        // Child: overrides only color.bg, inheriting the rest (color.fg/accent/font.size.body/radius.card).
        reg.register(make_system(
            "custom",
            Some("apple-hig"),
            vec![Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#FF0000".into()),
                // i18n: user-facing
                description: "自定义红底".into(),
            }],
        ))
        .unwrap();
        let resolved = reg.resolve("custom").unwrap();
        // Parent's 5 tokens + child overrides color.bg (same-name override, no growth) = 5.
        assert_eq!(
            resolved.tokens.len(),
            5,
            "inherits all parent tokens; child override does not add"
        );
        // color.bg is overridden to red by the child.
        let bg = resolved
            .tokens
            .iter()
            .find(|t| t.name == "color.bg")
            .expect("color.bg should exist");
        assert_eq!(bg.value, TokenValue::Color("#FF0000".into()));
        // Parent's color.fg is kept (not overridden).
        assert!(
            resolved.tokens.iter().any(|t| t.name == "color.fg"),
            "parent color.fg should be inherited"
        );
        // Result's extends is cleared (self-contained, no parent dependency).
        assert_eq!(resolved.extends, None);
    }

    #[test]
    fn resolve_extends_chain_multi_level() {
        let mut reg = DesignSystemRegistry::new();
        reg.register(make_system(
            "grand",
            None,
            vec![
                Token {
                    name: "a".into(),
                    value: TokenValue::Number(1.0),
                    description: "".into(),
                },
                Token {
                    name: "b".into(),
                    value: TokenValue::Number(2.0),
                    description: "".into(),
                },
            ],
        ))
        .unwrap();
        reg.register(make_system(
            "mid",
            Some("grand"),
            vec![Token {
                name: "b".into(),
                value: TokenValue::Number(20.0),
                description: "".into(),
            }],
        ))
        .unwrap();
        reg.register(make_system(
            "leaf",
            Some("mid"),
            vec![Token {
                name: "c".into(),
                value: TokenValue::Number(3.0),
                description: "".into(),
            }],
        ))
        .unwrap();
        let resolved = reg.resolve("leaf").unwrap();
        // a(parent) + b(child override 20) + c(child-only) = 3.
        assert_eq!(resolved.tokens.len(), 3);
        let b = resolved
            .tokens
            .iter()
            .find(|t| t.name == "b")
            .expect("b should exist");
        assert_eq!(
            b.value,
            TokenValue::Number(20.0),
            "mid's override b=20 should win"
        );
        let a = resolved
            .tokens
            .iter()
            .find(|t| t.name == "a")
            .expect("a should exist");
        assert_eq!(
            a.value,
            TokenValue::Number(1.0),
            "grand's a=1 should be inherited"
        );
    }

    #[test]
    fn resolve_circular_chain_errors() {
        let mut reg = DesignSystemRegistry::new();
        reg.register(make_system(
            "x",
            Some("y"),
            vec![Token {
                name: "a".into(),
                value: TokenValue::Number(1.0),
                description: "".into(),
            }],
        ))
        .unwrap();
        reg.register(make_system(
            "y",
            Some("x"),
            vec![Token {
                name: "b".into(),
                value: TokenValue::Number(2.0),
                description: "".into(),
            }],
        ))
        .unwrap();
        match reg.resolve("x") {
            Err(ResolveError::Circular { id, .. }) => assert_eq!(id, "x"),
            other => panic!(
                "expected Circular, got {:?}",
                other.map_err(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn resolve_missing_parent_errors() {
        let mut reg = DesignSystemRegistry::new();
        reg.register(make_system(
            "orphan",
            Some("ghost"),
            vec![Token {
                name: "a".into(),
                value: TokenValue::Number(1.0),
                description: "".into(),
            }],
        ))
        .unwrap();
        match reg.resolve("orphan") {
            Err(ResolveError::NotFound(name)) => assert_eq!(name, "ghost"),
            other => panic!(
                "expected NotFound ghost, got {:?}",
                other.map_err(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn resolve_extends_inherits_parent_dark_tokens() {
        let mut reg = DesignSystemRegistry::new();
        // Parent has dark_tokens, child has none → inherits the parent's dark tokens.
        let mut parent = builtin_apple_hig();
        parent.id = "parent-dark".into();
        reg.register(parent).unwrap();
        reg.register(make_system(
            "child-no-dark",
            Some("parent-dark"),
            vec![Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#FFFFFF".into()),
                description: "".into(),
            }],
        ))
        .unwrap();
        let resolved = reg.resolve("child-no-dark").unwrap();
        assert!(
            resolved.dark_tokens.is_some(),
            "parent dark_tokens should be inherited by the child"
        );
        let dark = resolved.dark_tokens.as_ref().unwrap();
        assert!(
            dark.iter().any(|t| t.name == "color.bg"),
            "parent dark color.bg should exist"
        );
    }

    #[test]
    fn resolve_to_css_for_theme_with_inherited_tokens() {
        let mut reg = DesignSystemRegistry::new();
        reg.register(builtin_apple_hig()).unwrap();
        reg.register(make_system(
            "branded",
            Some("apple-hig"),
            vec![Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#FF0000".into()),
                // i18n: user-facing
                description: "品牌红底".into(),
            }],
        ))
        .unwrap();
        let resolved = reg.resolve("branded").unwrap();
        let css = resolved.to_css_custom_properties();
        // after override, color-bg is red
        assert!(
            css.contains("--color-bg: #FF0000;"),
            "after inheritance resolve, color-bg should be the child's override value, css={css}"
        );
        // parent's color-accent is kept
        assert!(
            css.contains("--color-accent: #007AFF;"),
            "parent color-accent should be inherited, css={css}"
        );
    }
}
