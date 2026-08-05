import init, { WebShell, fusion_bridge_send_command } from "./pkg/fd_host_web.js";

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

function send(cmd) {
  fusion_bridge_send_command(JSON.stringify(cmd));
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

cases.push(["PageRender 渲染 mock PenDocument", async () => {
  send({ PageRender: { document_json: JSON.stringify(mockDoc()) } });
  await new Promise((r) => requestAnimationFrame(r));
  await new Promise((r) => requestAnimationFrame(r));
  const host = document.getElementById("canvas-host");
  const nodes = host.querySelectorAll("[data-node-id]");
  record(nodes.length >= 3, `渲染节点数 >= 3`, `实际 ${nodes.length}`);
}]);

cases.push(["SelectNode 选中节点（触发 node.select 事件）", async () => {
  const got = captureEvent("node.select");
  send({ SelectNode: { node_id: "n1" } });
  await new Promise((r) => requestAnimationFrame(r));
  const ev = await got;
  record(ev !== null, "回传 node.select 事件", ev ? "" : "未捕获事件");
  record(ev && ev.node_id === "n1", "node_id == n1", ev ? JSON.stringify(ev) : "");
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
  const styleId = "fusion-design-tokens";
  document.getElementById(styleId)?.remove();
  send({ ApplyTokens: { css: ":root{--color-accent:#007AFF;}" } });
  await new Promise((r) => requestAnimationFrame(r));
  const style = document.getElementById(styleId);
  record(!!style, `存在 #${styleId} 样式节点`);
  record(!!style && style.textContent.includes("--color-accent"), "CSS 含 --color-accent");
}]);

cases.push(["ClearCanvas 清空画布", async () => {
  send({ ClearCanvas: null });
  await new Promise((r) => requestAnimationFrame(r));
  const host = document.getElementById("canvas-host");
  const nodes = host.querySelectorAll("[data-node-id]");
  record(nodes.length === 0, "画布已清空", `残留 ${nodes.length} 个节点`);
}]);

cases.push(["PlanPreview + PlanApply 虚线预览流程", async () => {
  send({ PlanPreview: { document_json: JSON.stringify(mockDoc()) } });
  await new Promise((r) => requestAnimationFrame(r));
  await new Promise((r) => requestAnimationFrame(r));
  const previewEls = document.querySelectorAll('[data-plan="preview"]');
  record(previewEls.length >= 3, `PlanPreview 注入预览节点 >= 3`, `实际 ${previewEls.length}`);
  send({ PlanApply: null });
  await new Promise((r) => requestAnimationFrame(r));
  const afterApply = document.querySelectorAll('[data-plan="preview"]');
  record(afterApply.length === 0, "PlanApply 移除虚线预览层", `残留 ${afterApply.length}`);
}]);

cases.push(["SetNodeVisibility 隐藏节点", async () => {
  send({ PageRender: { document_json: JSON.stringify(mockDoc()) } });
  await new Promise((r) => requestAnimationFrame(r));
  send({ SetNodeVisibility: { node_id: "n2", visible: false } });
  await new Promise((r) => requestAnimationFrame(r));
  const el = document.querySelector('[data-node-id="n2"]');
  let hidden = false;
  if (el) {
    const disp = getComputedStyle(el).display;
    hidden = disp === "none" || el.hidden;
  }
  record(hidden, "n2 已隐藏", el ? `display=${getComputedStyle(el).display}` : "节点未找到");
}]);

function captureEvent(kind) {
  return new Promise((resolve) => {
    function handler(evt) {
      const data = evt.detail || evt.data;
      if (!data || data.kind !== kind) return;
      window.removeEventListener("fusion-bridge-event", handler);
      resolve(data);
    }
    window.addEventListener("fusion-bridge-event", handler);
    setTimeout(() => {
      window.removeEventListener("fusion-bridge-event", handler);
      resolve(null);
    }, 3000);
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
    shell = WebShell.mount("fusion-canvas");
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
