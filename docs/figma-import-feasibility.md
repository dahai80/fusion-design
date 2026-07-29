// Callers: design-plan-ar.md P5-5 task reference
// Affected API: N/A — evaluation report only, no code changes
// Data schemas: Figma JSON → PenDocument mapping proposed
// User instruction: "按照P1~P6顺序实施所有未完成的任务" — Task #34 P5-5 Figma 导入可行性验证

# Figma 文件导入可行性评估报告

**评估日期**: 2026-07-28
**项目约束**: 100% 离线，无外部网络请求

## 结论：不可行（离线约束下）

Figma 设计文件存储在 Figma 云端，**无本地文件格式**。导入必须通过网络请求 Figma REST API，违反项目离线硬约束。

## 备选方案

| 方案 | 离线兼容 | 说明 |
|------|---------|------|
| Figma → JSON 导出 + 本地解析 | ✅ | 用户手动从 Figma 导出 JSON，本地解析转 PenDocument |
| `.fig` 文件解析 | ❌ | `.fig` 是二进制 protobuf，格式未公开，无 Rust 解析器 |
| `dc_figma_import` (Google) | ❌ | 需要 Figma API fetch，非离线 |
| `figma-api` (gridaco) | ❌ | REST API client，需联网 |

## 推荐路径：Figma JSON 离线导入

1. **用户操作**: Figma Plugin → 导出选区为 JSON (Figma REST API response 格式)
2. **本地解析**: 新增 `fd-figma-import` crate，解析 Figma JSON → `PenDocument`
3. **映射规则**:
   - Figma `FRAME` → `PenNode::Group`
   - Figma `RECTANGLE` → `PenNode::Rect`
   - Figma `TEXT` → `PenNode::Text`
   - Figma `VECTOR` → `PenNode::Image` (光栅化)
   - Figma fills/strokes → `NodeStyle.fill/stroke`
   - Figma absoluteBoundingBox → `x/y/w/h`
   - Figma componentPropertyDefinitions → `ComponentSlot`

## 可用 Rust Crate

| Crate | 版本 | 用途 | 离线兼容 |
|-------|------|------|---------|
| `dc_figma_import` | 0.39.4 | Google automotive-design-compose 的 Figma 文档模型 | 数据模型可参考，fetch 功能不可用 |
| `figma-api` | 0.31.4 | Figma REST API Rust client | 仅数据模型可参考 |

## 实施建议

- **优先级**: 低（P5-5，非核心路径）
- **工作量**: 3-5 天
- **依赖**: 需用户手动导出 Figma JSON，体验不够流畅
- **风险**: Figma JSON schema 变动导致解析失败
- **MVP 建议**: 暂不实施，等用户需求驱动

## 如果实施：技术方案

```rust
// fd-figma-import (新 crate)
pub struct FigmaImporter;

impl FigmaImporter {
    /// 从 Figma REST API 导出的 JSON 解析为 PenDocument
    pub fn from_json(figma_json: &str) -> Result<PenDocument> {
        let figma_doc: FigmaDocument = serde_json::from_str(figma_json)?;
        let mut doc = PenDocument::new();
        for canvas in &figma_doc.document.children {
            let page = convert_canvas(canvas);
            doc.add_page(page);
        }
        Ok(doc)
    }
}
```

映射核心逻辑在 `convert_node()` 递归函数中，将 Figma Node 类型映射到 `PenNode` 枚举变体。
