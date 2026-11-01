//! fd-skills 测试：迁自 fd-ai-adapter skills_tests（ARCH-11）。
//!
//! MockSkillContext 替代 FusionMlxClient，验证 skill trait 行为无需真实推理。
//! 纯 helper 测试（parse_ui_json/repair_model_json/strip_code_fence/parse_*）
//! 直接调用 pub fn。skill struct 用 ::new(model) 构造（Ruling #16）。

use fd_canvas_core::PenDocument;
use fd_design_system::DesignSystem;
use fd_skills::{
    parse_flow_pages, parse_local_edit_input, parse_node_with_depth, parse_page_flow_input,
    parse_spec_doc_input, parse_spec_doc_json, parse_ui_json, repair_model_json, strip_code_fence,
    DesignSkill, ImageToUiSkill, LocalEditSkill, MultiVariantsSkill, PageFlowSkill,
    PartialEditSkill, SkillContext, SkillOutput, SkillRegistry, SpecDocSkill, TextToUiSkill,
};
use std::sync::{Arc, Mutex};

// ── MockSkillContext：记录 chat 调用，返回预置响应 ──

struct MockSkillContext {
    responses: Arc<Mutex<Vec<String>>>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl MockSkillContext {
    fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn next_response(&self, user: &str) -> anyhow::Result<String> {
        self.calls.lock().unwrap().push(user.to_string());
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            anyhow::bail!("MockSkillContext: 响应队列耗尽");
        }
        Ok(responses.remove(0))
    }
}

impl SkillContext for MockSkillContext {
    fn chat(
        &self,
        _model: &str,
        _sys: &str,
        user: &str,
        _max_tokens: u32,
    ) -> anyhow::Result<String> {
        self.next_response(user)
    }

    fn chat_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>
    {
        Box::pin(async move { self.chat(model, sys, user, max_tokens) })
    }

    fn chat_with_image_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        _image_base64: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>
    {
        Box::pin(async move { self.chat(model, sys, user, max_tokens) })
    }

    fn chat_with_image(
        &self,
        model: &str,
        sys: &str,
        user: &str,
        _image_base64: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        self.chat(model, sys, user, max_tokens)
    }

    fn token_prompt_fragment(&self, design_system: Option<&DesignSystem>) -> Option<String> {
        design_system.map(|ds| {
            let css = ds.to_css_custom_properties();
            format!("当前设计系统 Token（CSS Custom Properties）：\n{css}\n\n生成的 UI 必须使用这些 CSS 变量。")
        })
    }
}

// ── 纯 helper 测试 ──

fn fake_doc_json() -> String {
    let fill_a = String::from("#") + "FFF";
    let fill_b = String::from("#") + "000";
    format!(
        r#"{{"page":{{"width":100,"height":100,"nodes":[{{"id":"a","kind":"rect","x":0,"y":0,"w":50,"h":50,"fill":"{fill_a}"}},{{"id":"b","kind":"text","x":10,"y":20,"text":"hi","fill":"{fill_b}"}}]}}}}"#,
    )
}

#[test]
fn parse_ui_json_full_shape() {
    let doc = parse_ui_json(&fake_doc_json(), "Test").unwrap();
    assert_eq!(doc.pages.len(), 1);
    let page = &doc.pages[0];
    assert_eq!(page.width, 100.0);
    assert_eq!(page.nodes.len(), 2);
    assert_eq!(page.nodes[0].id, "a");
    assert_eq!(page.nodes[1].kind, fd_canvas_core::NodeKind::Text);
}

