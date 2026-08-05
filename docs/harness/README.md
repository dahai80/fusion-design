# fd-host-web E2E Harness

最小 HTML 测试页，用于黑盒验证 `fd-host-web` 的 WASM 渲染层与 Bridge 协议。对应 issue #10。

## 背景

`fd-host-web` 编译为 `wasm32-unknown-unknown`，设计为 Fusion-Desk WKWebView 内嵌模块。本仓库此前无独立 HTML 入口加载该 wasm，导致 GUI/WASM 层无法端到端验证。本 harness 填补该缺口。

## 文件

- `index.html` - 测试页：包含 `<canvas id="fusion-canvas">` 与 HUD 控制面板。
- `harness.js` - ES 模块：加载 wasm，注入 mock PenDocument，模拟触发 BridgeCommand，断言 DOM/Canvas 渲染结果与 BridgeEvent 回传。

## 覆盖用例

| 用例 | 验证点 | 对应 BridgeCommand / BridgeEvent |
|------|--------|----------------------------------|
| mount | 加载 wasm，校验 canvas | `WebShell.mount()` |
| PageRender | 注入 mock PenDocument，渲染节点 >= 3 | `PageRender` |
| SelectNode | 选中节点并断言 `node.select` 事件回传 | `SelectNode` -> `node.select` |
| MutateNode | 修改节点位置，DOM 坐标更新 | `MutateNode` |
| ApplyTokens | 注入设计 Token CSS 变量 | `ApplyTokens` |
| ClearCanvas | 清空画布，残留节点 = 0 | `ClearCanvas` |
| PlanPreview + PlanApply | 虚线预览注入与移除 | `PlanPreview` / `PlanApply` |
| SetNodeVisibility | 隐藏节点，display=none | `SetNodeVisibility` |

## 运行

### 1. 安装 wasm 工具链

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli
```

### 2. 构建 wasm

```bash
cd /Users/dahai/fusion/fusion-design
cargo build -p fd-host-web --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/fd_host_web.wasm \
  --out-dir docs/harness/pkg --target web
```

生成的 `docs/harness/pkg/fd_host_web.js` 与 `fd_host_web_bg.wasm` 即为 harness 加载入口。

### 3. 启动本地静态服务

由于 wasm 以 ES 模块加载，必须经 HTTP（不能 `file://`）：

```bash
cd docs/harness
python3 -m http.server 8765
# 浏览器打开 http://localhost:8765/
```

点击 HUD 中「运行全部用例」，或页面加载即自动运行。HUD 实时显示 PASS/FAIL 与日志。

### 4. Headless 回归

```bash
# 可选：headless 浏览器跑回归（需安装 playwright/chromedriver）
# 例：捕获 summary 文本判定全绿
```

## Mock PenDocument 结构

```jsonc
{
  "id": "doc-harness", "name": "HarnessDoc", "version": "0.1.0",
  "active_design_system": "apple-hig",
  "pages": [{
    "id": "p1", "name": "Page1", "width": 800, "height": 600,
    "nodes": [
      { "id": "n1", "kind": "Rect",   "name": "Card",  "x": 10,  "y": 10, "w": 120, "h": 80,  "style": { "fill": "#007AFF" }, "text": null, "children": [] },
      { "id": "n2", "kind": "Text",   "name": "Title", "x": 20,  "y": 20, "w": 100, "h": 24,  "style": { "fill": "#FFFFFF", "font_size": 16 }, "text": "Hello", "children": [] },
      { "id": "n3", "kind": "Circle", "name": "Dot",   "x": 150, "y": 30, "w": 40,  "h": 40,  "style": { "fill": "#FF3B30" }, "text": null, "children": [] }
    ]
  }]
}
```

`kind` 使用 PascalCase（`Rect`/`Circle`/`Text`/`Image`/`Group`），与 `fd-canvas-core::NodeKind` 的 serde 表示一致。

## 注意

- 本 harness 不依赖任何外网；wasm 与 mock 数据均为本地资产，符合 100% 离线约束。
- harness.js 通过 `window` 的 `fusion-bridge-event` 事件捕获 `BridgeEvent` 回传。若 wasm 实现改用 `webkit.messageHandlers` 通道，需同步调整 `captureEvent`。
