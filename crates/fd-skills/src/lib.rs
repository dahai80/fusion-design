//! fd-skills：AI 设计 Skill 系统。trait 化能力 + SkillContext trait 解耦 inference client。
//!
//! ARCH-11：skill 系统（7 skill + SkillRegistry + SkillOutput 类型）+ 纯 helper
//!（parse_ui_json/repair_model_json/strip_code_fence/encode_image_base64/
//! ui_generator_system_prompt/DEFAULT_MAX_TOKENS）从 fd-ai-adapter 迁出。
//! SkillContext 改对象安全 trait，fd-ai-adapter impl SkillContext for FusionMlxClient
//! 反向单向依赖（fd-skills ← adapter）。fd-skills 零 HTTP，纯 trait + helper + 解析逻辑。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use fd_canvas_core::{NodeKind, Page, PenDocument, PenNode};
use fd_design_system::DesignSystem;

/// 默认 max_tokens（生成上限）。A5：旧实现散落 9 处硬编码 4096，提为常量统一引用。
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

/// SkillContext trait：抽象 chat 能力，解耦 skill 与具体 inference client。
///
/// 对象安全（无泛型/Self），SkillRegistry 用 `&dyn SkillContext` 避免泛型蔓延。
/// fd-ai-adapter `impl SkillContext for FusionMlxClient` 反向实现本 trait。
pub trait SkillContext: Send + Sync {
    /// 同步 chat 请求。
    fn chat(&self, model: &str, sys: &str, user: &str, max_tokens: u32) -> anyhow::Result<String>;
    /// 异步 chat 请求（返回 boxed future，借用 self + 入参同生命周期）。
    fn chat_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>;
    /// 多模态异步 chat（图片 + 文字）。
    fn chat_with_image_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        image_base64: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>;
    /// 多模态同步 chat（图片 + 文字）。
    fn chat_with_image(
        &self,
        model: &str,
        sys: &str,
        user: &str,
        image_base64: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String>;
    /// 生成设计 Token 的 CSS Custom Properties 片段，注入 system prompt。
    /// `design_system` 作参数传入（原 SkillContext struct 读字段，trait 化后改参数）。
    /// 返回 None 如果没有激活设计系统。
    fn token_prompt_fragment(&self, design_system: Option<&DesignSystem>) -> Option<String>;
}

/// Skill 输出类型。
#[derive(Debug, Clone)]
pub enum SkillOutput {
    /// 完整页面文档
    Document(PenDocument),
    /// 局部编辑的节点 JSON
    PartialEdit(String),
    /// 多方案对比（3 份文档）
    MultiVariants([PenDocument; 3]),
    /// 设计规范文档（交互规范/组件规范/页面架构）
    SpecDoc(SpecDocument),
    /// 页面流程批量生成（多页连贯流程）
    PageFlow(Vec<PenDocument>),
}

/// 设计规范文档。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecDocument {
    /// 文档标题。
    pub title: String,
    /// 页面架构描述。
    pub page_architecture: String,
    /// 交互规范条目。
    pub interaction_specs: Vec<InteractionSpec>,
    /// 组件规范条目。
    pub component_specs: Vec<ComponentSpec>,
    /// 设计 Token 引用摘要。
    pub token_summary: String,
}

/// 交互规范条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionSpec {
    pub id: String,
    pub element: String,
    pub event: String,
    pub behavior: String,
    #[serde(default)]
    pub animation: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// 组件规范条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSpec {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub props: Vec<ComponentProp>,
    #[serde(default)]
    pub variants: Vec<String>,
    #[serde(default)]
    pub accessibility: Option<String>,
}

/// 组件属性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentProp {
    pub name: String,
    pub prop_type: String,
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// AI 设计 Skill trait — 每个 Skill 实现一个设计能力。
///
/// `execute`/`execute_async` 接收 `&dyn SkillContext`（对象安全 trait），
/// skill 自持 `model: String` 字段（Ruling #16），`design_system` 作参数传入
///（Ruling #15：token_prompt_fragment 不再读 struct 字段）。
pub trait DesignSkill: Send + Sync {
    /// Skill 唯一标识。
    fn id(&self) -> &str;
    /// Skill 显示名称。
    fn label(&self) -> &str;
    /// 同步执行。
    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput>;
    /// 异步执行（默认实现：委托给同步版本，input move 进 async 闭包后借）。
    fn execute_async<'a>(
        &'a self,
        ctx: &'a dyn SkillContext,
        design_system: Option<&'a DesignSystem>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move { self.execute(ctx, design_system, &input) })
    }
}

