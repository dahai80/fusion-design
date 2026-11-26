//! fd-skills: AI design skill system. Trait-based capabilities + SkillContext trait
//! to decouple from the inference client.
//!
//! ARCH-11: skill system (7 skills + SkillRegistry + SkillOutput types) + pure helpers
//! (parse_ui_json/repair_model_json/strip_code_fence/encode_image_base64/
//! ui_generator_system_prompt/DEFAULT_MAX_TOKENS) migrated out of fd-ai-adapter.
//! SkillContext became an object-safe trait; fd-ai-adapter implements
//! SkillContext for FusionMlxClient as a reverse one-way dependency
//! (fd-skills ← adapter). fd-skills has zero HTTP: pure traits + helpers + parsing logic.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use fd_canvas_core::{NodeKind, Page, PenDocument, PenNode};
use fd_design_system::DesignSystem;

/// Default max_tokens (generation cap). A5: the old implementation hardcoded 4096 in 9 places; promoted to one shared constant.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

/// SkillContext trait: abstracts the chat capability, decoupling skills from a concrete inference client.
///
/// Object-safe (no generics/Self); SkillRegistry uses `&dyn SkillContext` to avoid generic sprawl.
/// fd-ai-adapter `impl SkillContext for FusionMlxClient` implements this trait in reverse.
pub trait SkillContext: Send + Sync {
    /// Synchronous chat request.
    fn chat(&self, model: &str, sys: &str, user: &str, max_tokens: u32) -> anyhow::Result<String>;
    /// Async chat request (returns a boxed future borrowing self with the same lifetime as the inputs).
    fn chat_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>;
    /// Multimodal async chat (image + text).
    fn chat_with_image_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        image_base64: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>;
    /// Multimodal sync chat (image + text).
    fn chat_with_image(
        &self,
        model: &str,
        sys: &str,
        user: &str,
        image_base64: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String>;
    /// Generates the design-token CSS custom properties fragment injected into the system prompt.
    /// `design_system` is passed as an argument (the original SkillContext struct read a field; the trait takes a parameter).
    /// Returns None when no design system is active.
    fn token_prompt_fragment(&self, design_system: Option<&DesignSystem>) -> Option<String>;
}

/// Skill output types.
#[derive(Debug, Clone)]
pub enum SkillOutput {
    /// Full page document
    Document(PenDocument),
    /// Partial-edit node JSON
    PartialEdit(String),
    /// Multi-variant comparison (3 documents)
    MultiVariants([PenDocument; 3]),
    /// Spec document (interaction/component specs, page architecture)
    SpecDoc(SpecDocument),
    /// Batch page-flow generation (multi-page coherent flow)
    PageFlow(Vec<PenDocument>),
}

/// Spec document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecDocument {
    /// Document title.
    pub title: String,
    /// Page architecture description.
    pub page_architecture: String,
    /// Interaction spec entries.
    pub interaction_specs: Vec<InteractionSpec>,
    /// Component spec entries.
    pub component_specs: Vec<ComponentSpec>,
    /// Design token usage summary.
    pub token_summary: String,
}

/// Interaction spec entry.
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

/// Component spec entry.
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

/// Component property.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentProp {
    pub name: String,
    pub prop_type: String,
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// AI design skill trait — each Skill implements one design capability.
///
/// `execute`/`execute_async` take `&dyn SkillContext` (object-safe trait);
/// each skill holds its own `model: String` field (Ruling #16), and `design_system`
/// is passed in as an argument (Ruling #15: token_prompt_fragment no longer reads a struct field).
pub trait DesignSkill: Send + Sync {
    /// Unique skill id.
    fn id(&self) -> &str;
    /// Skill display name.
    fn label(&self) -> &str;
    /// Synchronous execution.
    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput>;
    /// Async execution (default impl: delegates to the sync version, moving input into the async closure and borrowing it).
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

/// Skill registry: looks up and dispatches skills by id.
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

    /// Registers a skill.
    pub fn register(&mut self, skill: Box<dyn DesignSkill>) {
        self.skills.insert(skill.id().to_string(), skill);
    }

    /// Looks up a skill by id.
    pub fn get(&self, id: &str) -> Option<&dyn DesignSkill> {
        self.skills.get(id).map(|s| s.as_ref())
    }

    /// Lists ids of all registered skills.
    pub fn list(&self) -> Vec<&str> {
        self.skills.keys().map(|s| s.as_str()).collect()
    }

