//! Fusion-Design 设计系统 — 三套内置规范 + Token 管理。
//!
//! 对应 PRD 模块 3「本地设计系统与组件库」。
//! 全局 Token（颜色/字号/间距/圆角/阴影）统一定义，一键同步所有页面。

use std::collections::HashMap;

use fd_canvas_core::sanitize_css_value;
use serde::{Deserialize, Serialize};

/// P-5：token 引用链深度上限。防超长非环链栈溢出（visited 仅防环，不防深度）。
const MAX_REFERENCE_DEPTH: u32 = 64;
/// P-5：import_json 合理上限。设计规范 token 数远低于此，超限视为恶意/错误输入。
const MAX_IMPORT_TOKENS: usize = 10000;
/// P-5：单 token 字符串值长度上限（字节）。防巨串占内存。
const MAX_TOKEN_STR_LEN: usize = 4096;

/// 设计 Token 值类型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TokenValue {
    Color(String),  // hex, e.g. "#FFFFFF"
    Number(f32),    // 字号/间距/圆角
    Shadow(String), // CSS box-shadow
    String(String), // 字体族等 / token:xxx 引用
}

impl TokenValue {
    /// 转换为 CSS 属性值字符串。
    /// - Color → 直接输出 hex
    /// - Number → 输出 `Npx`（字号/间距/圆角默认 px）
    /// - Shadow → 直接输出 CSS box-shadow
    /// - String → 直接输出（字体族等）；若为 `token:xxx` 引用则输出 `var(--xxx)`
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

    /// 检查是否为 token 引用（`token:xxx` 格式）。
    pub fn is_reference(&self) -> bool {
        match self {
            TokenValue::String(s) => s.starts_with("token:"),
            _ => false,
        }
    }

    /// 提取引用目标名（`token:xxx` → `xxx`）。
    pub fn reference_target(&self) -> Option<&str> {
        match self {
            TokenValue::String(s) => s.strip_prefix("token:"),
            _ => None,
        }
        .filter(|s| !s.is_empty())
    }
}

/// 单个 Token 定义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub name: String,
    pub value: TokenValue,
    pub description: String,
}

/// 设计规范（一组 Token 集合）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignSystem {
    pub id: String,
    pub name: String,
    pub tokens: Vec<Token>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark_tokens: Option<Vec<Token>>,
    // A-8：继承父规范名（如 "apple-hig"）。自定义规范可仅覆写个别 token，
    // 其余从父规范继承。None 表示无父（内置规范/已 resolve 的全量规范）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
}

/// 主题模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

impl DesignSystem {
    /// 生成 CSS Custom Properties 输出：`:root { --token-name: value; ... }`
    /// Token name 中的 `.` 替换为 `-` 以符合 CSS 自定义属性命名规范。
    /// 引用类型 token（`token:xxx`）会递归解析为最终值。
    /// P-5：预构 token map 一次，resolve 走 O(1) 查，避免循环内 O(n) find = O(n²)。
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

    /// 解析 token 引用：若值为 `token:xxx` 则查找目标 token 的值并递归解析。
    /// 防止循环引用（visited 集合）+ 深度封顶 64（防超长非环链栈溢出）。
    pub fn resolve_reference(
        &self,
        value: &TokenValue,
        visited: &mut std::collections::HashSet<String>,
    ) -> String {
        let map = self.token_map();
        self.resolve_reference_inner(value, visited, &map, 0)
    }