/// Skill 注册中心：按 id 查找并调度 Skill。
pub struct SkillRegistry {
    skills: HashMap<String, Box<dyn DesignSkill>>,
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
        }
    }

    /// 注册一个 Skill。
    pub fn register(&mut self, skill: Box<dyn DesignSkill>) {
        self.skills.insert(skill.id().to_string(), skill);
    }

    /// 按 id 查找 Skill。
    pub fn get(&self, id: &str) -> Option<&dyn DesignSkill> {
        self.skills.get(id).map(|s| s.as_ref())
    }

    /// 列出所有已注册 Skill 的 id。
    pub fn list(&self) -> Vec<&str> {
        self.skills.keys().map(|s| s.as_str()).collect()
    }

    /// 注册内置 Skill（text-to-ui, image-to-ui, partial-edit, local-edit,
    /// multi-variants, spec-doc, page-flow）。
    ///
    /// Ruling #16：7 skill 持 `model: String` 字段，`register_builtin(model)` 传入。
    pub fn register_builtin(&mut self, model: &str) {
        self.register(Box::new(TextToUiSkill::new(model)));
        self.register(Box::new(ImageToUiSkill::new(model)));
        self.register(Box::new(PartialEditSkill::new(model)));
        self.register(Box::new(LocalEditSkill::new(model)));
        self.register(Box::new(MultiVariantsSkill::new(model)));
        self.register(Box::new(SpecDocSkill::new(model)));
        self.register(Box::new(PageFlowSkill::new(model)));
    }
}

// ── 内置 Skill 实现 ──

/// 文生 UI Skill。Ruling #16：持 model 字段。
pub struct TextToUiSkill {
    model: String,
}