    /// Registers the builtin skills (text-to-ui, image-to-ui, partial-edit, local-edit,
    /// multi-variants, spec-doc, page-flow).
    ///
    /// Ruling #16: the 7 skills hold a `model: String` field, passed in via `register_builtin(model)`.
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

// ── Builtin skill implementations ──

/// Text-to-UI skill. Ruling #16: holds a model field.
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

/// Image-to-UI skill.
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

/// Partial-edit skill.
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

/// Local-edit skill: marquee-select multiple nodes + natural-language instruction → batch modification.
///
/// input format: "node1_json\n---\nnode2_json\n---\n...|||instruction"
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

/// Spec-doc skill: generates an interaction/component spec + page architecture document from PenDocument JSON.
///
/// input format: "pen_document_json|spec_title"
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

/// Page-flow skill: generates a full page-flow sequence (home → list → detail → dialog) in a consistent style.
///
/// input format: "flow_desc|style_hint"
/// flow_desc example: "e-commerce app:home,product list,product detail,cart,checkout"
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

/// Multi-variant comparison skill.
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

// ── 7 skill trait impls (bodies migrated verbatim; 3 changes: self.model / design_system parameter / &dyn SkillContext) ──

impl DesignSkill for TextToUiSkill {
    fn id(&self) -> &str {
        "text-to-ui"
    }
    fn label(&self) -> &str {
        // i18n: user-facing
        "文生 UI"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        let mut sys =
            "You are the fusion-design UI generator. Output strict JSON: {\"page\":{...}}. \
Output JSON only, no extra text. page contains width/height (default 1440x900); each entry in the \
nodes list has {id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}."
                .to_string();
        if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user = format!("Generate a page. Requirement: {input}");
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
            let mut sys =
                "You are the fusion-design UI generator. Output strict JSON: {\"page\":{...}}. \
Output JSON only, no extra text. page contains width/height (default 1440x900); each entry in the \
nodes list has {id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?}."
                    .to_string();
            if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!("Generate a page. Requirement: {input}");
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
        // i18n: user-facing
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
        let mut sys =
            "You are the fusion-design UI generator. Based on the sketch image and notes \
provided by the user, output strict JSON: {\"page\":{...}}. Output JSON only, no extra text and no \
markdown fences. page contains width/height (default 1440x900); each entry in the nodes list has \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?,children?}. Example: \
{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[\
{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":900,\"fill\":\"#ffffff\"},\
{\"id\":\"n1\",\"kind\":\"rect\",\"x\":560,\"y\":360,\"w\":320,\"h\":48,\"fill\":\"#f0f0f0\"}\
]}}."
                .to_string();
        if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user = format!(
            "Additional notes: {hint}\nGenerate the UI layout for page \"{page_name}\" based on the sketch image above."
        );
        let resp = match encode_image_base64(std::path::Path::new(sketch_path)) {
            Ok(b64) => {
                tracing::info!(
                    sketch_path,
                    bytes = b64.len(),
                    "image-to-ui: sketch loaded, sending real multimodal request"
                );
                ctx.chat_with_image(&self.model, &sys, &user, &b64, DEFAULT_MAX_TOKENS)?
            }
            Err(e) => {
                tracing::warn!(sketch_path, error = %e, "image-to-ui: sketch load failed, falling back to text description");
                let user_text = format!(
                    "Sketch path: {sketch_path} (unreadable: {e})\nAdditional notes: {hint}\nGenerate the UI layout for page \"{page_name}\"."
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
            let mut sys =
                "You are the fusion-design UI generator. Based on the sketch image and notes \
provided by the user, output strict JSON: {\"page\":{...}}. Output JSON only, no extra text and no \
markdown fences. page contains width/height (default 1440x900); each entry in the nodes list has \
{id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?,children?}. Example: \
{\"page\":{\"width\":1440,\"height\":900,\"nodes\":[\
{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":900,\"fill\":\"#ffffff\"},\
{\"id\":\"n1\",\"kind\":\"rect\",\"x\":560,\"y\":360,\"w\":320,\"h\":48,\"fill\":\"#f0f0f0\"}\
]}}."
                    .to_string();
            if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!(
                "Additional notes: {hint}\nGenerate the UI layout for page \"{page_name}\" based on the sketch image above."
            );
            // R-13: in async context use tokio::fs for async encoding, so the worker thread is not blocked.
            let resp = match encode_image_base64_async(std::path::Path::new(sketch_path)).await {
                Ok(b64) => {
                    tracing::info!(
                        sketch_path,
                        bytes = b64.len(),
                        "image-to-ui: sketch loaded, sending real multimodal request"
                    );
                    ctx.chat_with_image_async(&self.model, &sys, &user, &b64, DEFAULT_MAX_TOKENS)
                        .await?
                }
                Err(e) => {
                    tracing::warn!(sketch_path, error = %e, "image-to-ui: sketch load failed, falling back to text description");
                    let user_text = format!(
                    "Sketch path: {sketch_path} (unreadable: {e})\nAdditional notes: {hint}\nGenerate the UI layout for page \"{page_name}\"."
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
        // i18n: user-facing
        "局部编辑"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        _design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        // input format: "node_json|instruction"
        let parts: Vec<&str> = input.splitn(2, '|').collect();
        let node_json = parts[0];
        let instruction = parts
            .get(1)
            .unwrap_or(&"Restyle it to a more suitable look");
        let sys = "You are the fusion-design partial editor. Input is one node JSON plus an edit \
instruction; output the modified node JSON (keep the original fields, change only the fields the \
instruction involves). Output JSON only.";
        let user = format!("Node: {node_json}\nInstruction: {instruction}");
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
            let instruction = parts
                .get(1)
                .unwrap_or(&"Restyle it to a more suitable look");
            let sys =
                "You are the fusion-design partial editor. Input is one node JSON plus an edit \
instruction; output the modified node JSON (keep the original fields, change only the fields the \
instruction involves). Output JSON only.";
            let user = format!("Node: {node_json}\nInstruction: {instruction}");
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
        // i18n: user-facing
        "本地编辑"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        let (nodes_part, instruction) = parse_local_edit_input(input);
        let mut sys = "You are the fusion-design local editor. Input is a JSON array of multiple \
nodes plus an edit instruction; output the modified node JSON array (keep the original fields, \
change only the fields the instruction involves). Output a JSON array only."
            .to_string();
        if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user = format!("Selected nodes:\n{nodes_part}\n\nEdit instruction: {instruction}");
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
            let mut sys =
                "You are the fusion-design local editor. Input is a JSON array of multiple \
nodes plus an edit instruction; output the modified node JSON array (keep the original fields, \
change only the fields the instruction involves). Output a JSON array only."
                    .to_string();
            if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!("Selected nodes:\n{nodes_part}\n\nEdit instruction: {instruction}");
            let resp = ctx.chat_async(&self.model, &sys, &user, 2048).await?;
            Ok(SkillOutput::PartialEdit(resp))
        })
    }
}

/// Parses the local-edit input format.
/// Format: "node_jsons|||instruction" or "node_jsons|instruction" (backward compatible)
pub fn parse_local_edit_input(input: &str) -> (String, &str) {
    if let Some((nodes, instr)) = input.splitn(2, "|||").collect::<Vec<_>>().split_first() {
        if !instr.is_empty() {
            return (nodes.to_string(), instr[0]);
        }
    }
    let parts: Vec<&str> = input.splitn(2, '|').collect();
    let nodes = parts[0].to_string();
    let instr = parts
        .get(1)
        .unwrap_or(&"Restyle it to a more suitable look");
    (nodes, instr)
}

impl DesignSkill for SpecDocSkill {
    fn id(&self) -> &str {
        "spec-doc"
    }
    fn label(&self) -> &str {
        // i18n: user-facing
        "设计规范文档"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        let (doc_json, title) = parse_spec_doc_input(input);
        let mut sys = "You are the fusion-design design-spec document generator. Input is a \
PenDocument JSON; analyze its page structure and nodes, and output strict JSON: \
{\"title\":\"...\",\"page_architecture\":\"...\",\"interaction_specs\":[...],\
\"component_specs\":[...],\"token_summary\":\"...\"}. Output JSON only, no extra text.\n\n\
Each interaction_specs entry contains: id, element, event, behavior, animation?, notes?\n\
Each component_specs entry contains: id, name, kind, props[{name,prop_type,default_value?,description?}], variants[], accessibility?\n\n\
Requirements:\n\
1. Generate an interaction spec for every interactive node (buttons/inputs/links, etc.)\n\
2. Generate a component spec for each component type, covering props, variants, and accessibility notes\n\
3. Describe the page architecture in text: layout hierarchy and navigation relationships\n\
4. token_summary summarizes design-system token usage".to_string();
        if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
            sys.push_str("\n\n");
            sys.push_str(&tokens);
        }
        let user =
            format!("PenDocument: {doc_json}\n\nGenerate the design spec document \"{title}\".");
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
            let mut sys = "You are the fusion-design design-spec document generator. Input is a \
PenDocument JSON; analyze its page structure and nodes, and output strict JSON: \
{\"title\":\"...\",\"page_architecture\":\"...\",\"interaction_specs\":[...],\
\"component_specs\":[...],\"token_summary\":\"...\"}. Output JSON only, no extra text.\n\n\
Each interaction_specs entry contains: id, element, event, behavior, animation?, notes?\n\
Each component_specs entry contains: id, name, kind, props[{name,prop_type,default_value?,description?}], variants[], accessibility?\n\n\
Requirements:\n\
1. Generate an interaction spec for every interactive node (buttons/inputs/links, etc.)\n\
2. Generate a component spec for each component type, covering props, variants, and accessibility notes\n\
3. Describe the page architecture in text: layout hierarchy and navigation relationships\n\
4. token_summary summarizes design-system token usage".to_string();
            if let Some(tokens) = ctx.token_prompt_fragment(design_system) {
                sys.push_str("\n\n");
                sys.push_str(&tokens);
            }
            let user = format!(
                "PenDocument: {doc_json}\n\nGenerate the design spec document \"{title}\"."
            );
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
    // i18n: user-facing
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
        "parse_spec_doc_json: spec document parsed"
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
        // i18n: user-facing
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
            let prompt = format!("{flow_desc} (page: {page_name}, style: {style_hint})");
            match text_skill.execute(ctx, design_system, &prompt)? {
                SkillOutput::Document(d) => docs.push(d),
                _ => anyhow::bail!("text-to-ui returned non-Document"),
            }
        }
        tracing::info!(count = docs.len(), "page-flow: flow generation complete");
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
                let prompt = format!("{flow_desc} (page: {page_name}, style: {style_hint})");
                match text_skill.execute_async(ctx, design_system, prompt).await? {
                    SkillOutput::Document(d) => docs.push(d),
                    _ => anyhow::bail!("text-to-ui returned non-Document"),
                }
            }
            tracing::info!(count = docs.len(), "page-flow: flow generation complete");
            Ok(SkillOutput::PageFlow(docs))
        })
    }
}