    /// 引用解析内部实现：token map O(1) 查 + 显式深度计数。
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
                    "token 引用链超深度上限，回退原始 CSS var"
                );
                return value.to_css_value();
            }
            if visited.contains(target) {
                tracing::warn!("检测到循环 token 引用: {:?}, 已访问: {:?}", target, visited);
                return value.to_css_value();
            }
            visited.insert(target.to_string());
            if let Some(resolved) = map.get(target).copied() {
                self.resolve_reference_inner(resolved, visited, map, depth + 1)
            } else {
                tracing::warn!("Token 引用目标未找到: {}", target);
                value.to_css_value()
            }
        } else {
            value.to_css_value()
        }
    }

    /// P-5：预构 token name→value map（O(1) 查）。借用 self.tokens，生命周期绑 self。
    fn token_map(&self) -> std::collections::HashMap<&str, &TokenValue> {
        self.tokens
            .iter()
            .map(|t| (t.name.as_str(), &t.value))
            .collect()
    }

    /// 按主题模式生成 CSS Custom Properties。
    /// Light 使用 self.tokens，Dark 使用 self.dark_tokens（如无则回退到 tokens）。
    pub fn to_css_custom_properties_for_theme(&self, theme: Theme) -> String {
        // E-15：Dark 主题旧实现直接用 dark_tokens（若仅覆写颜色未列 font_size/radius，
        // 这些 token 丢失 → CSS 变量缺失）。改为以 light tokens 为基底，dark 覆盖 overlay。
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
                // 保持 light tokens 的声明顺序，dark 新增的 token 追加到末尾。
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
        // P-5：map 从 merged 切片构建，复用 inner resolver。
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

    // A-8：用父规范的 token 为基底，self 的 token overlay 覆盖（self 同名 token 胜出）。
    // 仅用于继承链 resolve，不改变 self 的 extends 字段。
    // light tokens：父在前 + self 覆盖，保留父声明顺序，self 新增 token 追加末尾。
    // dark_tokens：各自 dark overlay 后再覆盖（None 视为空 dark 集）。
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
            // 合并产物已无父依赖，extends 清空（防 resolve 后再被当作可继承规范）。
            extends: None,
        }
    }
}

/// A-8：base 在前，overlay 同名覆盖，overlay 新增 token 追加末尾。保留 base 声明顺序。
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