impl TextToUiSkill {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

/// 图生 UI Skill。
pub struct ImageToUiSkill {
    model: String,
}

impl ImageToUiSkill {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

/// 局部编辑 Skill。
pub struct PartialEditSkill {
    model: String,
}

impl PartialEditSkill {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

/// 本地编辑 Skill：框选多节点 + 自然语言指令 → 批量修改。
///
/// input 格式: "node1_json\n---\nnode2_json\n---\n...|||instruction"
pub struct LocalEditSkill {
    model: String,
}

impl LocalEditSkill {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

/// 设计规范文档生成 Skill：从 PenDocument JSON 生成交互规范/组件规范/页面架构文档。
///
/// input 格式: "pen_document_json|spec_title"
pub struct SpecDocSkill {
    model: String,
}

impl SpecDocSkill {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

/// 页面流程批量生成 Skill：生成完整页面流程序列（首页→列表→详情→弹窗），统一风格。
///
/// input 格式: "flow_desc|style_hint"
/// flow_desc 示例: "电商应用:首页,商品列表,商品详情,购物车,结算"
pub struct PageFlowSkill {
    model: String,
}

impl PageFlowSkill {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

/// 多方案对比 Skill。
pub struct MultiVariantsSkill {
    model: String,
}

impl MultiVariantsSkill {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

// ── 7 Skill trait 实现（逐字迁代码体，改 3 处：self.model / design_system 参数 / &dyn SkillContext）──

impl DesignSkill for TextToUiSkill {
    fn id(&self) -> &str {
        "text-to-ui"
    }
    fn label(&self) -> &str {
        "文生 UI"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        let mut sys = "你是 fusion-design UI 生成器。输出严格 JSON：{\"page\":{...}}。\
只输出 JSON，禁止额外文字。page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}。"
            .to_string();
        if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user = format!("生成页面。需求：{input}");
        let resp = ctx.chat(&self.model, &sys, &user, 2048)?;
        let doc = parse_ui_json(&resp, "generated")?;
        Ok(SkillOutput::Document(doc))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: &'a dyn SkillContext,
        design_system: Option<&'a DesignSystem>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut sys = "你是 fusion-design UI 生成器。输出严格 JSON：{\"page\":{...}}。\
只输出 JSON，禁止额外文字。page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}。"
                .to_string();
            if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!("生成页面。需求：{input}");
            let resp = ctx.chat_async(&self.model, &sys, &user, 2048).await?;
            let doc = parse_ui_json(&resp, "generated")?;
            Ok(SkillOutput::Document(doc))
        })
    }
}

impl DesignSkill for ImageToUiSkill {
    fn id(&self) -> &str {
        "image-to-ui"
    }
    fn label(&self) -> &str {
        "图生 UI"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        let parts: Vec<&str> = input.splitn(3, '|').collect();
        let sketch_path = parts[0];
        let hint = parts.get(1).unwrap_or(&"");
        let page_name = parts.get(2).unwrap_or(&"generated");
        let mut sys = "你是 fusion-design UI 生成器。根据用户提供的草图图片与说明，\
输出严格 JSON：{\"page\":{...}}。只输出 JSON，禁止额外文字与 markdown 围栏。\
page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?,children?}。\
示例：{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[\
{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":900,\"fill\":\"#ffffff\"},\
{\"id\":\"n1\",\"kind\":\"rect\",\"x\":560,\"y\":360,\"w\":320,\"h\":48,\"fill\":\"#f0f0f0\"}\
]}}。"
            .to_string();
        if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user =
            format!("补充说明：{hint}\n请根据上方草图图片生成页面「{page_name}」对应的 UI 布局。");
        let resp = match encode_image_base64(std::path::Path::new(sketch_path)) {
            Ok(b64) => {
                tracing::info!(
                    sketch_path,
                    bytes = b64.len(),
                    "image-to-ui: 已加载草图，发送真实多模态请求"
                );
                ctx.chat_with_image(&self.model, &sys, &user, &b64, DEFAULT_MAX_TOKENS)?
            }
            Err(e) => {
                tracing::warn!(sketch_path, error = %e, "image-to-ui: 草图加载失败，回退文字描述");
                let user_text = format!(
                    "草图路径：{sketch_path}（无法读取：{e}）\n补充说明：{hint}\n生成页面「{page_name}」对应的 UI 布局。"
                );
                ctx.chat(&self.model, &sys, &user_text, 2048)?
            }
        };
        let doc = parse_ui_json(&resp, page_name)?;
        Ok(SkillOutput::Document(doc))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: &'a dyn SkillContext,
        design_system: Option<&'a DesignSystem>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let parts: Vec<&str> = input.splitn(3, '|').collect();
            let sketch_path = parts[0];
            let hint = parts.get(1).unwrap_or(&"");
            let page_name = parts.get(2).unwrap_or(&"generated");
            let mut sys = "你是 fusion-design UI 生成器。根据用户提供的草图图片与说明，\
输出严格 JSON：{\"page\":{...}}。只输出 JSON，禁止额外文字与 markdown 围栏。\
page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?,children?}。\
示例：{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[\
{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":900,\"fill\":\"#ffffff\"},\
{\"id\":\"n1\",\"kind\":\"rect\",\"x\":560,\"y\":360,\"w\":320,\"h\":48,\"fill\":\"#f0f0f0\"}\
]}}。"
                .to_string();
            if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!(
                "补充说明：{hint}\n请根据上方草图图片生成页面「{page_name}」对应的 UI 布局。"
            );
            // R-13：async 上下文用 tokio::fs 异步编码，不阻塞 worker 线程。
            let resp = match encode_image_base64_async(std::path::Path::new(sketch_path)).await {
                Ok(b64) => {
                    tracing::info!(
                        sketch_path,
                        bytes = b64.len(),
                        "image-to-ui: 已加载草图，发送真实多模态请求"
                    );
                    ctx.chat_with_image_async(&self.model, &sys, &user, &b64, DEFAULT_MAX_TOKENS)
                        .await?
                }
                Err(e) => {
                    tracing::warn!(sketch_path, error = %e, "image-to-ui: 草图加载失败，回退文字描述");
                    let user_text = format!(
                        "草图路径：{sketch_path}（无法读取：{e}）\n补充说明：{hint}\n生成页面「{page_name}」对应的 UI 布局。"
                    );
                    ctx.chat_async(&self.model, &sys, &user_text, 2048).await?
                }
            };
            let doc = parse_ui_json(&resp, page_name)?;
            Ok(SkillOutput::Document(doc))
        })
    }
}

impl DesignSkill for PartialEditSkill {
    fn id(&self) -> &str {
        "partial-edit"
    }
    fn label(&self) -> &str {
        "局部编辑"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        _design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        // input 格式: "node_json|instruction"
        let parts: Vec<&str> = input.splitn(2, '|').collect();
        let node_json = parts[0];
        let instruction = parts.get(1).unwrap_or(&"修改为更合适的样式");
        let sys = "你是 fusion-design 局部编辑器。输入一个节点 JSON 和编辑指令，\
输出修改后的节点 JSON（保持原字段，仅变更指令涉及的字段）。只输出 JSON。";
        let user = format!("节点：{node_json}\n指令：{instruction}");
        let resp = ctx.chat(&self.model, sys, &user, 1024)?;
        Ok(SkillOutput::PartialEdit(resp))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: &'a dyn SkillContext,
        _design_system: Option<&'a DesignSystem>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let parts: Vec<&str> = input.splitn(2, '|').collect();
            let node_json = parts[0];
            let instruction = parts.get(1).unwrap_or(&"修改为更合适的样式");
            let sys = "你是 fusion-design 局部编辑器。输入一个节点 JSON 和编辑指令，\
输出修改后的节点 JSON（保持原字段，仅变更指令涉及的字段）。只输出 JSON。";
            let user = format!("节点：{node_json}\n指令：{instruction}");
            let resp = ctx.chat_async(&self.model, sys, &user, 1024).await?;
            Ok(SkillOutput::PartialEdit(resp))
        })
    }
}

impl DesignSkill for LocalEditSkill {
    fn id(&self) -> &str {
        "local-edit"
    }
    fn label(&self) -> &str {
        "本地编辑"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        let (nodes_part, instruction) = parse_local_edit_input(input);
        let mut sys = "你是 fusion-design 本地编辑器。输入多个节点的 JSON 数组和编辑指令，\
输出修改后的节点 JSON 数组（保持原字段，仅变更指令涉及的字段）。只输出 JSON 数组。"
            .to_string();
        if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user = format!("选中节点：\n{nodes_part}\n\n编辑指令：{instruction}");
        let resp = ctx.chat(&self.model, &sys, &user, 2048)?;
        Ok(SkillOutput::PartialEdit(resp))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: &'a dyn SkillContext,
        design_system: Option<&'a DesignSystem>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let (nodes_part, instruction) = parse_local_edit_input(&input);
            let mut sys = "你是 fusion-design 本地编辑器。输入多个节点的 JSON 数组和编辑指令，\
输出修改后的节点 JSON 数组（保持原字段，仅变更指令涉及的字段）。只输出 JSON 数组。"
                .to_string();
            if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!("选中节点：\n{nodes_part}\n\n编辑指令：{instruction}");
            let resp = ctx.chat_async(&self.model, &sys, &user, 2048).await?;
            Ok(SkillOutput::PartialEdit(resp))
        })
    }
}