pub fn parse_page_flow_input(input: &str) -> (&str, &str) {
    let parts: Vec<&str> = input.splitn(2, '|').collect();
    let flow_desc = parts[0];
    // i18n: user-facing
    let style = parts.get(1).unwrap_or(&"简约");
    (flow_desc, style)
}

/// Extracts the page list from a flow description.
/// Format: "app_name:page1,page2,page3" or directly "page1,page2,page3"
pub fn parse_flow_pages(flow_desc: &str) -> Vec<String> {
    let desc = flow_desc.trim();
    if desc.is_empty() {
        // i18n: user-facing
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
        // i18n: user-facing
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
        // i18n: user-facing
        "多方案对比"
    }

    fn execute(
        &self,
        ctx: &dyn SkillContext,
        design_system: Option<&DesignSystem>,
        input: &str,
    ) -> anyhow::Result<SkillOutput> {
        // input format: "prompt|style1|style2|style3"
        let parts: Vec<&str> = input.splitn(4, '|').collect();
        let prompt = parts[0];
        let styles = [
            // i18n: user-facing
            parts.get(1).unwrap_or(&"简约"),
            // i18n: user-facing
            parts.get(2).unwrap_or(&"玻璃拟态"),
            // i18n: user-facing
            parts.get(3).unwrap_or(&"深色"),
        ];
        let text_skill = TextToUiSkill::new(&self.model);
        let v1_doc = match text_skill.execute(
            ctx,
            design_system,
            &format!("{prompt} (style: {})", styles[0]),
        )? {
            SkillOutput::Document(d) => d,
            _ => anyhow::bail!("text-to-ui returned non-Document"),
        };
        let v2_doc = match text_skill.execute(
            ctx,
            design_system,
            &format!("{prompt} (style: {})", styles[1]),
        )? {
            SkillOutput::Document(d) => d,
            _ => anyhow::bail!("text-to-ui returned non-Document"),
        };
        let v3_doc = match text_skill.execute(
            ctx,
            design_system,
            &format!("{prompt} (style: {})", styles[2]),
        )? {
            SkillOutput::Document(d) => d,
            _ => anyhow::bail!("text-to-ui returned non-Document"),
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
                // i18n: user-facing
                parts.get(1).unwrap_or(&"简约"),
                // i18n: user-facing
                parts.get(2).unwrap_or(&"玻璃拟态"),
                // i18n: user-facing
                parts.get(3).unwrap_or(&"深色"),
            ];
            let text_skill = TextToUiSkill::new(&self.model);
            let v1_doc = match text_skill
                .execute_async(
                    ctx,
                    design_system,
                    format!("{prompt} (style: {})", styles[0]),
                )
                .await?
            {
                SkillOutput::Document(d) => d,
                _ => anyhow::bail!("text-to-ui returned non-Document"),
            };
            let v2_doc = match text_skill
                .execute_async(
                    ctx,
                    design_system,
                    format!("{prompt} (style: {})", styles[1]),
                )
                .await?
            {
                SkillOutput::Document(d) => d,
                _ => anyhow::bail!("text-to-ui returned non-Document"),
            };
            let v3_doc = match text_skill
                .execute_async(
                    ctx,
                    design_system,
                    format!("{prompt} (style: {})", styles[2]),
                )
                .await?
            {
                SkillOutput::Document(d) => d,
                _ => anyhow::bail!("text-to-ui returned non-Document"),
            };
            Ok(SkillOutput::MultiVariants([v1_doc, v2_doc, v3_doc]))
        })
    }
}