/// 设计系统注册中心（管理多套规范，支持一键切换）。
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

    /// 注册一套设计规范。
    pub fn register(&mut self, system: DesignSystem) -> Result<(), DuplicateError> {
        if self.systems.contains_key(&system.id) {
            return Err(DuplicateError(system.id));
        }
        self.systems.insert(system.id.clone(), system);
        Ok(())
    }

    /// 激活某套规范（切换全局 Token）。
    pub fn activate(&mut self, id: &str) -> Result<(), NotFoundError> {
        if !self.systems.contains_key(id) {
            return Err(NotFoundError(id.to_string()));
        }
        self.active_id = Some(id.to_string());
        Ok(())
    }

    /// 切换主题模式（Light/Dark）。
    pub fn activate_theme(&mut self, theme: Theme) {
        self.active_theme = theme;
        tracing::info!(theme = ?theme, "主题已切换");
    }

    /// 获取当前主题模式。
    pub fn current_theme(&self) -> Theme {
        self.active_theme
    }

    /// 获取当前激活的规范。
    pub fn active(&self) -> Option<&DesignSystem> {
        self.active_id.as_ref().and_then(|id| self.systems.get(id))
    }

    /// 列出全部已注册规范 ID。
    pub fn list(&self) -> Vec<&str> {
        self.systems.keys().map(|s| s.as_str()).collect()
    }

    /// 按 ID 获取规范。
    pub fn get(&self, id: &str) -> Option<&DesignSystem> {
        self.systems.get(id)
    }

    // A-8：递归 resolve 继承链，返回全量 DesignSystem（parent token 为基底，子逐层 overlay）。
    // 环检测：visited 集合记录已访问 id，重复访问 → Err(ResolveError::Circular)。
    // 父规范未注册 → Err(ResolveError::ParentNotFound)。
    // 无 extends → 返回自身 clone（去掉 extends 依赖，产物 extends=None）。
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
            tracing::warn!(system_id = %id, chain = ?visited, "检测到继承环");
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
                // 叶子规范：clone 并清空 extends（产物自洽，无父依赖）。
                let mut out = system.clone();
                out.extends = None;
                Ok(out)
            }
        }
    }

    /// 注册三套内置规范（Apple HIG / 极简后台 / 机器人仿真控制台）。
    /// 幂等：已存在的 ID 跳过并告警，不覆盖用户自定义/激活中的规范（E-32/P3）。
    pub fn register_builtin(&mut self) {
        let builtins = [
            ("apple-hig".to_string(), builtin_apple_hig()),
            ("minimal-dashboard".to_string(), builtin_minimal_dashboard()),
            ("robot-sim".to_string(), builtin_robot_sim()),
        ];
        let mut skipped = 0usize;
        for (id, system) in builtins {
            if self.systems.contains_key(&id) {
                tracing::warn!(system_id = %id, "register_builtin: 规范已存在，跳过不覆盖");
                skipped += 1;
                continue;
            }
            self.systems.insert(id, system);
        }
        if skipped > 0 {
            tracing::warn!(
                skipped,
                total = 3,
                "register_builtin: 跳过 {skipped}/3 个已存在内置规范"
            );
        } else {
            tracing::info!("register_builtin: 注册 3 套内置规范");
        }
    }

    /// 按名称查找 Token 值（当前激活规范内）。
    pub fn lookup(&self, token_name: &str) -> Option<&TokenValue> {
        self.active()?
            .tokens
            .iter()
            .find(|t| t.name == token_name)
            .map(|t| &t.value)
    }

    /// 导出为 JSON（团队本地共享设计规范）。
    pub fn export_json(&self) -> serde_json::Result<String> {
        let active = self.active();
        serde_json::to_string_pretty(&active)
    }

    /// 从 JSON 导入一套规范。
    /// P-5：token 数 / 单值长度 / 引用深度受限，防恶意/错误巨输入。
    pub fn import_json(&mut self, json: &str) -> Result<(), ImportError> {
        let system: DesignSystem = serde_json::from_str(json).map_err(ImportError::Parse)?;
        if system.tokens.len() > MAX_IMPORT_TOKENS {
            tracing::warn!(
                count = system.tokens.len(),
                cap = MAX_IMPORT_TOKENS,
                "import_json: token 数超限，拒绝导入"
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
                    "import_json: dark token 数超限，拒绝导入"
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

/// P-5：校验单 token 值的字符串长度（Color/Shadow/String 变体）。
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
                "token 字符串值超长，拒绝导入"
            );
            return Err(ImportError::ValueTooLong(s.len()));
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("设计规范 {0} 已注册")]
pub struct DuplicateError(pub String);

#[derive(Debug, thiserror::Error)]
#[error("设计规范 {0} 未找到")]
pub struct NotFoundError(pub String);

// A-8：继承链 resolve 错误。
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("设计规范 {0} 未找到")]
    NotFound(String),
    #[error("继承环检测：规范 {id} 重复出现，链路 {chain:?}")]
    Circular { id: String, chain: Vec<String> },
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("导入 JSON 解析失败: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("token 数 {0} 超过上限 {MAX_IMPORT_TOKENS}")]
    TooManyTokens(usize),
    #[error("token 字符串值长度 {0} 超过上限 {MAX_TOKEN_STR_LEN}")]
    ValueTooLong(usize),
}

// ── 三套内置规范 ──

pub fn builtin_apple_hig() -> DesignSystem {
    DesignSystem {
        id: "apple-hig".into(),
        name: "Apple HIG".into(),
        tokens: vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#F2F2F7".into()),
                description: "系统背景".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#1C1C1E".into()),
                description: "主前景".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#007AFF".into()),
                description: "系统强调色".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(17.0),
                description: "正文字号".into(),
            },
            Token {
                name: "radius.card".into(),
                value: TokenValue::Number(10.0),
                description: "卡片圆角".into(),
            },
        ],
        extends: None,
        dark_tokens: Some(vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#1C1C1E".into()),
                description: "暗色背景".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#F2F2F7".into()),
                description: "暗色前景".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#0A84FF".into()),
                description: "暗色强调".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(17.0),
                description: "正文字号".into(),
            },
            Token {
                name: "radius.card".into(),
                value: TokenValue::Number(10.0),
                description: "卡片圆角".into(),
            },
        ]),
    }
}