/// 解析 local-edit 输入格式。
/// 格式: "node_jsons|||instruction" 或 "node_jsons|instruction"（向后兼容）
pub fn parse_local_edit_input(input: &str) -> (String, &str) {
    if let Some((nodes, instr)) = input.splitn(2, "|||").collect::<Vec<_>>().split_first() {
        if !instr.is_empty() {
            return (nodes.to_string(), instr[0]);
        }
    }
    let parts: Vec<&str> = input.splitn(2, '|').collect();
    let nodes = parts[0].to_string();
    let instr = parts.get(1).unwrap_or(&"修改为更合适的样式");
    (nodes, instr)
}

impl DesignSkill for SpecDocSkill {
    fn id(&self) -> &str {
        "spec-doc"
    }
    fn label(&self) -> &str {
        "设计规范文档"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        let (doc_json, title) = parse_spec_doc_input(input);
        let mut sys = "你是 fusion-design 设计规范文档生成器。输入一个 PenDocument JSON，\
分析其页面结构和节点，输出严格 JSON：{\"title\":\"...\",\"page_architecture\":\"...\",\
\"interaction_specs\":[...],\"component_specs\":[...],\"token_summary\":\"...\"}。\
只输出 JSON，禁止额外文字。\n\n\
interaction_specs 每项含：id, element, event, behavior, animation?, notes?\n\
component_specs 每项含：id, name, kind, props[{name,prop_type,default_value?,description?}], variants[], accessibility?\n\n\
规范要求：\n\
1. 为每个可交互节点（按钮/输入框/链接等）生成交互规范\n\
2. 为每种组件类型生成组件规范，包含属性、变体、无障碍说明\n\
3. 页面架构用文字描述布局层次和导航关系\n\
4. token_summary 汇总设计系统 Token 使用情况".to_string();
        if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user = format!("PenDocument：{doc_json}\n\n生成设计规范文档「{title}」。");
        let resp = ctx.chat(&self.model, &sys, &user, DEFAULT_MAX_TOKENS)?;
        let spec = parse_spec_doc_json(&resp, title)?;
        Ok(SkillOutput::SpecDoc(spec))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: &'a dyn SkillContext,
        design_system: Option<&'a DesignSystem>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let (doc_json, title) = parse_spec_doc_input(&input);
            let mut sys = "你是 fusion-design 设计规范文档生成器。输入一个 PenDocument JSON，\
分析其页面结构和节点，输出严格 JSON：{\"title\":\"...\",\"page_architecture\":\"...\",\
\"interaction_specs\":[...],\"component_specs\":[...],\"token_summary\":\"...\"}。\
只输出 JSON，禁止额外文字。\n\n\
interaction_specs 每项含：id, element, event, behavior, animation?, notes?\n\
component_specs 每项含：id, name, kind, props[{name,prop_type,default_value?,description?}], variants[], accessibility?\n\n\
规范要求：\n\
1. 为每个可交互节点（按钮/输入框/链接等）生成交互规范\n\
2. 为每种组件类型生成组件规范，包含属性、变体、无障碍说明\n\
3. 页面架构用文字描述布局层次和导航关系\n\
4. token_summary 汇总设计系统 Token 使用情况".to_string();
            if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!("PenDocument：{doc_json}\n\n生成设计规范文档「{title}」。");
            let resp = ctx
                .chat_async(&self.model, &sys, &user, DEFAULT_MAX_TOKENS)
                .await?;
            let spec = parse_spec_doc_json(&resp, title)?;
            Ok(SkillOutput::SpecDoc(spec))
        })
    }
}

pub fn parse_spec_doc_input(input: &str) -> (&str, &str) {
    let parts: Vec<&str> = input.splitn(2, '|').collect();
    let doc_json = parts[0];
    let title = parts.get(1).unwrap_or(&"设计规范文档");
    (doc_json, title)
}