// ── Pure helpers (migrated verbatim from fd-ai-adapter, only visibility changed to pub) ──

/// Reads an image file and encodes it as base64.
/// P-3: pre-checks size via metadata (≤50MB) before reading; oversized sketches bail early to prevent OOM/base64 bloat.
pub fn encode_image_base64(path: &std::path::Path) -> anyhow::Result<String> {
    const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
    let len = std::fs::metadata(path)
        .map_err(|e| anyhow::anyhow!("failed to read sketch metadata {}: {e}", path.display()))?
        .len();
    if len > MAX_IMAGE_BYTES {
        tracing::warn!(size = len, "sketch exceeds 50MB limit, refusing to encode");
        anyhow::bail!(
            "sketch {} exceeds the 50MB limit ({} bytes)",
            path.display(),
            len
        );
    }
    let bytes = std::fs::read(path)?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &bytes,
    ))
}

/// R-13: async variant of image base64 encoding. Uses tokio::fs instead of std::fs in async context,
/// avoiding sync IO that would block tokio worker threads (image_to_ui_async call path).
/// The sync version is kept for blocking callers (the image_to_ui sync version runs on BLOCKING_RT).
pub async fn encode_image_base64_async(path: &std::path::Path) -> anyhow::Result<String> {
    const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
    let len = tokio::fs::metadata(path)
        .await
        .map_err(|e| anyhow::anyhow!("failed to read sketch metadata {}: {e}", path.display()))?
        .len();
    if len > MAX_IMAGE_BYTES {
        tracing::warn!(size = len, "sketch exceeds 50MB limit, refusing to encode");
        anyhow::bail!(
            "sketch {} exceeds the 50MB limit ({} bytes)",
            path.display(),
            len
        );
    }
    let bytes = tokio::fs::read(path).await?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &bytes,
    ))
}

