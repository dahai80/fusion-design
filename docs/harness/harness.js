// fd-host-web E2E harness（OPS-11 复用，CI headless chromium 跑）。
//
// 契约对齐（务必与 crates/fd-host-web/src/lib.rs 实现一致，否则假绿）：
//   - mount 是顶层 #[wasm_bindgen] 自由函数（非 WebShell 静态方法）。
//   - DOM 渲染走 window.postMessage({kind:"page.render-dom",payload:{document:json}})，
//     非 fusion_bridge_send_command（后者仅 BridgeCommand enum，无 DOM 渲染变体）。
//   - 命令（SelectNode/MutateNode/ApplyTokens/ClearCanvas/PlanPreview/PlanApply/
//     SetNodeVisibility）走 fusion_bridge_send_command(JSON)，BridgeCommand serde
//     外部标记格式 {"VariantName":{fields}}。
//   - 事件回传：send_to_host 写 navigator.__fd_host_post（JSON 串 {direction,kind,payload}），
//     非 fusion-bridge-event CustomEvent。须轮询读取。
//   - node.select 仅由用户点击触发（非 SelectNode 命令），须模拟 click DOM 节点。
//   - token style id = "fusion-tokens"；隐藏 = data-fd-hidden 属性；plan 预览 = #fd-plan-preview。
import init, { mount, WebShell, fusion_bridge_send_command } from "./pkg/fd_host_web.js";

const logEl = document.getElementById("log");
const summaryEl = document.getElementById("summary");

function log(msg, cls = "info") {
  const span = document.createElement("span");
  span.className = cls;
  span.textContent = msg + "\n";
  logEl.appendChild(span);
  logEl.scrollTop = logEl.scrollHeight;
}

let passCount = 0;
let failCount = 0;
function record(pass, name, detail = "") {
  if (pass) {
    passCount++;
    log(`  PASS ${name}`, "ok");
  } else {
    failCount++;
    log(`  FAIL ${name} ${detail ? ":: " + detail : ""}`, "fail");
  }
}
function summarize() {
  summaryEl.textContent = `${passCount} passed, ${failCount} failed`;
  summaryEl.style.color = failCount === 0 ? "#0a7d23" : "#b00";
}

// 命令走 BridgeCommand serde 外部标记格式。
function send(cmd) {
  fusion_bridge_send_command(JSON.stringify(cmd));
}

// DOM 渲染走 window message（page.render-dom），非 BridgeCommand。
// 监听器用 event.data().as_string()，故 postMessage 必须传字符串（非对象）。
function renderDom(docJson) {
  const msg = JSON.stringify({
    kind: "page.render-dom",
    payload: { document: docJson },
    schema_version: 1,
  });
  window.postMessage(msg, "*");
}

function mockDoc() {
  return {
    id: "doc-harness",
    name: "HarnessDoc",
    version: "0.1.0",
    active_design_system: "apple-hig",
    pages: [
      {
        id: "p1",
        name: "Page1",
        width: 800,
        height: 600,
        nodes: [
          { id: "n1", kind: "Rect", name: "Card", x: 10, y: 10, w: 120, h: 80,
            style: { fill: "#007AFF" }, text: null, children: [] },
          { id: "n2", kind: "Text", name: "Title", x: 20, y: 20, w: 100, h: 24,
            style: { fill: "#FFFFFF", font_size: 16 }, text: "Hello", children: [] },
          { id: "n3", kind: "Circle", name: "Dot", x: 150, y: 30, w: 40, h: 40,
            style: { fill: "#FF3B30" }, text: null, children: [] },
        ],
      },
    ],
  };
}

const cases = [];

cases.push(["mount 加载 wasm 并校验 canvas", async (shell) => {
  record(shell instanceof WebShell, "mount 返回 WebShell 实例");
  const canvas = document.getElementById("fusion-canvas");
  record(canvas.tagName === "CANVAS", "canvas 元素存在");
}]);

cases.push(["page.render-dom 渲染 mock PenDocument 到 DOM", async () => {
  renderDom(JSON.stringify(mockDoc()));
  await new Promise((r) => requestAnimationFrame(r));
  await new Promise((r) => requestAnimationFrame(r));
  const host = document.getElementById("fusion-dom-root");
  const nodes = host ? host.querySelectorAll("[data-node-id]") : [];
  record(nodes.length >= 3, `渲染节点数 >= 3`, `实际 ${nodes.length}`);
}]);

cases.push(["点击节点触发 node.select 事件回传", async () => {
  const got = captureHostEvent("node.select");
  const el = document.querySelector('[data-node-id="n1"]');
  record(!!el, "n1 DOM 节点存在");
  if (el) {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window }));
  }
  const ev = await got;
  record(ev !== null, "回传 node.select 事件", ev ? "" : "未捕获事件");
  // BridgeEvent serde 外部标记：payload = {NodeSelect:{node_id}}，解包变体取内层。
  const inner = ev && ev.NodeSelect ? ev.NodeSelect : ev;
  record(inner && inner.node_id === "n1", "node_id == n1", ev ? JSON.stringify(ev) : "");
}]);