pub fn parse_spec_doc_json(json: &str, fallback_title: &str) -> anyhow::Result<SpecDocument> {
    let cleaned = strip_code_fence(json);
    let v: serde_json::Value = serde_json::from_str(cleaned)?;

    let title = v
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or(fallback_title)
        .to_string();
    let page_architecture = v
        .get("page_architecture")
        .and_then(|a| a.as_str())
        .unwrap_or("")
        .to_string();
    let token_summary = v
        .get("token_summary")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    let interaction_specs: Vec<InteractionSpec> = v
        .get("interaction_specs")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    Some(InteractionSpec {
                        id: obj.get("id")?.as_str()?.to_string(),
                        element: obj.get("element")?.as_str()?.to_string(),
                        event: obj.get("event")?.as_str()?.to_string(),
                        behavior: obj.get("behavior")?.as_str()?.to_string(),
                        animation: obj
                            .get("animation")
                            .and_then(|a| a.as_str())
                            .map(String::from),
                        notes: obj.get("notes").and_then(|n| n.as_str()).map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let component_specs: Vec<ComponentSpec> = v
        .get("component_specs")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    Some(ComponentSpec {
                        id: obj.get("id")?.as_str()?.to_string(),
                        name: obj.get("name")?.as_str()?.to_string(),
                        kind: obj.get("kind")?.as_str()?.to_string(),
                        props: obj
                            .get("props")
                            .and_then(|p| p.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|p| {
                                        let o = p.as_object()?;
                                        Some(ComponentProp {
                                            name: o.get("name")?.as_str()?.to_string(),
                                            prop_type: o.get("prop_type")?.as_str()?.to_string(),
                                            default_value: o
                                                .get("default_value")
                                                .and_then(|d| d.as_str())
                                                .map(String::from),
                                            description: o
                                                .get("description")
                                                .and_then(|d| d.as_str())
                                                .map(String::from),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        variants: obj
                            .get("variants")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        accessibility: obj
                            .get("accessibility")
                            .and_then(|a| a.as_str())
                            .map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    tracing::info!(
        title = %title,
        interactions = interaction_specs.len(),
        components = component_specs.len(),
        "parse_spec_doc_json: 规范文档解析完成"
    );

    Ok(SpecDocument {
        title,
        page_architecture,
        interaction_specs,
        component_specs,
        token_summary,
    })
}

impl DesignSkill for PageFlowSkill {
    fn id(&self) -> &str {
        "page-flow"
    }
    fn label(&self) -> &str {
        "页面流程生成"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        let (flow_desc, style_hint) = parse_page_flow_input(input);
        let pages = parse_flow_pages(flow_desc);
        let text_skill = TextToUiSkill::new(&self.model);
        let mut docs = Vec::with_capacity(pages.len());
        for page_name in &pages {
            let prompt = format!("{flow_desc}（页面：{page_name}，风格：{style_hint}）");
            match text_skill.execute(ctx, design_system, &prompt)? {
                SkillOutput::Document(d) => docs.push(d),
                _ => anyhow::bail!("text-to-ui 返回非 Document"),
            }
        }
        tracing::info!(count = docs.len(), "page-flow: 流程生成完成");
        Ok(SkillOutput::PageFlow(docs))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: &'a dyn SkillContext,
        design_system: Option<&'a DesignSystem>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let (flow_desc, style_hint) = parse_page_flow_input(&input);
            let pages = parse_flow_pages(flow_desc);
            let text_skill = TextToUiSkill::new(&self.model);
            let mut docs = Vec::with_capacity(pages.len());
            for page_name in &pages {
                let prompt = format!("{flow_desc}（页面：{page_name}，风格：{style_hint}）");
                match text_skill.execute_async(ctx, design_system, prompt).await? {
                    SkillOutput::Document(d) => docs.push(d),
                    _ => anyhow::bail!("text-to-ui 返回非 Document"),
                }
            }
            tracing::info!(count = docs.len(), "page-flow: 流程生成完成");
            Ok(SkillOutput::PageFlow(docs))
        })
    }
}

pub fn parse_page_flow_input(input: &str) -> (&str, &str) {
    let parts: Vec<&str> = input.splitn(2, '|').collect();
    let flow_desc = parts[0];
    let style = parts.get(1).unwrap_or(&"简约");
    (flow_desc, style)
}

/// 从流程描述中提取页面列表。
/// 格式: "应用名:页面1,页面2,页面3" 或直接 "页面1,页面2,页面3"
pub fn parse_flow_pages(flow_desc: &str) -> Vec<String> {
    let desc = flow_desc.trim();
    if desc.is_empty() {
        return vec!["首页".to_string()];
    }
    let pages_str = if desc.contains(':') {
        desc.split_once(':').map(|(_, s)| s).unwrap_or("")
    } else {
        desc
    };
    let pages: Vec<String> = pages_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if pages.is_empty() {
        vec!["首页".to_string()]
    } else {
        pages
    }
}

impl DesignSkill for MultiVariantsSkill {
    fn id(&self) -> &str {
        "multi-variants"
    }
    fn label(&self) -> &str {
        "多方案对比"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        // input 格式: "prompt|style1|style2|style3"
        let parts: Vec<&str> = input.splitn(4, '|').collect();
        let prompt = parts[0];
        let styles = [
            parts.get(1).unwrap_or(&"简约"),
            parts.get(2).unwrap_or(&"玻璃拟态"),
            parts.get(3).unwrap_or(&"深色"),
        ];
        let text_skill = TextToUiSkill::new(&self.model);
        let v1_doc = match text_skill.execute(
            ctx,
            design_system,
            &format!("{prompt}（风格：{}）", styles[0]),
        )? {
            SkillOutput::Document(d) => d,
            _ => anyhow::bail!("text-to-ui 返回非 Document"),
        };
        let v2_doc = match text_skill.execute(
            ctx,
            design_system,
            &format!("{prompt}（风格：{}）", styles[1]),
        )? {
            SkillOutput::Document(d) => d,
            _ => anyhow::bail!("text-to-ui 返回非 Document"),
        };
        let v3_doc = match text_skill.execute(
            ctx,
            design_system,
            &format!("{prompt}（风格：{}）", styles[2]),
        )? {
            SkillOutput::Document(d) => d,
            _ => anyhow::bail!("text-to-ui 返回非 Document"),
        };
        Ok(SkillOutput::MultiVariants([v1_doc, v2_doc, v3_doc]))
    }

    fn execute_async<'a>(
        &'a self,
        ctx: &'a dyn SkillContext,
        design_system: Option<&'a DesignSystem>,
        input: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SkillOutput>> + Send + 'a>>
    {
        Box::pin(async move {
            let parts: Vec<&str> = input.splitn(4, '|').collect();
            let prompt = parts[0];
            let styles = [
                parts.get(1).unwrap_or(&"简约"),
                parts.get(2).unwrap_or(&"玻璃拟态"),
                parts.get(3).unwrap_or(&"深色"),
            ];
            let text_skill = TextToUiSkill::new(&self.model);
            let v1_doc = match text_skill
                .execute_async(
                    ctx,
                    design_system,
                    format!("{prompt}（风格：{}）", styles[0]),
                )
                .await?
            {
                SkillOutput::Document(d) => d,
                _ => anyhow::bail!("text-to-ui 返回非 Document"),
            };
            let v2_doc = match text_skill
                .execute_async(
                    ctx,
                    design_system,
                    format!("{prompt}（风格：{}）", styles[1]),
                )
                .await?
            {
                SkillOutput::Document(d) => d,
                _ => anyhow::bail!("text-to-ui 返回非 Document"),
            };
            let v3_doc = match text_skill
                .execute_async(
                    ctx,
                    design_system,
                    format!("{prompt}（风格：{}）", styles[2]),
                )
                .await?
            {
                SkillOutput::Document(d) => d,
                _ => anyhow::bail!("text-to-ui 返回非 Document"),
            };
            Ok(SkillOutput::MultiVariants([v1_doc, v2_doc, v3_doc]))
        })
    }
}

// ── 纯 helper（逐字迁自 fd-ai-adapter，仅改可见性为 pub）──

/// 读取图片文件并编码为 base64。
/// P-3：读前按 metadata 预检 ≤50MB，超大草图直接 bail 防 OOM/base64 膨胀。
pub fn encode_image_base64(path: &std::path::Path) -> anyhow::Result<String> {
    const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
    let len = std::fs::metadata(path)
        .map_err(|e| anyhow::anyhow!("读取草图元数据失败 {}: {e}", path.display()))?
        .len();
    if len > MAX_IMAGE_BYTES {
        tracing::warn!(size = len, "草图超过 50MB 上限，拒绝编码");
        anyhow::bail!("草图 {} 超过 50MB 上限（{} 字节）", path.display(), len);
    }
    let bytes = std::fs::read(path)?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &bytes,
    ))
}

/// R-13：异步版图片 base64 编码。async 上下文下用 tokio::fs 替代 std::fs，
/// 避免同步 IO 阻塞 tokio worker 线程（image_to_ui_async 调用路径）。
/// 同步版保留供阻塞调用方（image_to_ui 同步版走 BLOCKING_RT）。
pub async fn encode_image_base64_async(path: &std::path::Path) -> anyhow::Result<String> {
    const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
    let len = tokio::fs::metadata(path)
        .await
        .map_err(|e| anyhow::anyhow!("读取草图元数据失败 {}: {e}", path.display()))?
        .len();
    if len > MAX_IMAGE_BYTES {
        tracing::warn!(size = len, "草图超过 50MB 上限，拒绝编码");
        anyhow::bail!("草图 {} 超过 50MB 上限（{} 字节）", path.display(), len);
    }
    let bytes = tokio::fs::read(path).await?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &bytes,
    ))
}

/// 解析 fusion-mlx 返回的 UI JSON 为 PenDocument。
///
/// 兼容形状：`{"page": {"width":..,"height":..,"nodes":[..]}}` 或裸 `{"nodes":[..]}`。
/// 文生/图生 UI 共用 system prompt：显式约束 node schema + 示例，
/// 避免 7B 模型自行发明 components/type 等不匹配 schema 的结构。
pub fn ui_generator_system_prompt() -> String {
    "你是 fusion-design UI 生成器。输出严格 JSON：{\"page\":{...}}。\
只输出 JSON，禁止额外文字与 markdown 围栏。\
page 含 width/height（默认 1440×900），nodes 列表每项 \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?,children?}。\
示例：{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[\
{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":900,\"fill\":\"#ffffff\"},\
{\"id\":\"n1\",\"kind\":\"rect\",\"x\":560,\"y\":360,\"w\":320,\"h\":48,\"fill\":\"#f0f0f0\"}\
]}}。"
        .to_string()
}

pub fn parse_ui_json(json: &str, page_name: &str) -> anyhow::Result<PenDocument> {
    // 容错：模型可能包裹 markdown ```json ... ```，剥之
    let cleaned = strip_code_fence(json);
    let v: serde_json::Value = match serde_json::from_str(cleaned) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "parse_ui_json: 原始 JSON 解析失败，尝试修复");
            let repaired = repair_model_json(cleaned);
            match serde_json::from_str::<serde_json::Value>(&repaired) {
                Ok(v) => {
                    tracing::info!("parse_ui_json: JSON 修复后解析成功");
                    v
                }
                // R-2：修复仍失败 → fail visibly，不再静默回退合成占位文档
                // （审计 R-2：伪成功占位掩盖模型输出错误，用户须看到真实失败）。
                Err(e2) => {
                    tracing::error!(error = %e2, "parse_ui_json: JSON 修复仍失败，拒绝伪成功");
                    anyhow::bail!("模型输出非合法 JSON，修复失败: {e2}");
                }
            }
        }
    };
    let page_obj = v.get("page").and_then(|p| p.as_object());
    let (w, h) = match page_obj {
        Some(o) => (
            o.get("width").and_then(|x| x.as_f64()).unwrap_or(1440.0),
            o.get("height").and_then(|x| x.as_f64()).unwrap_or(900.0),
        ),
        None => (1440.0, 900.0),
    };
    let nodes_val = page_obj
        .and_then(|p| p.get("nodes"))
        .or_else(|| v.get("nodes"));
    let nodes = match nodes_val {
        Some(nv) => match parse_nodes_with_depth(nv, 0) {
            Ok(n) => n,
            // R-2：nodes 解析失败 → fail visibly（半结构化损坏同样不可伪成功）。
            Err(e) => {
                tracing::error!(error = %e, "parse_ui_json: nodes 解析失败，拒绝伪成功");
                anyhow::bail!("模型输出 nodes 解析失败: {e}");
            }
        },
        // R-2：缺 nodes 字段视为合法空页（非伪成功——空文档是有效状态，
        // 不注入合成的占位元素）。模型可能输出仅 page 尺寸无节点的设计稿。
        None => {
            tracing::info!("parse_ui_json: JSON 缺 nodes 字段，返回空页");
            let mut doc = PenDocument::new();
            let page = Page::new("page_1", page_name, w, h);
            doc.add_page(page);
            return Ok(doc);
        }
    };
    let validated: Vec<PenNode> = nodes.into_iter().map(validate_node).collect();
    let mut doc = PenDocument::new();
    let mut page = Page::new("page_1", page_name, w, h);
    for n in validated {
        page.add(n);
    }
    doc.add_page(page);
    Ok(doc)
}