/// Parses the UI JSON returned by fusion-mlx into a PenDocument.
///
/// Accepted shapes: `{"page": {"width":..,"height":..,"nodes":[..]}}` or bare `{"nodes":[..]}`.
/// Text-to-UI / image-to-UI share one system prompt: it explicitly constrains the node schema + example,
/// preventing 7B models from inventing schema-mismatched structures such as components/type.
pub fn ui_generator_system_prompt() -> String {
    "You are the fusion-design UI generator. Output strict JSON: {\"page\":{...}}. \
Output JSON only, no extra text and no markdown fences. Output single-line compact JSON, \
no newlines or indentation. page contains width/height (default 1440x900); each entry in the \
nodes list has {id,kind(rect|circle|text|image|group),x,y,w,h,text?,fill?,stroke?,children?}. \
Example: {\"page\":{\"width\":1440,\"height\":900,\"nodes\":[\
{\"id\":\"n0\",\"kind\":\"rect\",\"x\":0,\"y\":0,\"w\":1440,\"h\":900,\"fill\":\"#ffffff\"},\
{\"id\":\"n1\",\"kind\":\"rect\",\"x\":560,\"y\":360,\"w\":320,\"h\":48,\"fill\":\"#f0f0f0\"}\
]}}."
        .to_string()
}

