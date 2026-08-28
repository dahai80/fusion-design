use fd_canvas_core::*;

fn doc_with_nodes(ids: &[&str]) -> PenDocument {
    let mut page = Page {
        id: "p0".into(),
        name: "P0".into(),
        width: 100.0,
        height: 100.0,
        nodes: vec![],
    };
    for id in ids {
        page.nodes.push(PenNode {
            id: (*id).into(),
            kind: NodeKind::Rect,
            name: (*id).into(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            style: Default::default(),
            text: None,
            children: vec![],
            rotation: 0.0,
            z_index: 0,
        });
    }
    PenDocument {
        schema_version: 1,
        pages: vec![page],
        variables: None,
        active_design_system: None,
    }
}

fn node_ids(doc: &PenDocument) -> Vec<String> {
    doc.pages
        .first()
        .map(|p| p.nodes.iter().map(|n| n.id.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn undo_reverses_add() {
    let mut stack = UndoRedoStack::new(doc_with_nodes(&["a"]));
    let new_doc = doc_with_nodes(&["a", "b"]);
    let delta = UndoDelta::compute(stack.current(), &new_doc);
    stack.push(delta);
    assert_eq!(node_ids(stack.current()), vec!["a".to_string(), "b".into()]);
    let undone = stack.undo().unwrap();
    assert_eq!(node_ids(&undone), vec!["a".to_string()]);
}

#[test]
fn undo_reverses_delete() {
    let mut stack = UndoRedoStack::new(doc_with_nodes(&["a", "b"]));
    let new_doc = doc_with_nodes(&["a"]);
    let delta = UndoDelta::compute(stack.current(), &new_doc);
    stack.push(delta);
    assert_eq!(node_ids(stack.current()), vec!["a".to_string()]);
    let undone = stack.undo().unwrap();
    assert_eq!(node_ids(&undone), vec!["a".to_string(), "b".into()]);
}

#[test]
fn undo_reverses_modify() {
    let mut doc0 = doc_with_nodes(&["a"]);
    doc0.pages[0].nodes[0].x = 5.0;
    let mut stack = UndoRedoStack::new(doc0.clone());
    let mut new_doc = doc0.clone();
    new_doc.pages[0].nodes[0].x = 99.0;
    let delta = UndoDelta::compute(stack.current(), &new_doc);
    stack.push(delta);
    assert_eq!(stack.current().pages[0].nodes[0].x, 99.0);
    let undone = stack.undo().unwrap();
    assert_eq!(undone.pages[0].nodes[0].x, 5.0);
}

#[test]
fn redo_reapplies_delta() {
    let mut stack = UndoRedoStack::new(doc_with_nodes(&["a"]));
    let new_doc = doc_with_nodes(&["a", "b", "c"]);
    stack.push(UndoDelta::compute(stack.current(), &new_doc));
    let _ = stack.undo().unwrap();
    assert_eq!(node_ids(stack.current()), vec!["a".to_string()]);
    let redone = stack.redo().unwrap();
    assert_eq!(
        node_ids(&redone),
        vec!["a".to_string(), "b".into(), "c".into()]
    );
}

#[test]
fn delta_stack_serde_roundtrip() {
    let mut stack = UndoRedoStack::new(doc_with_nodes(&["a"]));
    stack.push(UndoDelta::compute(
        stack.current(),
        &doc_with_nodes(&["a", "b"]),
    ));
    let json = serde_json::to_string(&stack).unwrap();
    let back: UndoRedoStack = serde_json::from_str(&json).unwrap();
    assert_eq!(node_ids(back.current()), vec!["a".to_string(), "b".into()]);
}

#[test]
fn old_snapshot_history_migrates_with_warn() {
    // 旧快照式序列化（无 current 字段，纯 VecDeque<PenDocument>）→ 反序列化失败。
    // 旧格式: {"undo_stack":[{...PenDocument...}],"redo_stack":[]}
    let old = r#"{"undo_stack":[{"schema_version":1,"pages":[{"id":"p0","name":"P0","width":100,"height":100,"nodes":[{"id":"a","kind":"rect","name":"a","x":0,"y":0,"w":10,"h":10,"rotation":0,"z_index":0}]}],"variables":null,"active_design_system":null}],"redo_stack":[]}"#;
    let result: Result<UndoRedoStack, _> = serde_json::from_str(old);
    assert!(
        result.is_err(),
        "旧快照式格式应反序列化失败（缺 current 字段）触发迁移"
    );
}