/// 修复 7B 模型常见 JSON 语法错误（纯字符串替换，避免引入 regex 依赖）：
/// - `"k":"":"v"` 双冒号 → `"k":"v"`
/// - `"k":"v "k2"` 值内吞引号缺逗号 → `"k":"v","k2"`
/// - 数字/右括号后紧跟空格再接引号键：`100 "w"` → `100,"w"`
/// - `{ {` / `} }` 连续同括号去重
/// - 括号失衡：尾部多余 `}` `]` 裁剪；EOF 处缺括号按栈补齐
pub fn repair_model_json(s: &str) -> String {
    let mut out = s.to_string();
    while out.contains("\":\"\":\"") {
        out = out.replace("\":\"\":\"", "\":\"");
    }
    out = out.replace("{ {", "{").replace("} }", "}");
    // 扫描修复"值后空格+引号键"缺逗号：形如 `#fff "stroke"` 或 `100 "w"`
    // 逐字节匹配 ASCII 模式（空格/引号/字母），但 CJK 等多字节 UTF-8 字符
    // 必须按原字节复制——旧 `bytes[i] as char` 把每个字节当 Latin-1 码点转
    // char 再以 UTF-8 编码，CJK 3 字节变 6 字节乱码（R-A9）。模式仅涉及 ASCII，
    // 非首字节（>=0x80）永不匹配，故按 UTF-8 字符边界推进复制即可。
    let bytes = out.as_bytes();
    let mut rebuilt = String::with_capacity(out.len());
    let mut i = 0;
    while i < bytes.len() {
        // 检测模式：空格 + `"` + ASCII 字母 + ... + `":`，且前一非空字符非 `, : [ {`
        if bytes[i] == b' '
            && i + 3 < bytes.len()
            && bytes[i + 1] == b'"'
            && bytes[i + 2].is_ascii_alphabetic()
        {
            // 找到对应的 `":` 结束
            let mut j = i + 2;
            while j + 1 < bytes.len() && !(bytes[j] == b'"' && bytes[j + 1] == b':') {
                j += 1;
            }
            if j + 1 < bytes.len() && bytes[j] == b'"' && bytes[j + 1] == b':' {
                let prev = rebuilt.trim_end().chars().last();
                let need_comma = match prev {
                    Some(',') | Some(':') | Some('[') | Some('{') => false,
                    Some(_) => true,
                    None => false,
                };
                if need_comma {
                    rebuilt.push(',');
                }
            }
        }
        // 按 UTF-8 字符边界复制原字节：ASCII 1 字节，多字节字符整体 push。
        let char_len = utf8_char_len(bytes[i]);
        let end = (i + char_len).min(bytes.len());
        if let Ok(slice) = std::str::from_utf8(&bytes[i..end]) {
            rebuilt.push_str(slice);
        } else {
            // 残缺多字节序列（输入本身非法），整体跳过避免乱码扩散。
            tracing::warn!(
                at = i,
                len = char_len,
                "repair_model_json 跳过残缺 UTF-8 序列"
            );
        }
        i = end;
    }
    // 括号失衡修复：7B 模型常输出尾部多余 `}` 或中途截断。
    balance_json_braces(&mut rebuilt);
    rebuilt
}