pub fn parse_ui_json(json: &str, page_name: &str) -> anyhow::Result<PenDocument> {
    // Tolerance: the model may wrap output in markdown ```json ... ```; strip it
    let cleaned = strip_code_fence(json);
    let v: serde_json::Value = match serde_json::from_str(cleaned) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "parse_ui_json: raw JSON parse failed, attempting repair");
            let repaired = repair_model_json(cleaned);
            match serde_json::from_str::<serde_json::Value>(&repaired) {
                Ok(v) => {
                    tracing::info!("parse_ui_json: parse succeeded after JSON repair");
                    v
                }
                // R-2: if repair still fails → fail visibly, no more silent fallback to a synthetic placeholder document
                // (audit R-2: fake-success placeholders mask model output errors; the user must see the real failure).
                Err(e2) => {
                    // Node-level salvage: when model output is corrupted midway, parse per node to salvage intact nodes
                    // and drop corrupted ones (draft semantics — partial nodes are still usable; fail visibly downgraded to warn).
                    if let Some(doc) = salvage_pen_document(&repaired) {
                        tracing::warn!(error = %e2, "parse_ui_json: whole-document parse failed, node-level salvage succeeded (some nodes dropped)");
                        return Ok(doc);
                    }
                    tracing::error!(error = %e2, "parse_ui_json: JSON repair still failed, refusing fake success");
                    // Keep the raw output on record (truncated to 2KB): required for defect-class triage; without it the model's bad output shape cannot be analyzed.
                    tracing::error!(raw = %String::from_utf8_lossy(&cleaned.as_bytes()[..cleaned.len().min(2048)]), "parse_ui_json: raw output logged for failed repair");
                    anyhow::bail!("model output is not valid JSON, repair failed: {e2}");
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
            // R-2: nodes parse failure → fail visibly (half-structured corruption must not fake success either).
            Err(e) => {
                tracing::error!(error = %e, "parse_ui_json: nodes parse failed, refusing fake success");
                anyhow::bail!("failed to parse nodes from model output: {e}");
            }
        },
        // R-2: a missing nodes field is treated as a valid empty page (not fake success — an empty document is a valid state,
        // no synthetic placeholder elements injected). The model may output a design with only page dimensions and no nodes.
        None => {
            tracing::info!("parse_ui_json: JSON missing nodes field, returning empty page");
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

/// Repairs common JSON syntax errors from 7B models (pure string replacement, avoiding a regex dependency):
/// - `"k":"":"v"` double colon → `"k":"v"`
/// - `"k":"v "k2"` quote swallowed inside a value with missing comma → `"k":"v","k2"`
/// - digit/closing bracket followed by whitespace then a quoted key: `100 "w"` → `100,"w"`
/// - `{ {` / `} }` consecutive duplicate brackets deduplicated
/// - unbalanced brackets: trailing extra `}` `]` trimmed; missing brackets at EOF appended via stack
pub fn repair_model_json(s: &str) -> String {
    let mut out = s.to_string();
    while out.contains("\":\"\":\"") {
        out = out.replace("\":\"\":\"", "\":\"");
    }
    out = out.replace("{ {", "{").replace("} }", "}");
    // Scan-repair the missing comma in "value + whitespace + quoted key": e.g. `#fff "stroke"` or `100 "w"`
    // Match the ASCII pattern byte by byte (whitespace/quote/letters), but multi-byte UTF-8 characters
    // such as CJK must be copied as original bytes — the old `bytes[i] as char` treated each byte as a
    // Latin-1 code point, then re-encoded as UTF-8, turning 3-byte CJK into 6-byte mojibake (R-A9).
    // The pattern only involves ASCII; non-leading bytes (>=0x80) never match, so copying advances
    // along UTF-8 character boundaries.
    let bytes = out.as_bytes();
    let mut rebuilt = String::with_capacity(out.len());
    let mut i = 0;
    while i < bytes.len() {
        // Detect pattern: whitespace (space/tab/newline; indented multi-line output hits too) + `"` + ASCII letter
        // + ... + `":`, with the previous non-space character not in `, : [ {`
        if (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n' || bytes[i] == b'\r')
            && i + 3 < bytes.len()
            && bytes[i + 1] == b'"'
            && bytes[i + 2].is_ascii_alphabetic()
        {
            // Found the matching `":` terminator
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
        // Copy original bytes along UTF-8 character boundaries: ASCII is 1 byte; multi-byte characters are pushed whole.
        let char_len = utf8_char_len(bytes[i]);
        let end = (i + char_len).min(bytes.len());
        if let Ok(slice) = std::str::from_utf8(&bytes[i..end]) {
            rebuilt.push_str(slice);
        } else {
            // Broken multi-byte sequence (input itself invalid); skip it whole to stop mojibake from spreading.
            tracing::warn!(
                at = i,
                len = char_len,
                "repair_model_json skipping invalid UTF-8 sequence"
            );
        }
        i = end;
    }
    // Array element missing `{` (the `},"id":` shape) + trailing comma: defect class observed on 4B models.
    // Must run before balance_json_braces: an orphan element's closing `}` would trigger balance's
    // negative-depth truncation; inserting the open brace first preserves the following content.
    // Value-level garbage is repaired first: dangling quotes/garbage segments create fake string states
    // during the structural scan; clean them up before the element-open-brace/trailing-comma structural fixes.
    rebuilt = fix_value_garbage(&rebuilt);
    rebuilt = fix_element_open_brace(&rebuilt);
    // Unbalanced bracket repair: 7B models often emit trailing extra `}` or get truncated midway.
    balance_json_braces(&mut rebuilt);
    rebuilt
}

/// Determines the UTF-8 character byte length (1..=4) from the leading byte. Invalid/continuation bytes return 1 (skipped byte by byte).
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

/// Balances curly/square brackets per JSON semantics (skipping string literals and escapes).
/// - If depth goes negative during the scan (extra `}`/`]`), the remaining content is truncated;
/// - If depth is still > 0 at the end, the missing closing brackets are appended in reverse stack order.
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
        tracing::info!(
            pos,
            "balance_json_braces: truncated trailing extra closing brackets"
        );
        return;
    }
    if !stack.is_empty() {
        let mut tail = String::new();
        while let Some(b) = stack.pop() {
            tail.push(if b == b'{' { '}' } else { ']' });
        }
        tracing::info!(
            missing = tail.len(),
            "balance_json_braces: appended missing closing brackets"
        );
        s.push_str(&tail);
    }
}

/// Repairs array elements missing their open brace (defect class observed on 4B models): `},"id":` → `},{"id":`.
/// The model omits an element's `{` in the nodes array while its closing `}` exists — inserting the open brace rebalances it.
/// Companion trailing-comma removal: a `,` immediately followed by `]`/`}`/EOF is deleted (dangling trailing comma in model output).
/// Only runs on the repair path after strict parsing fails; valid JSON never enters this function.
fn fix_element_open_brace(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + 16);
    let mut stack: Vec<u8> = Vec::new();
    let mut in_str = false;
    let mut escape = false;
    // In array context: set when `,` immediately follows `}`; if the next non-space char is `"`, insert `{`.
    let mut pending_elem = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let char_len = utf8_char_len(b);
        let end = (i + char_len).min(bytes.len());
        let slice = match std::str::from_utf8(&bytes[i..end]) {
            Ok(v) => v,
            Err(_) => {
                i = end;
                continue;
            }
        };
        if in_str {
            out.push_str(slice);
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            i = end;
            continue;
        }
        match b {
            b'"' => {
                if pending_elem {
                    // Array element starting with a string key = missing `{`; insert the open brace and push it.
                    out.push('{');
                    stack.push(b'{');
                    pending_elem = false;
                    tracing::info!("fix_element_open_brace: inserted missing element open brace");
                }
                in_str = true;
                out.push_str(slice);
            }
            b'{' | b'[' => {
                pending_elem = false;
                stack.push(b);
                out.push_str(slice);
            }
            b'}' | b']' => {
                pending_elem = false;
                let opener = if b == b'}' { b'{' } else { b'[' };
                if stack.last() == Some(&opener) {
                    stack.pop();
                }
                // Trailing-comma removal: if the closest non-space char before the closing bracket is `,`, delete it (with trailing whitespace).
                let trimmed = out.trim_end();
                if trimmed.ends_with(',') {
                    let keep = trimmed.len() - 1;
                    out.truncate(keep);
                    tracing::info!(
                        "fix_element_open_brace: removed trailing comma before closing bracket"
                    );
                }
                out.push_str(slice);
            }
            b',' => {
                // Treat as an element boundary only in array context when the previous non-space char is `}`.
                let prev_ok = out.trim_end().ends_with('}');
                pending_elem = stack.last() == Some(&b'[') && prev_ok;
                out.push_str(slice);
            }
            b' ' | b'\t' | b'\n' | b'\r' => {
                out.push_str(slice);
            }
            _ => {
                pending_elem = false;
                out.push_str(slice);
            }
        }
        i = end;
    }
    // EOF trailing-comma removal.
    let trimmed = out.trim_end();
    if trimmed.ends_with(',') {
        out.truncate(trimmed.len() - 1);
    }
    out
}