cases.push(["MutateNode 修改节点位置", async () => {
  send({ MutateNode: { node_id: "n1", x: 200, y: 150, w: null, h: null,
    fill: null, stroke: null, stroke_width: null, radius: null,
    font_size: null, font_family: null, opacity: null } });
  await new Promise((r) => requestAnimationFrame(r));
  await new Promise((r) => requestAnimationFrame(r));
  const el = document.querySelector('[data-node-id="n1"]');
  let moved = false;
  if (el) {
    const left = parseFloat(getComputedStyle(el).left) || el.getBoundingClientRect().left;
    moved = left >= 190;
  }
  record(moved, "n1 x 已更新到 ~200", el ? "" : "节点 DOM 未找到");
}]);

cases.push(["ApplyTokens 注入设计 Token CSS", async () => {
  document.getElementById("fusion-tokens")?.remove();
  send({ ApplyTokens: { css: ":root{--color-accent:#007AFF;}" } });
  await new Promise((r) => requestAnimationFrame(r));
  const style = document.getElementById("fusion-tokens");
  record(!!style, `存在 #fusion-tokens 样式节点`);
  record(!!style && style.textContent.includes("--color-accent"), "CSS 含 --color-accent");
}]);

cases.push(["ClearCanvas 清空画布", async () => {
  send({ ClearCanvas: null });
  await new Promise((r) => requestAnimationFrame(r));
  const host = document.getElementById("fusion-dom-root");
  const nodes = host ? host.querySelectorAll("[data-node-id]") : [];
  record(nodes.length === 0, "画布已清空", `残留 ${nodes.length} 个节点`);
}]);

cases.push(["PlanPreview + PlanApply 虚线预览流程", async () => {
  send({ PlanPreview: { document_json: JSON.stringify(mockDoc()) } });
  await new Promise((r) => requestAnimationFrame(r));
  await new Promise((r) => requestAnimationFrame(r));
  const overlay = document.getElementById("fd-plan-preview");
  const previewEls = overlay ? overlay.querySelectorAll("div") : [];
  record(previewEls.length >= 3, `PlanPreview 注入预览节点 >= 3`, `实际 ${previewEls.length}`);
  send({ PlanApply: null });
  await new Promise((r) => requestAnimationFrame(r));
  const afterApply = document.getElementById("fd-plan-preview");
  record(afterApply === null, "PlanApply 移除虚线预览层", afterApply ? "残留 overlay" : "");
}]);

cases.push(["SetNodeVisibility 隐藏节点", async () => {
  renderDom(JSON.stringify(mockDoc()));
  await new Promise((r) => requestAnimationFrame(r));
  send({ SetNodeVisibility: { node_id: "n2", visible: false } });
  await new Promise((r) => requestAnimationFrame(r));
  const el = document.querySelector('[data-node-id="n2"]');
  let hidden = false;
  if (el) {
    hidden = el.hasAttribute("data-fd-hidden");
  }
  record(hidden, "n2 已隐藏（data-fd-hidden）", el ? "" : "节点未找到");
}]);

// 轮询 navigator.__fd_host_post 事件队列（send_to_host 累积的字符串数组），
// shift 取首条匹配 kind 的消息 payload。3s 超时返 null。
// harness 无原生宿主 → send_to_host 走 __fd_host_post 回退队列路径。
function captureHostEvent(kind) {
  return new Promise((resolve) => {
    const start = performance.now();
    function poll() {
      const q = navigator.__fd_host_post;
      if (Array.isArray(q) && q.length > 0) {
        // 扫队列找首条匹配 kind 的，移除并返回；其余保留。
        for (let i = 0; i < q.length; i++) {
          try {
            const msg = JSON.parse(q[i]);
            if (msg.kind === kind) {
              q.splice(i, 1);
              resolve(msg.payload || null);
              return;
            }
          } catch (e) {
            // 非 JSON 条目，剔除避免堆积。
            q.splice(i, 1);
            i--;
          }
        }
      }
      if (performance.now() - start > 3000) {
        resolve(null);
        return;
      }
      requestAnimationFrame(poll);
    }
    poll();
  });
}

async function runAll() {
  passCount = 0;
  failCount = 0;
  logEl.textContent = "";
  log("== fd-host-web E2E harness 启动 ==");

  let shell;
  try {
    await init();
    shell = mount("fusion-canvas");
  } catch (e) {
    log(`mount 失败: ${e}`, "fail");
    summarize();
    return;
  }

  for (const [name, fn] of cases) {
    log(`\n[CASE] ${name}`);
    try {
      await fn(shell);
    } catch (e) {
      record(false, name, String(e));
    }
  }
  log(`\n== 完成 ==`);
  summarize();
}

document.getElementById("run-all").addEventListener("click", runAll);
document.getElementById("clear-log").addEventListener("click", () => {
  logEl.textContent = "";
  passCount = 0;
  failCount = 0;
  summarize();
});

runAll();