/// 由首字节判定 UTF-8 字符占用的字节数（1..=4）。非法/续字节返回 1（逐字节跳过）。
fn utf8_char_len(b: u8) -> usize {
    if b < 0xC0 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    }
}

/// 按 JSON 语义平衡花括号/方括号（跳过字符串字面量与转义）。
/// - 扫描中深度变负（多余 `}`/`]`）则截断后续内容；
/// - 末尾深度仍 > 0 则按栈逆序补齐缺失闭括号。
fn balance_json_braces(s: &mut String) {
    let bytes = s.as_bytes();
    let mut stack: Vec<u8> = Vec::new();
    let mut in_str = false;
    let mut escape = false;
    let mut cut_len: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if cut_len.is_some() {
            break;
        }
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
        } else if b == b'"' {
            in_str = true;
        } else if b == b'{' || b == b'[' {
            stack.push(b);
        } else if b == b'}' {
            if stack.last() == Some(&b'{') {
                stack.pop();
            } else {
                cut_len = Some(i);
            }
        } else if b == b']' {
            if stack.last() == Some(&b'[') {
                stack.pop();
            } else {
                cut_len = Some(i);
            }
        }
        i += 1;
    }
    if let Some(pos) = cut_len {
        s.truncate(pos);
        tracing::info!(pos, "balance_json_braces: 裁剪尾部多余闭括号");
        return;
    }
    if !stack.is_empty() {
        let mut tail = String::new();
        while let Some(b) = stack.pop() {
            tail.push(if b == b'{' { '}' } else { ']' });
        }
        tracing::info!(missing = tail.len(), "balance_json_braces: 补齐缺失闭括号");
        s.push_str(&tail);
    }
}