/// Value-level garbage repair (defect class observed on vision models; repair path only, valid JSON never enters this function):
/// - Dangling quote after a value: `"x": 0",` → `"x": 0,` (an extra quote after a number/string value's closing quote)
/// - Duplicate number: `"h": 60 16,` → `"h": 60,` (number-whitespace-number cannot occur in valid JSON values)
/// - Garbage segment after a string's closing quote: `"# "#6b4e9a",` → `"# ",` (delete up to and including the next quote)
fn fix_value_garbage(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut escape = false;
    let mut stack: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let char_len = utf8_char_len(b);
        let end = (i + char_len).min(bytes.len());
        let slice = match std::str::from_utf8(&bytes[i..end]) {
            Ok(v) => v,
            Err(_) => {
                i = end;
                continue;
            }
        };
        if in_str {
            out.push_str(slice);
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            i = end;
            continue;
        }
        if b == b'{' || b == b'[' {
            stack.push(b);
            out.push_str(slice);
            i = end;
            continue;
        }
        if b == b'}' || b == b']' {
            let opener = if b == b'}' { b'{' } else { b'[' };
            if stack.last() == Some(&opener) {
                stack.pop();
            }
            out.push_str(slice);
            i = end;
            continue;
        }
        // Orphan string in key position: a string after `,`/`{` inside an object whose following content (skipping spaces)
        // is not `:` is a duplicated-value artifact (e.g. `"left","left",`); delete the string and its trailing comma.
        // Not applicable in array context (string array elements are valid).
        if b == b'"' && stack.last() == Some(&b'{') {
            let prev = out.trim_end().chars().last();
            if matches!(prev, Some('{') | Some(',')) {
                if let Some(close) = find_closing_quote(bytes, i) {
                    let mut j = close + 1;
                    while j < bytes.len()
                        && (bytes[j] == b' '
                            || bytes[j] == b'\t'
                            || bytes[j] == b'\n'
                            || bytes[j] == b'\r')
                    {
                        j += 1;
                    }
                    let after = bytes.get(j).copied();
                    if !matches!(after, Some(b':')) {
                        tracing::info!("fix_value_garbage: removed orphan string in key position");
                        // Skip the string; if a comma follows, skip it too.
                        i = if after == Some(b',') {
                            j + 1
                        } else {
                            close + 1
                        };
                        continue;
                    }
                }
            }
        }
        // Quote outside a string: determine whether it is a dangling quote after a value.
        if b == b'"' {
            let prev = out.trim_end().chars().last();
            let prev_is_value_end = matches!(prev, Some(c) if c.is_ascii_digit() || c == '"');
            if prev_is_value_end {
                // Look ahead (skipping spaces) at the first char: comma/closing bracket/EOF → dangling quote, delete it.
                let mut j = i + 1;
                while j < bytes.len()
                    && (bytes[j] == b' '
                        || bytes[j] == b'\t'
                        || bytes[j] == b'\n'
                        || bytes[j] == b'\r')
                {
                    j += 1;
                }
                let next = bytes.get(j).copied();
                if matches!(next, None | Some(b',') | Some(b'}') | Some(b']')) {
                    tracing::info!("fix_value_garbage: removed dangling quote after value");
                    i = end;
                    continue;
                }
            }
            in_str = true;
            out.push_str(slice);
            i = end;
            continue;
        }
        // Number outside a string: whitespace between it and the previous number token → duplicate number; skip the whole digit run.
        // Note this requires out to end with whitespace — digits within the same token (e.g. the last
        // two digits of 100) have no whitespace before them and must not be mistakenly deleted.
        if b.is_ascii_digit() && out.ends_with(|c: char| c.is_whitespace()) {
            let trimmed_prev = out.trim_end().chars().last();
            if matches!(trimmed_prev, Some(c) if c.is_ascii_digit()) {
                let mut j = i;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
                    j += 1;
                }
                tracing::info!("fix_value_garbage: removed duplicate number segment");
                i = j;
                continue;
            }
        }
        // Garbage character outside a string (previous non-space char is a closing quote): delete up to the next quote (inclusive) or comma (exclusive).
        if !b.is_ascii_whitespace()
            && b != b','
            && b != b':'
            && b != b'{'
            && b != b'}'
            && b != b'['
            && b != b']'
        {
            let prev = out.trim_end().chars().last();
            if prev == Some('"') {
                let mut j = i;
                let mut cut = None;
                while j < bytes.len() {
                    if bytes[j] == b'"' {
                        cut = Some(j + 1);
                        break;
                    }
                    if bytes[j] == b',' {
                        cut = Some(j);
                        break;
                    }
                    j += 1;
                }
                if let Some(cut) = cut {
                    tracing::info!("fix_value_garbage: removed garbage segment after string");
                    i = cut;
                    continue;
                }
            }
        }
        out.push_str(slice);
        i = end;
    }
    out
}