fn builtin_minimal_dashboard() -> DesignSystem {
    DesignSystem {
        id: "minimal-dashboard".into(),
        name: "极简后台".into(),
        tokens: vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#FFFFFF".into()),
                description: "纯白背景".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#374151".into()),
                description: "深灰前景".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#10B981".into()),
                description: "绿色强调".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(14.0),
                description: "正文字号".into(),
            },
            Token {
                name: "radius.card".into(),
                value: TokenValue::Number(6.0),
                description: "卡片圆角".into(),
            },
        ],
        extends: None,
        dark_tokens: None,
    }
}

/// 机器人仿真控制台预设（PRD 模块 3 第三套内置规范）。
/// 面向 Fusion-Simulation 联动：深色工程界面、状态/告警/遥测语义色、等宽数据字体。
pub fn builtin_robot_sim() -> DesignSystem {
    DesignSystem {
        id: "robot-sim".into(),
        name: "机器人仿真控制台".into(),
        tokens: vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#0F1115".into()),
                description: "控制台深色背景".into(),
            },
            Token {
                name: "color.surface".into(),
                value: TokenValue::Color("#1A1F26".into()),
                description: "面板/卡片表面".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#E6EDF3".into()),
                description: "主前景（高对比浅色）".into(),
            },
            Token {
                name: "color.muted".into(),
                value: TokenValue::Color("#7D8590".into()),
                description: "次要文本/标注".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#2F81F7".into()),
                description: "主交互强调（蓝）".into(),
            },
            Token {
                name: "color.success".into(),
                value: TokenValue::Color("#3FB950".into()),
                description: "运行正常/在线".into(),
            },
            Token {
                name: "color.warning".into(),
                value: TokenValue::Color("#D29922".into()),
                description: "告警/阈值临近".into(),
            },
            Token {
                name: "color.danger".into(),
                value: TokenValue::Color("#F85149".into()),
                description: "故障/急停/离线".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(13.0),
                description: "正文字号（紧凑工程界面）".into(),
            },
            Token {
                name: "font.size.mono".into(),
                value: TokenValue::Number(12.0),
                description: "遥测/日志等宽数据字号".into(),
            },
            Token {
                name: "font.mono".into(),
                value: TokenValue::String("SF Mono, Menlo, monospace".into()),
                description: "等宽字体族（遥测/坐标/日志）".into(),
            },
            Token {
                name: "radius.panel".into(),
                value: TokenValue::Number(4.0),
                description: "面板圆角（锐利工程风）".into(),
            },
            Token {
                name: "spacing.gauge".into(),
                value: TokenValue::Number(8.0),
                description: "仪表/控件间距基准".into(),
            },
            Token {
                name: "shadow.panel".into(),
                value: TokenValue::Shadow("0 1px 0 rgba(0,0,0,0.4)".into()),
                description: "面板内阴影（深色浮起）".into(),
            },
        ],
        extends: None,
        dark_tokens: Some(vec![
            Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#010409".into()),
                description: "夜间极深背景".into(),
            },
            Token {
                name: "color.surface".into(),
                value: TokenValue::Color("#0D1117".into()),
                description: "夜间面板表面".into(),
            },
            Token {
                name: "color.fg".into(),
                value: TokenValue::Color("#F0F6FC".into()),
                description: "夜间高对比前景".into(),
            },
            Token {
                name: "color.accent".into(),
                value: TokenValue::Color("#1F6FEB".into()),
                description: "夜间强调".into(),
            },
            Token {
                name: "color.success".into(),
                value: TokenValue::Color("#238636".into()),
                description: "夜间运行正常".into(),
            },
            Token {
                name: "color.danger".into(),
                value: TokenValue::Color("#DA3633".into()),
                description: "夜间故障".into(),
            },
            Token {
                name: "font.size.body".into(),
                value: TokenValue::Number(13.0),
                description: "正文字号".into(),
            },
            Token {
                name: "radius.panel".into(),
                value: TokenValue::Number(4.0),
                description: "面板圆角".into(),
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
    fn register_builtin_loads_three() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        assert_eq!(reg.list().len(), 3);
        reg.activate("apple-hig").unwrap();
        assert!(reg.lookup("color.bg").is_some());
    }

    #[test]
    fn robot_sim_preset_registered_and_lookup() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("robot-sim").unwrap();
        match reg.lookup("color.bg").unwrap() {
            TokenValue::Color(c) => assert_eq!(c, "#0F1115"),
            _ => panic!("期望 Color"),
        }
        match reg.lookup("color.danger").unwrap() {
            TokenValue::Color(c) => assert_eq!(c, "#F85149"),
            _ => panic!("期望 Color"),
        }
        match reg.lookup("font.mono").unwrap() {
            TokenValue::String(s) => assert!(s.contains("SF Mono")),
            _ => panic!("期望 String"),
        }
    }

    #[test]
    fn lookup_returns_token_value() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("apple-hig").unwrap();
        match reg.lookup("color.accent").unwrap() {
            TokenValue::Color(c) => assert_eq!(c, "#007AFF"),
            _ => panic!("期望 Color"),
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

    // ── TokenValue::to_css_value 测试 ──

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

    // ── DesignSystem::to_css_custom_properties 测试 ──

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
                    description: "主色".into(),
                },
                Token {
                    name: "color.button".into(),
                    value: TokenValue::String("token:color.primary".into()),
                    description: "按钮色引用主色".into(),
                },
            ],
            extends: None,
            dark_tokens: None,
        };
        let css = ds.to_css_custom_properties();
        // 引用 token 解析为实际值
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
                    description: "循环A".into(),
                },
                Token {
                    name: "b".into(),
                    value: TokenValue::String("token:a".into()),
                    description: "循环B".into(),
                },
            ],
            extends: None,
            dark_tokens: None,
        };
        let css = ds.to_css_custom_properties();
        // 循环引用不会 panic，回退到原始 CSS var 输出
        assert!(css.contains("--a:") || css.contains("--b:"));
    }

    // ── DesignSystem::resolve_reference 测试 ──

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
                    description: "基础色".into(),
                },
                Token {
                    name: "color.mid".into(),
                    value: TokenValue::String("token:color.base".into()),
                    description: "中间引用".into(),
                },
                Token {
                    name: "color.top".into(),
                    value: TokenValue::String("token:color.mid".into()),
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
        // 回退到原始 CSS var 输出
        assert_eq!(val, "var(--nonexistent)");
    }

    // ── P-5：import 限额 + 引用深度封顶 ──

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
                "期望 TooManyTokens，得 {:?}",
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
                "期望 ValueTooLong，得 {:?}",
                other.map_err(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn resolve_deep_non_cyclic_chain_hits_depth_cap() {
        // 100 层非环链：a1→a2→…→a100→color。深度 64 应封顶回退 CSS var。
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
        // 深度封顶：未解析到末端 #000000，回退到某层 var(--aN)。
        assert!(
            val.starts_with("var(--a"),
            "深度封顶应回退 var(--aN)，得 {val}"
        );
    }

    #[test]
    fn register_builtin_idempotent_no_overwrite() {
        // E-32/P3：重复调用 register_builtin 不应覆盖已注册/用户自定义的规范。
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        assert_eq!(reg.list().len(), 3);
        // 二次调用：数量不变，不抛错。
        reg.register_builtin();
        assert_eq!(reg.list().len(), 3, "重复注册不应翻倍或覆盖");
    }

    #[test]
    fn register_builtin_preserves_user_customized_system() {
        // E-32/P3：用户自定义同名规范后调用 register_builtin，用户版本不被内置覆盖。
        let mut reg = DesignSystemRegistry::new();
        // 用户先注册一个自定义 apple-hig（active 设为自定义值便于验证）。
        let mut custom = builtin_apple_hig();
        custom.name = "我的自定义 HIG".into();
        reg.register(custom).unwrap();
        // 再注册内置：应跳过 apple-hig，补齐另外两套。
        reg.register_builtin();
        assert_eq!(reg.list().len(), 3, "内置补齐另两套，apple-hig 跳过");
        // 验证 apple-hig 仍是用户自定义版本。
        reg.activate("apple-hig").unwrap();
        assert_eq!(
            reg.active().unwrap().name,
            "我的自定义 HIG",
            "用户自定义规范未被内置覆盖"
        );
    }

    // E-15 回归：dark_tokens 仅覆写部分 token 时，未覆写的 light token（font.size.body/radius.card）
    // 旧实现丢失（dark_tokens 直接替代，缺失 token → CSS 变量缺）。
    // 修复后以 light 为基底 + dark overlay，全量 token 保留，dark 值覆盖 light 值。
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
        // dark 覆盖：color-bg 用 dark 值
        assert!(
            css.contains("--color-bg: #000000;"),
            "dark 主题 color.bg 应被 dark 值覆盖"
        );
        // 未覆写的 light token 应保留（E-15 修复前丢失）
        assert!(
            css.contains("--font-size-body: 16px;"),
            "dark 主题未覆写的 font.size.body 应从 light 基底保留（E-15）"
        );
    }

    // ── A-8：DesignSystem 继承链 resolve ──

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
        // 父：apple-hig 基底 token。
        reg.register(builtin_apple_hig()).unwrap();
        // 子：仅覆写 color.bg，继承其余（color.fg/accent/font.size.body/radius.card）。
        reg.register(make_system(
            "custom",
            Some("apple-hig"),
            vec![Token {
                name: "color.bg".into(),
                value: TokenValue::Color("#FF0000".into()),
                description: "自定义红底".into(),
            }],
        ))
        .unwrap();
        let resolved = reg.resolve("custom").unwrap();
        // 父的 5 个 token + 子覆写 color.bg（同名覆盖不增）= 5 个。
        assert_eq!(resolved.tokens.len(), 5, "继承父全部 token，子覆写不增数");
        // color.bg 被子覆写为红。
        let bg = resolved
            .tokens
            .iter()
            .find(|t| t.name == "color.bg")
            .expect("color.bg 应存在");
        assert_eq!(bg.value, TokenValue::Color("#FF0000".into()));
        // 父的 color.fg 保留（未覆写）。
        assert!(
            resolved.tokens.iter().any(|t| t.name == "color.fg"),
            "父 color.fg 应继承保留"
        );
        // 产物 extends 清空（自洽，无父依赖）。
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
        // a(父) + b(子覆写 20) + c(子新增) = 3。
        assert_eq!(resolved.tokens.len(), 3);
        let b = resolved
            .tokens
            .iter()
            .find(|t| t.name == "b")
            .expect("b 应存在");
        assert_eq!(b.value, TokenValue::Number(20.0), "mid 覆写 b=20 应胜出");
        let a = resolved
            .tokens
            .iter()
            .find(|t| t.name == "a")
            .expect("a 应存在");
        assert_eq!(a.value, TokenValue::Number(1.0), "grand 的 a=1 应继承");
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
            other => panic!("期望 Circular，得 {:?}", other.map_err(|e| e.to_string())),
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
                "期望 NotFound ghost，得 {:?}",
                other.map_err(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn resolve_extends_inherits_parent_dark_tokens() {
        let mut reg = DesignSystemRegistry::new();
        // 父有 dark_tokens，子无 dark_tokens → 继承父 dark。
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
        assert!(resolved.dark_tokens.is_some(), "父 dark_tokens 应被子继承");
        let dark = resolved.dark_tokens.as_ref().unwrap();
        assert!(
            dark.iter().any(|t| t.name == "color.bg"),
            "父 dark color.bg 应存在"
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
                description: "品牌红底".into(),
            }],
        ))
        .unwrap();
        let resolved = reg.resolve("branded").unwrap();
        let css = resolved.to_css_custom_properties();
        // 覆写后的 color-bg 为红。
        assert!(
            css.contains("--color-bg: #FF0000;"),
            "继承 resolve 后 color-bg 应为子覆写值，css={css}"
        );
        // 父的 color-accent 保留。
        assert!(
            css.contains("--color-accent: #007AFF;"),
            "父 color-accent 应继承保留，css={css}"
        );
    }
}