const MAX_NODE_DEPTH: usize = 20;

pub fn parse_nodes_with_depth(v: &serde_json::Value, depth: usize) -> anyhow::Result<Vec<PenNode>> {
    if depth > MAX_NODE_DEPTH {
        anyhow::bail!("节点嵌套深度超过 {MAX_NODE_DEPTH}，拒绝解析");
    }
    let arr = v
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("nodes 非数组"))?;
    let mut nodes = Vec::with_capacity(arr.len());
    // E-6/P3：sanitize_node_id 过滤后不同原始 id 可能归一（"a-b!" 和 "a-b" → "a-b"），
    // 同级 id 碰撞致后续节点操作错乱。per-array seen 集合对碰撞 id 追加 _<n> 去重。
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (i, item) in arr.iter().enumerate() {
        let mut node = parse_node_with_depth(item, i, depth)?;
        if !seen.insert(node.id.clone()) {
            let original = node.id.clone();
            let mut suffix = 2;
            loop {
                let candidate = format!("{original}_{suffix}");
                if seen.insert(candidate.clone()) {
                    node.id = candidate;
                    break;
                }
                suffix += 1;
            }
            tracing::warn!(original = %original, new = %node.id, "sanitize_node_id 碰撞，已追加后缀去重");
        }
        nodes.push(node);
    }
    Ok(nodes)
}

pub fn parse_node_with_depth(
    item: &serde_json::Value,
    idx: usize,
    depth: usize,
) -> anyhow::Result<PenNode> {
    let o = item
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("节点 {idx} 非对象"))?;
    let kind_str = o.get("kind").and_then(|k| k.as_str()).unwrap_or("rect");
    let kind = match kind_str {
        "rect" => NodeKind::Rect,
        "circle" => NodeKind::Circle,
        "text" => NodeKind::Text,
        "image" => NodeKind::Image,
        "group" => NodeKind::Group,
        other => anyhow::bail!("未知节点类型 {other:?}"),
    };
    let id = o
        .get("id")
        .and_then(|x| x.as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("n_{idx}"));
    let id = sanitize_node_id(&id);
    let name = o
        .get("name")
        .and_then(|x| x.as_str())
        .map(String::from)
        .unwrap_or_else(|| kind_str.to_string());
    let get_f = |key: &str, d: f64| o.get(key).and_then(|x| x.as_f64()).unwrap_or(d);
    let text = o.get("text").and_then(|x| x.as_str()).map(String::from);
    let fill = o.get("fill").and_then(|x| x.as_str()).map(String::from);
    let stroke = o.get("stroke").and_then(|x| x.as_str()).map(String::from);
    let style = fd_canvas_core::NodeStyle {
        fill,
        stroke,
        ..Default::default()
    };
    let children_val = o.get("children");
    let children = match children_val {
        Some(cv) => parse_nodes_with_depth(cv, depth + 1)?,
        None => vec![],
    };
    Ok(PenNode {
        id,
        kind,
        name,
        x: get_f("x", 0.0),
        y: get_f("y", 0.0),
        w: get_f("w", 0.0),
        h: get_f("h", 0.0),
        style,
        text,
        children,
        rotation: get_f("rotation", 0.0),
        z_index: get_f("z_index", 0.0) as i32,
    })
}

fn sanitize_node_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if sanitized.is_empty() {
        "node".to_string()
    } else {
        sanitized
    }
}

fn validate_node(mut node: PenNode) -> PenNode {
    if node.w <= 0.0 {
        tracing::warn!("节点 {} w={:.1} 非正，修正为 1.0", node.id, node.w);
        node.w = 1.0;
    }
    if node.h <= 0.0 {
        tracing::warn!("节点 {} h={:.1} 非正，修正为 1.0", node.id, node.h);
        node.h = 1.0;
    }
    node.children = node.children.into_iter().map(validate_node).collect();
    node
}

pub fn strip_code_fence(s: &str) -> &str {
    let trimmed = s.trim();
    if !trimmed.starts_with("```") {
        return trimmed;
    }
    let inner = trimmed.trim_start_matches('`').trim_end_matches('`');
    inner.trim_start_matches("json").trim()
}