#[test]
fn parse_ui_json_strips_code_fence() {
    let fenced = r##"```json
{"page":{"width":1,"height":1,"nodes":[]}}
```"##;
    let doc = parse_ui_json(fenced, "F").unwrap();
    assert_eq!(doc.pages[0].width, 1.0);
}

#[test]
fn parse_ui_json_naked_nodes() {
    let naked = r#"{"nodes":[{"id":"x","kind":"circle","x":0,"y":0,"w":10,"h":10}]}"#;
    let doc = parse_ui_json(naked, "N").unwrap();
    assert_eq!(doc.pages[0].nodes[0].kind, fd_canvas_core::NodeKind::Circle);
}

#[test]
fn parse_ui_json_missing_nodes_empty_page() {
    let doc = parse_ui_json(r#"{"page":{}}"#, "x").unwrap();
    assert_eq!(doc.pages.len(), 1);
    assert!(doc.pages[0].nodes.is_empty());
}

#[test]
fn parse_ui_json_unknown_kind_bails() {
    let bad = r#"{"nodes":[{"id":"x","kind":"weird"}]}"#;
    assert!(parse_ui_json(bad, "x").is_err());
}

#[test]
fn parse_ui_json_invalid_json_bails() {
    assert!(parse_ui_json("not json", "x").is_err());
}

#[test]
fn repair_model_json_fixes_double_colon() {
    let broken = "{\"fill\":\"\":\"#ffffff\"}";
    assert_eq!(repair_model_json(broken), "{\"fill\":\"#ffffff\"}");
}

#[test]
fn repair_model_json_fixes_missing_comma() {
    let broken = "{\"y\":100 \"w\":400}";
    let repaired = repair_model_json(broken);
    assert!(
        repaired.contains("100,\"w\"") || repaired.contains("100, \"w\""),
        "repaired={repaired}"
    );
}

#[test]
fn repair_model_json_trims_trailing_extra_brace() {
    let broken = "{\"page\":{\"width\":10}}}";
    let repaired = repair_model_json(broken);
    let v: serde_json::Value = serde_json::from_str(&repaired).expect("repaired parses");
    assert_eq!(v["page"]["width"], 10);
}

#[test]
fn repair_model_json_completes_truncated_object() {
    let broken = "{\"page\":{\"nodes\":[{\"id\":\"n0\",\"kind\":\"rect\"";
    let repaired = repair_model_json(broken);
    let v: serde_json::Value = serde_json::from_str(&repaired).expect("repaired parses");
    assert_eq!(v["page"]["nodes"][0]["id"], "n0");
}

#[test]
fn repair_model_json_preserves_cjk_no_mojibake() {
    let broken = "{\"label\":\"登录\" \"w\":400}";
    let repaired = repair_model_json(broken);
    assert!(repaired.contains("登录"), "CJK 被乱码: {repaired}");
    assert!(
        repaired.contains("\"登录\"")
            || repaired.contains("\"登录\",")
            || repaired.contains("\"登录\", \"w\""),
        "repaired={repaired}"
    );
    let v: serde_json::Value = serde_json::from_str(&repaired).expect("repaired parses");
    assert_eq!(v["label"], "登录");
    assert_eq!(v["w"], 400);
}

#[test]
fn repair_model_json_cjk_value_after_space_no_corruption() {
    let broken = "{\"name\":\"按钮\" \"type\":\"rect\"}";
    let repaired = repair_model_json(broken);
    assert!(repaired.contains("按钮"), "CJK 被乱码: {repaired}");
    let v: serde_json::Value = serde_json::from_str(&repaired).expect("repaired parses");
    assert_eq!(v["name"], "按钮");
    assert_eq!(v["type"], "rect");
}

#[test]
fn strip_code_fence_plain_passthrough() {
    assert_eq!(strip_code_fence("  hi  "), "hi");
}

#[test]
fn parse_node_defaults_id_when_absent() {
    let v: serde_json::Value = serde_json::from_str(r#"{"kind":"rect"}"#).unwrap();
    let n = parse_node_with_depth(&v, 5, 0).unwrap();
    assert_eq!(n.id, "n_5");
}

// ── SkillRegistry 测试（::new(model) 构造）──

#[test]
fn skill_registry_register_and_list() {
    let mut reg = SkillRegistry::new();
    reg.register(Box::new(TextToUiSkill::new("qwen3.5")));
    reg.register(Box::new(PartialEditSkill::new("qwen3.5")));
    let ids = reg.list();
    assert!(ids.contains(&"text-to-ui"));
    assert!(ids.contains(&"partial-edit"));
}

#[test]
fn skill_registry_get_found() {
    let mut reg = SkillRegistry::new();
    reg.register(Box::new(TextToUiSkill::new("qwen3.5")));
    let skill = reg.get("text-to-ui").unwrap();
    assert_eq!(skill.id(), "text-to-ui");
    assert_eq!(skill.label(), "文生 UI");
}

#[test]
fn skill_registry_get_missing() {
    let reg = SkillRegistry::new();
    assert!(reg.get("nope").is_none());
}

#[test]
fn skill_registry_builtin_registers_seven() {
    let mut reg = SkillRegistry::new();
    reg.register_builtin("qwen3.5");
    let ids = reg.list();
    assert_eq!(ids.len(), 7);
    assert!(ids.contains(&"text-to-ui"));
    assert!(ids.contains(&"image-to-ui"));
    assert!(ids.contains(&"partial-edit"));
    assert!(ids.contains(&"local-edit"));
    assert!(ids.contains(&"multi-variants"));
    assert!(ids.contains(&"spec-doc"));
    assert!(ids.contains(&"page-flow"));
}

// ── SkillContext trait 测试（MockSkillContext）──

#[test]
fn skill_context_token_prompt_without_design_system() {
    let ctx = MockSkillContext::new(vec![]);
    assert!(ctx.token_prompt_fragment(None).is_none());
}

#[test]
fn skill_context_token_prompt_with_design_system() {
    let ctx = MockSkillContext::new(vec![]);
    let ds = fd_design_system::builtin_apple_hig();
    let frag = ctx.token_prompt_fragment(Some(&ds)).unwrap();
    assert!(frag.contains("--color-accent"));
    assert!(frag.contains("CSS Custom Properties"));
}

// ── SkillOutput variant 测试 ──

#[test]
fn skill_output_variant_document() {
    let doc = PenDocument::new();
    let out = SkillOutput::Document(doc);
    match out {
        SkillOutput::Document(d) => assert_eq!(d.pages.len(), 0),
        _ => panic!("期望 Document"),
    }
}

#[test]
fn skill_output_variant_partial_edit() {
    let out = SkillOutput::PartialEdit("modified".into());
    match out {
        SkillOutput::PartialEdit(s) => assert_eq!(s, "modified"),
        _ => panic!("期望 PartialEdit"),
    }
}

// ── parse_local_edit_input ──

#[test]
fn parse_local_edit_input_with_triple_pipe() {
    let (nodes, instr) = parse_local_edit_input("[{\"id\":\"a\"}]|||改成红色");
    assert_eq!(nodes, "[{\"id\":\"a\"}]");
    assert_eq!(instr, "改成红色");
}

#[test]
fn parse_local_edit_input_with_single_pipe() {
    let (nodes, instr) = parse_local_edit_input("[{\"id\":\"a\"}]|修改样式");
    assert_eq!(nodes, "[{\"id\":\"a\"}]");
    assert_eq!(instr, "修改样式");
}

#[test]
fn parse_local_edit_input_no_instruction() {
    let (nodes, instr) = parse_local_edit_input("[{\"id\":\"a\"}]");
    assert_eq!(nodes, "[{\"id\":\"a\"}]");
    assert_eq!(instr, "修改为更合适的样式");
}

// ── skill id/label（::new(model) 构造）──

#[test]
fn local_edit_skill_id_and_label() {
    let skill = LocalEditSkill::new("qwen3.5");
    assert_eq!(skill.id(), "local-edit");
    assert_eq!(skill.label(), "本地编辑");
}

#[test]
fn spec_doc_skill_id_and_label() {
    let skill = SpecDocSkill::new("qwen3.5");
    assert_eq!(skill.id(), "spec-doc");
    assert_eq!(skill.label(), "设计规范文档");
}

#[test]
fn page_flow_skill_id_and_label() {
    let skill = PageFlowSkill::new("qwen3.5");
    assert_eq!(skill.id(), "page-flow");
    assert_eq!(skill.label(), "页面流程生成");
}

#[test]
fn text_to_ui_skill_id_and_label() {
    let skill = TextToUiSkill::new("qwen3.5");
    assert_eq!(skill.id(), "text-to-ui");
    assert_eq!(skill.label(), "文生 UI");
}

#[test]
fn image_to_ui_skill_id_and_label() {
    let skill = ImageToUiSkill::new("qwen3.5");
    assert_eq!(skill.id(), "image-to-ui");
    assert_eq!(skill.label(), "图生 UI");
}

#[test]
fn partial_edit_skill_id_and_label() {
    let skill = PartialEditSkill::new("qwen3.5");
    assert_eq!(skill.id(), "partial-edit");
    assert_eq!(skill.label(), "局部编辑");
}

#[test]
fn multi_variants_skill_id_and_label() {
    let skill = MultiVariantsSkill::new("qwen3.5");
    assert_eq!(skill.id(), "multi-variants");
    assert_eq!(skill.label(), "多方案对比");
}

// ── parse_spec_doc_input / parse_spec_doc_json ──

#[test]
fn parse_spec_doc_input_with_title() {
    let (doc, title) = parse_spec_doc_input("{\"pages\":[]}|登录页规范");
    assert_eq!(doc, "{\"pages\":[]}");
    assert_eq!(title, "登录页规范");
}

#[test]
fn parse_spec_doc_input_default_title() {
    let (doc, title) = parse_spec_doc_input("{\"pages\":[]}");
    assert_eq!(doc, "{\"pages\":[]}");
    assert_eq!(title, "设计规范文档");
}

#[test]
fn parse_spec_doc_json_full() {
    let json = r#"{"title":"登录页规范","page_architecture":"单页居中布局","interaction_specs":[{"id":"i1","element":"登录按钮","event":"click","behavior":"提交表单","animation":"fade-in","notes":"防重复点击"}],"component_specs":[{"id":"c1","name":"LoginButton","kind":"button","props":[{"name":"disabled","prop_type":"boolean","default_value":"false","description":"是否禁用"}],"variants":["primary","ghost"],"accessibility":"aria-label=登录"}],"token_summary":"主色蓝 #007AFF，圆角 8px"}"#;
    let spec = parse_spec_doc_json(json, "fallback").unwrap();
    assert_eq!(spec.title, "登录页规范");
    assert_eq!(spec.interaction_specs.len(), 1);
    assert_eq!(spec.interaction_specs[0].element, "登录按钮");
    assert_eq!(spec.component_specs.len(), 1);
    assert_eq!(spec.component_specs[0].props.len(), 1);
    assert_eq!(spec.component_specs[0].props[0].name, "disabled");
}

#[test]
fn parse_spec_doc_json_minimal() {
    let json = "{}";
    let spec = parse_spec_doc_json(json, "默认标题").unwrap();
    assert_eq!(spec.title, "默认标题");
    assert!(spec.interaction_specs.is_empty());
    assert!(spec.component_specs.is_empty());
}

// ── parse_page_flow_input / parse_flow_pages ──

#[test]
fn parse_page_flow_input_with_style() {
    let (desc, style) = parse_page_flow_input("电商:首页,列表,详情|Material");
    assert_eq!(desc, "电商:首页,列表,详情");
    assert_eq!(style, "Material");
}

#[test]
fn parse_page_flow_input_default_style() {
    let (desc, style) = parse_page_flow_input("电商:首页,列表,详情");
    assert_eq!(desc, "电商:首页,列表,详情");
    assert_eq!(style, "简约");
}

#[test]
fn parse_flow_pages_with_colon() {
    let pages = parse_flow_pages("电商:首页,商品列表,商品详情,购物车,结算");
    assert_eq!(
        pages,
        vec!["首页", "商品列表", "商品详情", "购物车", "结算"]
    );
}

#[test]
fn parse_flow_pages_without_colon() {
    let pages = parse_flow_pages("首页,列表,详情");
    assert_eq!(pages, vec!["首页", "列表", "详情"]);
}

#[test]
fn parse_flow_pages_empty() {
    let pages = parse_flow_pages("");
    assert_eq!(pages, vec!["首页"]);
}

// ── mock 端到端：TextToUiSkill 用 MockSkillContext 验证 prompt 构造 ──

#[test]
fn text_to_ui_skill_prompt_constructed() {
    let fake_resp = r#"{"page":{"width":800,"height":600,"nodes":[{"id":"n0","kind":"rect","x":0,"y":0,"w":100,"h":50}]}}"#;
    let ctx = MockSkillContext::new(vec![fake_resp.to_string()]);
    let skill = TextToUiSkill::new("qwen3.5");
    let out = skill.execute(&ctx, None, "登录页|登录").unwrap();
    match out {
        SkillOutput::Document(doc) => {
            assert_eq!(doc.pages.len(), 1);
            assert_eq!(doc.pages[0].nodes.len(), 1);
            assert_eq!(doc.pages[0].nodes[0].id, "n0");
        }
        _ => panic!("期望 Document"),
    }
    let calls = ctx.calls.lock().unwrap();
    assert!(
        calls[0].contains("登录页"),
        "user prompt 应包含 page_name: {}",
        calls[0]
    );
}

#[test]
fn repair_model_json_fixes_missing_element_open_brace() {
    // 4B 模型实测缺陷类：nodes 数组元素漏写 `{`，闭 `}` 仍在。
    let broken = r#"{"page":{"width":1440,"height":900,"nodes":[{"id":"g1","kind":"group","x":0,"y":0,"w":100,"h":100},"id":"t1","kind":"text","x":10,"y":10,"w":50,"h":20,"text":"欢迎登录"}]}},"#;
    let repaired = repair_model_json(broken);
    let v: serde_json::Value = serde_json::from_str(&repaired).expect("repaired parses");
    let nodes = v["page"]["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 2, "两个元素都须保留: {repaired}");
    assert_eq!(nodes[1]["id"], "t1");
    assert_eq!(nodes[1]["text"], "欢迎登录");
}

#[test]
fn repair_model_json_strips_trailing_comma_at_eof() {
    let broken = r#"{"page":{"width":10,"height":20,"nodes":[]}},"#;
    let repaired = repair_model_json(broken);
    let v: serde_json::Value = serde_json::from_str(&repaired).expect("repaired parses");
    assert_eq!(v["page"]["width"], 10);
}

#[test]
fn parse_ui_json_recovers_missing_element_open_brace() {
    // 端到端：坏输出经修复链后应成功产出 PenDocument（fail visibly 不触发）。
    let broken = r#"{"page":{"width":1440,"height":900,"nodes":[{"id":"g1","kind":"group","x":0,"y":0,"w":100,"h":100},"id":"t1","kind":"text","x":10,"y":10,"w":50,"h":20,"text":"标题"}]}}"#;
    let doc = parse_ui_json(broken, "page-1").expect("修复链应兜住漏开括号");
    let page = doc.pages.first().expect("page exists");
    assert_eq!(page.nodes.len(), 2);
    assert!(page.nodes.iter().any(|n| n.id == "t1"));
}

#[test]
fn repair_model_json_fixes_vision_value_garbage() {
    // 视觉模型实测缺陷类：悬空引号 / 重复数字 / 字符串后垃圾段 / 尾逗号。
    let broken = r##"{
  "page": {
    "width": 1480,
    "nodes": [
      {"id": "n0", "kind": "rect", "x": 0", "y": 60", "w": 480, "h": 60 16, "fill": "# "#6b4e9a",}
    ]
  }
"##;
    let repaired = repair_model_json(broken);
    let v: serde_json::Value = serde_json::from_str(&repaired).expect("repaired parses");
    let n = &v["page"]["nodes"][0];
    assert_eq!(n["x"], 0, "悬空引号删除后 x=0: {repaired}");
    assert_eq!(n["y"], 60, "悬空引号删除后 y=60");
    assert_eq!(n["h"], 60, "重复数字删除后 h=60");
    assert_eq!(n["fill"], "# ", "垃圾段删除后 fill 可解析");
}

#[test]
fn repair_model_json_keeps_multi_digit_tokens() {
    // 回归护栏：数字 token 内部（100/400/16.5）不可被重复数字规则误删。
    let ok = r#"{"page":{"width":1440,"height":900,"nodes":[{"id":"n0","kind":"rect","x":100,"y":400,"w":16.5,"h":900}]}}"#;
    let v: serde_json::Value = serde_json::from_str(&repair_model_json(ok)).expect("parses");
    assert_eq!(v["page"]["nodes"][0]["x"], 100);
    assert_eq!(v["page"]["nodes"][0]["y"], 400);
    assert_eq!(v["page"]["nodes"][0]["w"], 16.5);
}

#[test]
fn parse_ui_json_recovers_vl7b_output() {
    // Qwen2.5-VL-7B 实测输出（登录页截图还原）：键位孤儿串/双冒号等缺陷，
    // 修复链应兜住并产出完整 PenDocument。
    let raw = r##"{"page":{"width":1440,"height":900,"nodes":[{"id":"1","kind":"group","x":280,"y":150,"w":980,"h":600,"text":"","fill":"#f9f9f99","stroke":"#f9f999"},{"id":"2","kind":"text","x":412,"y":189,"w":320,"h":50,"text":"User Login","text-align":"center","font-family":"Arial","font-size":30,"font-weight":"bold"},{"id":"3","kind":"":"text","x":412,"y":229,"w":400,"h":50,"text":"Sign in to continue","text-align":"center","font-family":"Arial","font-size":15,"font-weight":"normal"},{"id":"4","kind":"image","x":412,"y":280,"w":400,"h":50,"text":"","fill":"#f9f9f9","stroke":"#f9f9f9","children":[{"id":"5","kind":"text","x":412,"y":280,"w":320,"h":50,"text":"Email address","text-align":"left","font-family":"Arial","font-size":15,"font-weight":"normal"},{"id":"6","kind":"text","x":412,"y":330,"w":3200,"h":50","text":"Password","text-align":"left","left","font-family-family":"Arial","Arial","font-size":15015,"font-weight":"normal"},{"id":"7","kind":"image","x":41412,"y":380,"w":4000,"h":50,"text":"","fill":"#f9f9f999","stroke":"#f9999","children":[{"id":"8","kind":"text","":"text","x":411412,"y":380,"w400":"0h":5050,"text":"Sign In","text-align":"center","font-family":"Arial","":"Arial","font-size":150,"":"font-weight":"bold"},{"id":"9","kind":"image","x":41412,"y":"0w":"0h50":"50"text":"","fill":"#f99","stroke":"#999","children":[{"id":"10","kind":"text","x412":"y380":"w400":"h50":"50"text":"Forgot password? Create account","text-align":"left","font-family":"Arial","font-size":15,"font-weight":"normal"},{"id":"11","kind":"image","x412y":"380w400":"h500":"50"text":"","fill":"#f99999","stroke":"#99999","children":[{"id":"12","kind":"text","":"text","x412y380w4000h50"text":"Remember me","text-align":"left","font-family-familyArial":"Arialfont-size15":"15font-weight":"normal"}]}]}]}}"##;
    let doc = parse_ui_json(raw, "restored").expect("VL 输出修复链应兜住");
    let page = doc.pages.first().expect("page exists");
    assert!(!page.nodes.is_empty(), "节点不可为空");
    assert!(page
        .nodes
        .iter()
        .any(|n| n.text.as_deref() == Some("User Login")));
}