/// Node-level salvage: cuts the top-level `{...}` chunks out of the nodes array in bad output, parses each independently,
/// keeps valid chunks and drops corrupted ones. ≥1 valid node produces a document (restores draft semantics).
fn salvage_pen_document(s: &str) -> Option<PenDocument> {
    let bytes = s.as_bytes();
    // Locate the nodes array's opening `[` (tolerates the `"nodes": [` space variant).
    let nodes_key = s.find("\"nodes\"")?;
    let arr_start = bytes[nodes_key..].iter().position(|&b| b == b'[')? + nodes_key;
    // String-aware scan, collecting top-level object chunks inside the array.
    let mut chunks: Vec<&str> = Vec::new();
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut escape = false;
    let mut obj_start: Option<usize> = None;
    let mut i = arr_start + 1;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => {
                if depth == 0 {
                    obj_start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(st) = obj_start {
                        chunks.push(&s[st..=i]);
                    }
                    obj_start = None;
                }
            }
            b']' if depth == 0 => break,
            _ => {}
        }
        i += 1;
    }
    let mut good: Vec<serde_json::Value> = Vec::new();
    for c in &chunks {
        match serde_json::from_str::<serde_json::Value>(c) {
            Ok(v) => good.push(v),
            Err(e) => tracing::warn!(error = %e, "salvage: dropped corrupted node chunk"),
        }
    }
    if good.is_empty() {
        tracing::warn!(chunks = chunks.len(), "salvage: no salvageable nodes");
        return None;
    }
    // Page dimensions: scan for the nearest `"width":<num>` / `"height":<num>` (defaults 1440×900).
    let w = scan_num_after_key(s, "width").unwrap_or(1440.0);
    let h = scan_num_after_key(s, "height").unwrap_or(900.0);
    let nodes = match parse_nodes_with_depth(&serde_json::Value::Array(good), 0) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, "salvage: salvaged node schema parse failed");
            return None;
        }
    };
    let mut doc = PenDocument::new();
    let mut page = Page::new("page_1", "salvaged", w, h);
    for n in nodes {
        page.add(n);
    }
    tracing::info!(
        nodes = page.nodes.len(),
        "salvage: node-level salvage produced document"
    );
    doc.add_page(page);
    Some(doc)
}

/// Finds the first number after `"key":` in the text (tolerates spaces after the colon / quote wrapping).
fn scan_num_after_key(s: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\":");
    let pos = s.find(&pat)?;
    let rest = &s[pos + pat.len()..];
    let mut num = String::new();
    for c in rest.chars() {
        if c.is_ascii_digit() || c == '.' || c == '-' {
            num.push(c);
        } else if !num.is_empty() {
            break;
        } else if c.is_whitespace() || c == '"' {
            continue;
        } else {
            return None;
        }
    }
    num.parse::<f64>().ok()
}

/// Finds the matching closing quote from an opening quote byte position (handles `\\` escapes). Returns the closing quote index.
fn find_closing_quote(bytes: &[u8], open: usize) -> Option<usize> {
    let mut j = open + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'"' => return Some(j),
            _ => j += 1,
        }
    }
    None
}

const MAX_NODE_DEPTH: usize = 20;

pub fn parse_nodes_with_depth(v: &serde_json::Value, depth: usize) -> anyhow::Result<Vec<PenNode>> {
    if depth > MAX_NODE_DEPTH {
        anyhow::bail!("node nesting depth exceeds {MAX_NODE_DEPTH}, refusing to parse");
    }
    let arr = v
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("nodes is not an array"))?;
    let mut nodes = Vec::with_capacity(arr.len());
    // E-6/P3: after sanitize_node_id filtering, distinct original ids may normalize to the same value
    // ("a-b!" and "a-b" → "a-b"); sibling id collisions break later node operations. A per-array seen
    // set appends a _<n> suffix to colliding ids for deduplication.
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
            tracing::warn!(original = %original, new = %node.id, "sanitize_node_id collision, appended suffix to deduplicate");
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
        .ok_or_else(|| anyhow::anyhow!("node {idx} is not an object"))?;
    let kind_str = o.get("kind").and_then(|k| k.as_str()).unwrap_or("rect");
    // kind is case-insensitive: document serialization is PascalCase ("Text"), and the model echoes
    // input JSON with the original casing (observed in issue#34 region iteration); the parser must
    // align with the serializer, otherwise echoing valid input would always fail.
    let kind = match kind_str.to_ascii_lowercase().as_str() {
        "rect" => NodeKind::Rect,
        "circle" => NodeKind::Circle,
        "text" => NodeKind::Text,
        "image" => NodeKind::Image,
        "group" => NodeKind::Group,
        other => anyhow::bail!("unknown node kind {other:?}"),
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
        tracing::warn!(
            "node {} w={:.1} is non-positive, corrected to 1.0",
            node.id,
            node.w
        );
        node.w = 1.0;
    }
    if node.h <= 0.0 {
        tracing::warn!(
            "node {} h={:.1} is non-positive, corrected to 1.0",
            node.id,
            node.h
        );
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
