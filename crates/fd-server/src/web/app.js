// Fusion Design D1 工作台逻辑（issue#31）：
// 左对话（生成/迭代/截图还原）→ 中 WASM 画布（三视口）→ 右版本（回退）+ 导出。
// 离线：同源 fetch，无任何外部资源。zh/en + dark/light。
"use strict";

import { syncShell } from "./shell-sync.js";

// ── i18n ──
const I18N = {
    zh: {
        chat: "对话", canvas: "画布", versions: "版本", export: "导出",
        send: "生成", expHtml: "HTML", expReact: "React",
        canvasEmpty: "尚无设计。在左侧描述界面，生成后在此预览。",
        composerPh: "描述界面，或直接粘贴截图/草图还原…（Enter 发送，Shift+Enter 换行）",
        pasteHint: "已贴入截图，发送即还原",
        generated: "已生成 {v} 版 → {n} 节点。可继续对话迭代，或右侧导出。",
        iterated: "v{v} 完成，画布已更新（{n} 节点）。",
        restored: "截图已还原为 v{v}（{n} 节点）。",
        rolledBack: "已回退到 v{v}。",
        rollBack: "回退", view: "查看", current: "当前",
        layers: "图层", noLayers: "尚无节点",
        tokens: "Token", noTokens: "先选择设计系统",
        tokenHint: "选中节点后点击 Token 应用（Shift+点击应用到描边）。",
        tokenApplied: "已应用 {name} → 节点 {node}（v{v}）。",
        selectFirst: "先在图层或画布中选中一个节点",
        savedFile: "已保存 {file}",
        dragPersistFail: "拖拽位置保存失败：{e}",
        unauthorized: "未授权（401）：请在 Shell 登录后重试",
        exportFailed: "导出失败：{e}",
        reqFailed: "请求失败：{e}",
        busy: "生成中…", model: "模型",
        noSession: "尚无会话，先在左侧生成一个设计",
sendCode: "发给 Code", saveArtifact: "存 Artifacts",
sentCode: "已发给 Code（{n} 个文件写入 fusion-code IPC 目录）。",
savedArtifact: "已存 Artifacts（已入库，回到 Artifacts 页可见）。",
        savedArtifactDegraded: "IPC 已保存，但 artifacts-engine 入库失败：{e}",
    },
    en: {
        chat: "Chat", canvas: "Canvas", versions: "Versions", export: "Export",
        send: "Generate", expHtml: "HTML", expReact: "React",
        canvasEmpty: "Nothing yet. Describe an interface on the left to generate.",
        composerPh: "Describe an interface, or paste a screenshot/sketch… (Enter to send, Shift+Enter for newline)",
        pasteHint: "Screenshot attached — send to restore",
        generated: "Generated v{v} → {n} nodes. Keep chatting to iterate, or export on the right.",
        iterated: "v{v} done, canvas updated ({n} nodes).",
        restored: "Screenshot restored as v{v} ({n} nodes).",
        rolledBack: "Rolled back to v{v}.",
        rollBack: "Rollback", view: "View", current: "Current",
        layers: "Layers", noLayers: "No nodes yet",
        tokens: "Tokens", noTokens: "Pick a design system first",
        tokenHint: "Select a node, then click a token to apply (Shift+click applies to stroke).",
        tokenApplied: "Applied {name} to node {node} (v{v}).",
        selectFirst: "Select a node in the layers or on the canvas first",
        savedFile: "Saved {file}",
        dragPersistFail: "Failed to persist drag position: {e}",
        unauthorized: "Unauthorized (401): sign in via the shell and retry",
        exportFailed: "Export failed: {e}",
        reqFailed: "Request failed: {e}",
        busy: "Generating…", model: "Model",
        noSession: "No session yet — generate a design first",
sendCode: "Send to Code", saveArtifact: "Save to Artifacts",
sentCode: "Sent to Code ({n} files written to the fusion-code IPC dir).",
savedArtifact: "Saved to Artifacts (created in the engine; visible on the Artifacts page).",
        savedArtifactDegraded: "IPC saved, but the artifacts-engine write failed: {e}",
    },
};
let lang = "zh";
function t(key, vars) {
    let s = (I18N[lang] || I18N.zh)[key] || key;
    if (vars) for (const [k, v] of Object.entries(vars)) s = s.replaceAll(`{${k}}`, v);
    return s;
}

// ── 状态 ──
const state = {
    session: null,
    currentVersion: null,
    document: null,
    designSystem: null,
    viewport: "desktop",
    pendingImage: null, // base64（无 data: 前缀）
    wasmReady: false,
};

const $ = (id) => document.getElementById(id);
const log = $("fd-chat");

// 模块基路径：直连 11493 时为 "/"，经 gateway /shell/fusion-design/ 代理时为该前缀。
// 资源与 API 一律相对此基路径解析，避免绝对路径打到 gateway 根（404 → 无样式）。
const BASE = new URL(".", import.meta.url).pathname;
function url(path) {
    return BASE + String(path).replace(/^\/+/, "");
}

// ── API ──
async function api(path, body) {
    const opt = body
        ? { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }
        : {};
    const resp = await fetch(url(path), opt);
    // issue#42 契约：模块内 401 只内联报错，绝不跳转/重定向（不踢 shell，#28 合同）。
    if (resp.status === 401) throw new Error(t("unauthorized"));
    const data = await resp.json().catch(() => ({}));
    if (!resp.ok) throw new Error(data.error || `HTTP ${resp.status}`);
    return data;
}

// ── 对话气泡 ──
function addMsg(role, text, meta) {
    const div = document.createElement("div");
    div.className = `msg ${role}`;
    div.textContent = text;
    if (meta) {
        const m = document.createElement("span");
        m.className = "msg-meta";
        m.textContent = meta;
        div.appendChild(m);
    }
    log.appendChild(div);
    log.scrollTop = log.scrollHeight;
    return div;
}

// ── 画布（WASM 桥）──
async function initCanvas() {
    try {
        const mod = await import(url("fd_host_web.js"));
        await mod.default(url("fd_host_web_bg.wasm"));
        // mount 校验 <canvas> 存在（canvas 2D 路径）；DOM 渲染管线落 #fusion-dom-root。
        mod.mount("fd-canvas-mount");
        state.wasmReady = true;
        // 消费 wasm 出站事件（node click/select → 图层高亮同步，issue#35）。
        setInterval(pollWasmEvents, WASM_QUEUE_POLL_MS);
    } catch (e) {
        console.warn("canvas wasm unavailable:", e);
        addMsg("bot", `画布 WASM 未加载：${e.message}（生成/导出不受影响，预览降级为空）`);
    }
}

// PenDocument → 画布。经 window.postMessage 驱动 fd-host-web 桥（浏览器路径）。
function renderDoc(docJson) {
    renderLayers(docJson);
    if (!state.wasmReady) return;
    window.postMessage(
        { kind: "page.render-dom", payload: { document: docJson }, schema_version: 1 },
        window.location.origin
    );
    // 设计系统 token CSS 注入画布（tokens.apply）。
    const ds = pickDesignSystem(docJson);
    if (ds) {
        fetch(url(`api/tokens?ds=${encodeURIComponent(ds)}&theme=${currentTheme()}`))
            .then((r) => r.json())
            .then((d) => {
                if (d.css) {
                    window.postMessage(
                        { kind: "tokens.apply", payload: { css: d.css }, schema_version: 1 },
                        window.location.origin
                    );
                }
            })
            .catch(() => {});
    }
    // Token 面板跟随文档的设计系统（issue#42）。
    loadTokenPanel();
    fitStage(docJson);
}

function pickDesignSystem(docJson) {
    try {
        const doc = JSON.parse(docJson);
        return doc.active_design_system || state.designSystem;
    } catch {
        return state.designSystem;
    }
}

function docSize(docJson) {
    try {
        const page = JSON.parse(docJson).pages[0];
        return { w: page.width || 1440, h: page.height || 900 };
    } catch {
        return { w: 1440, h: 900 };
    }
}

// 视口适配：stage 固定文档尺寸，CSS transform scale 适配视口框。
const VIEWPORTS = {
    iphone: { w: 390, h: 700, label: "iPhone" },
    ipad: { w: 820, h: 900, label: "iPad" },
    desktop: { w: 1200, h: 820, label: "Desktop" },
};
function fitStage(docJson) {
    const stage = $("fd-canvas-stage");
    const frame = $("canvas-frame");
    const { w, h } = docSize(docJson);
    stage.style.width = `${w}px`;
    stage.style.height = `${h}px`;
    const vp = VIEWPORTS[state.viewport];
    const frameRect = frame.getBoundingClientRect();
    const availW = Math.min(vp.w, frameRect.width - 32);
    const availH = frameRect.height - 32;
    const scale = Math.min(availW / w, availH / h, 1);
    stage.style.transform = `scale(${scale})`;
    $("canvas-empty").classList.add("hidden");
}

function setViewport(vp) {
    state.viewport = vp;
    document.querySelectorAll(".vp").forEach((b) => {
        const active = b.dataset.vp === vp;
        b.classList.toggle("active", active);
        b.setAttribute("aria-selected", active ? "true" : "false");
    });
    if (state.document) fitStage(state.document);
}

// ── 图层 + 选择（issue#35 最小画布 UX）──
// 选中：postMessage node.select 驱动 wasm 高亮（data-node-id 元素）；
// 画布点击 → wasm 出站 node.click/node.select 事件 → 同步图层高亮。
const WASM_QUEUE_POLL_MS = 300;

function selectNode(nodeId) {
    state.selectedNode = nodeId || null;
    window.postMessage(
        { kind: "node.select", payload: { node_id: nodeId }, schema_version: 1 },
        window.location.origin
    );
    document.querySelectorAll("#fd-layers .layer-item").forEach((el) => {
        el.classList.toggle("active", el.dataset.nodeId === state.selectedNode);
    });
}

function renderLayers(docJson) {
    const box = $("fd-layers");
    if (!box) return;
    let nodes = [];
    try {
        const doc = JSON.parse(docJson);
        (doc.pages || []).forEach((p) => {
            (p.nodes || []).forEach((n) => nodes.push(n));
        });
    } catch {
        /* 容错：文档非法时清空图层 */
    }
    if (!nodes.length) {
        box.innerHTML = `<p class="side-note">${t("noLayers")}</p>`;
        return;
    }
    box.innerHTML = "";
    nodes.forEach((n) => {
        const item = document.createElement("div");
        item.className = `layer-item${n.id === state.selectedNode ? " active" : ""}`;
        item.dataset.nodeId = n.id;
        const kind = document.createElement("span");
        kind.className = "layer-kind";
        kind.textContent = n.kind || "?";
        const name = document.createElement("span");
        name.className = "layer-name";
        name.textContent = n.name || n.id;
        name.title = `${n.id} · ${n.kind || ""}`;
        item.appendChild(kind);
        item.appendChild(name);
        item.onclick = () => selectNode(n.id);
        box.appendChild(item);
    });
}

// 消费 wasm 出站事件队列（原 v1 仅丢弃防堆积，现消费选择事件）。
function pollWasmEvents() {
    const q = navigator.__fd_host_post;
    if (Array.isArray(q)) {
        while (q.length) {
            const raw = q.shift();
            try {
                const ev = typeof raw === "string" ? JSON.parse(raw) : raw;
                const kind = ev && ev.kind;
                const nid = ev && ev.payload && ev.payload.node_id;
                if ((kind === "node.click" || kind === "node.select") && nid) {
                    syncLayerSelection(nid);
                }
                if (kind === "node.drag" && nid) {
                    // issue#42：拖拽持久化（防抖落库）+ 同步图层选中。
                    syncLayerSelection(nid);
                    onWasmNodeDrag(nid, (ev.payload.dx || 0), (ev.payload.dy || 0));
                }
            } catch {
                /* 非 JSON 事件忽略 */
            }
        }
    }
}

// 仅同步图层高亮，不回写 wasm（防 select 事件环）。
function syncLayerSelection(nodeId) {
    state.selectedNode = nodeId || null;
    document.querySelectorAll("#fd-layers .layer-item").forEach((el) => {
        el.classList.toggle("active", el.dataset.nodeId === state.selectedNode);
    });
}

// ── Token 面板（issue#42）──
// 受限取色：只从激活设计系统的 Token 里选，不出现裸取色器。
async function loadTokenPanel() {
    const box = $("fd-tokens");
    if (!box) return;
    const ds = pickDesignSystem(state.document) || state.designSystem;
    if (!ds) {
        box.innerHTML = `<p class="side-note">${t("noTokens")}</p>`;
        state.tokens = null;
        return;
    }
    try {
        const d = await fetch(url(`api/tokens?ds=${encodeURIComponent(ds)}&theme=${currentTheme()}`))
            .then((r) => r.json());
        state.tokens = d.tokens || [];
        box.innerHTML = "";
        state.tokens.forEach((tok) => {
            const item = document.createElement("button");
            item.type = "button";
            item.className = "token-item";
            const isColor = /^#|rgb/i.test(tok.value) && tok.value !== "transparent";
            const isNumber = /-?\d+(\.\d+)?px$/.test(tok.value);
            if (isColor) item.classList.add("tok-color");
            else if (isNumber) item.classList.add("tok-num");
            else return; // 字体族/shadow 等非取值 token 不上面板
            const swatch = document.createElement("span");
            swatch.className = "tok-swatch";
            if (isColor) swatch.style.background = tok.value;
            else swatch.textContent = tok.value.replace("px", "");
            const label = document.createElement("span");
            label.className = "tok-name";
            label.textContent = tok.name;
            item.appendChild(swatch);
            item.appendChild(label);
            item.title = `${tok.name} = ${tok.value}`;
            item.onclick = (ev) => applyToken(tok, ev.shiftKey);
            box.appendChild(item);
        });
        if (!box.children.length) box.innerHTML = `<p class="side-note">${t("noTokens")}</p>`;
    } catch {
        box.innerHTML = `<p class="side-note">${t("noTokens")}</p>`;
    }
}

// Token → 选中节点：color→fill（Shift→stroke），radius→radius，font.size→font_size。
async function applyToken(tok, toStroke) {
    if (!state.selectedNode) {
        addMsg("bot", t("selectFirst"));
        return;
    }
    if (!state.session) {
        addMsg("bot", t("noSession"));
        return;
    }
    const body = { node_id: state.selectedNode };
    if (tok.name.startsWith("color.")) {
        if (toStroke) body.stroke = tok.value;
        else body.fill = tok.value;
    } else if (tok.name.startsWith("radius")) {
        body.radius = parseFloat(tok.value);
    } else if (tok.name.startsWith("font.size")) {
        body.font_size = parseFloat(tok.value);
    } else {
        return; // spacing 等暂无画布落点
    }
    try {
        const d = await api(`/api/session/${state.session}/mutate`, body);
        applyDoc(d);
        addMsg("bot", t("tokenApplied", { name: tok.name, node: state.selectedNode, v: d.current_version }));
    } catch (e) {
        addMsg("bot", t("reqFailed", { e: e.message }));
    }
}

// ── 画布拖拽持久化（issue#42）──
// wasm 拖拽只改 DOM 并持续发 node.drag{node_id,dx,dy}（相对按下点累计）。
// 防抖 300ms 取最后一次累计位移，换算绝对坐标落 /mutate 新版本。
const DRAG_PERSIST_MS = 300;
const pendingDrags = new Map();
function onWasmNodeDrag(nodeId, dx, dy) {
    pendingDrags.set(nodeId, { dx, dy });
    clearTimeout(pendingDrags.get(nodeId).timer);
    pendingDrags.get(nodeId).timer = setTimeout(async () => {
        const d = pendingDrags.get(nodeId);
        pendingDrags.delete(nodeId);
        if (!d || !state.session) return;
        const base = findNodePos(state.document, nodeId);
        if (!base) return;
        try {
            const resp = await api(`/api/session/${state.session}/mutate`, {
                node_id: nodeId,
                x: base.x + d.dx,
                y: base.y + d.dy,
            });
            applyDoc(resp);
        } catch (e) {
            addMsg("bot", t("dragPersistFail", { e: e.message }));
        }
    }, DRAG_PERSIST_MS);
}

function findNodePos(docJson, nodeId) {
    try {
        const doc = JSON.parse(docJson);
        const stack = [...(doc.pages || []).flatMap((p) => p.nodes || [])];
        while (stack.length) {
            const n = stack.shift();
            if (n.id === nodeId) return { x: n.x || 0, y: n.y || 0 };
            if (Array.isArray(n.children)) stack.push(...n.children);
        }
    } catch { /* 容错 */ }
    return null;
}

// ── 保存 .fusiondesign（PenDocument JSON，git 友好）──
function saveFile() {
    if (!state.document) {
        addMsg("bot", t("noSession"));
        return;
    }
    const name = `${state.session || "design"}-v${state.currentVersion || 1}.fusiondesign`;
    const blob = new Blob([state.document], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = name;
    a.click();
    URL.revokeObjectURL(a.href);
    addMsg("bot", t("savedFile", { file: name }));
}

// ── 撤销/重做（版本栈指针移动，rollback 不产生新版本）──
async function stepVersion(delta) {
    if (!state.session || !state.currentVersion) return;
    const target = state.currentVersion + delta;
    if (target < 1 || target > state.maxVersion) return;
    try {
        const d = await api("/api/rollback", { session: state.session, version: target });
        state.currentVersion = d.current_version;
        state.document = d.document;
        renderDoc(d.document);
        renderLayers(d.document);
        refreshVersions();
    } catch (e) {
        addMsg("bot", t("reqFailed", { e: e.message }));
    }
}

function updateUndoRedo() {
    const undo = $("undo-btn");
    const redo = $("redo-btn");
    if (!undo || !redo) return;
    const cur = state.currentVersion || 0;
    undo.disabled = !(state.session && cur > 1);
    redo.disabled = !(state.session && cur < (state.maxVersion || 0));
}

// ── 版本列表 ──
async function refreshVersions() {
    const list = $("fd-versions");
    if (!state.session) {
        list.innerHTML = `<p class="side-note">${t("noSession")}</p>`;
        updateUndoRedo();
        return;
    }
    const data = await api(`/api/session/${state.session}`);
    state.currentVersion = data.current_version;
    state.maxVersion = (data.versions || []).reduce((m, v) => Math.max(m, v.version), 0);
    updateUndoRedo();
    list.innerHTML = "";
    // 新版在上。
    [...data.versions].reverse().forEach((v) => {
        const item = document.createElement("div");
        item.className = `version-item${v.version === data.current_version ? " current" : ""}`;
        const label = document.createElement("div");
        label.className = "v-label";
        label.innerHTML = `<div class="v-title">v${v.version} · ${escapeHtml(v.label)}</div>
            <div class="v-meta">${v.created_at} · ${v.node_count} ${lang === "zh" ? "节点" : "nodes"}${v.version === data.current_version ? ` · ${t("current")}` : ""}</div>`;
        const actions = document.createElement("div");
        actions.className = "v-actions";
        const viewBtn = document.createElement("button");
        viewBtn.textContent = t("view");
        viewBtn.onclick = async () => {
            const d = await api(`/api/version/${state.session}/${v.version}`);
            state.document = d.document;
            state.currentVersion = v.version;
            renderDoc(d.document);
            renderLayers(d.document);
            refreshVersions();
        };
        actions.appendChild(viewBtn);
        if (v.version !== data.current_version) {
            const rbBtn = document.createElement("button");
            rbBtn.textContent = t("rollBack");
            rbBtn.onclick = async () => {
                const d = await api("/api/rollback", { session: state.session, version: v.version });
                state.document = d.document;
                state.currentVersion = v.version;
                renderDoc(d.document);
                renderLayers(d.document);
                addMsg("bot", t("rolledBack", { v: v.version }));
                refreshVersions();
            };
            actions.appendChild(rbBtn);
        }
        item.appendChild(label);
        item.appendChild(actions);
        list.appendChild(item);
    });
}

function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

// ── 生成 / 迭代 / 还原 ──
async function handleSend() {
    const input = $("prompt-input");
    const prompt = input.value.trim();
    if (!prompt && !state.pendingImage) return;
    const sendBtn = $("send-btn");
    sendBtn.disabled = true;

    // 截图还原：有图优先。
    if (state.pendingImage) {
        addMsg("user", `[${lang === "zh" ? "截图" : "screenshot"}] ${prompt || t("pasteHint")}`);
        const busyMsg = addMsg("bot", t("busy"));
        try {
            const body = {
                image_base64: state.pendingImage,
                hint: prompt || "还原此界面",
                design_system: state.designSystem || undefined,
            };
            if (state.session) body.session = state.session;
            const d = await api("/api/restore", body);
            applyDoc(d);
            busyMsg.textContent = t("restored", { v: d.current_version, n: nodeCount(d.document) });
        } catch (e) {
            busyMsg.textContent = t("reqFailed", { e: e.message });
            busyMsg.classList.add("err");
        }
        clearPendingImage();
        sendBtn.disabled = false;
        return;
    }

    addMsg("user", prompt);
    const busyMsg = addMsg("bot", t("busy"));
    input.value = "";
    try {
        let d;
        if (state.session && state.currentVersion) {
            d = await api("/api/iterate", {
                session: state.session,
                instruction: prompt,
                design_system: state.designSystem || undefined,
                model: state.model || undefined,
            });
            busyMsg.textContent = t("iterated", { v: d.current_version, n: nodeCount(d.document) });
        } else {
            d = await api("/api/generate", {
                prompt,
                design_system: state.designSystem || undefined,
                model: state.model || undefined,
            });
            busyMsg.textContent = t("generated", { v: d.current_version, n: nodeCount(d.document) });
        }
        applyDoc(d);
    } catch (e) {
        busyMsg.textContent = t("reqFailed", { e: e.message });
        busyMsg.classList.add("err");
    }
    sendBtn.disabled = false;
    input.focus();
}

function nodeCount(docJson) {
    try {
        return JSON.parse(docJson).pages.reduce((a, p) => a + (p.nodes?.length || 0), 0);
    } catch {
        return "?";
    }
}

function applyDoc(d) {
    state.session = d.session;
    state.currentVersion = d.current_version;
    state.document = d.document;
    renderDoc(d.document);
    refreshVersions();
}

// ── 粘贴截图 ──
function clearPendingImage() {
    state.pendingImage = null;
    $("pending-image").classList.add("hidden");
    $("paste-hint").classList.add("hidden");
}

function handlePaste(ev) {
    const items = ev.clipboardData?.items || [];
    for (const item of items) {
        if (item.type.startsWith("image/")) {
            const file = item.getAsFile();
            if (!file) continue;
            const reader = new FileReader();
            reader.onload = () => {
                // data:image/png;base64,<payload> → 纯 base64。
                const b64 = String(reader.result).split(",")[1] || "";
                if (!b64) return;
                state.pendingImage = b64;
                $("pending-image").textContent = `📎 ${file.name || "screenshot"} (${Math.round(file.size / 1024)}KB)`;
                $("pending-image").classList.remove("hidden");
                $("paste-hint").classList.remove("hidden");
            };
            reader.readAsDataURL(file);
            ev.preventDefault();
            return;
        }
    }
}

// ── 导出 ──
async function doExport(format) {
    if (!state.session) {
        addMsg("bot", t("noSession"));
        return;
    }
    try {
        const d = await api("/api/export", {
            session: state.session,
            format,
            version: state.currentVersion || undefined,
        });
        const blob = new Blob([d.content], { type: d.mime });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = d.filename;
        a.click();
        URL.revokeObjectURL(url);
        addMsg("bot", `${t("export")}: ${d.filename}`);
    } catch (e) {
        addMsg("bot", t("exportFailed", { e: e.message }));
    }
}

// ── 主题 / 语言 ──
function applyTheme(theme) {
    document.documentElement.classList.toggle("light", theme === "light");
}

function applyI18n() {
    document.querySelectorAll("[data-i18n]").forEach((el) => {
        const v = t(el.dataset.i18n);
        if (v) el.textContent = v;
    });
    document.querySelectorAll("[data-i18n-ph]").forEach((el) => {
        const v = t(el.dataset.i18nPh);
        if (v) el.placeholder = v;
    });
    document.documentElement.lang = lang === "zh" ? "zh" : "en";
}

// ── 启动 ──
async function boot() {
    // 主题/语言统一跟随 shell（spec §2.1，模块不自带切换入口）。
    let booted = false;
    syncShell((theme, l) => {
        applyTheme(theme);
        if (state.document) renderDoc(state.document);
        const changed = booted && lang !== null && lang !== l;
        lang = l;
        applyI18n();
        if (changed) refreshVersions();
    });

    // 预设 chips。
    document.querySelectorAll("#preset-row .chip").forEach((b) => {
        b.onclick = () => {
            const on = b.classList.toggle("active");
            document.querySelectorAll("#preset-row .chip").forEach((x) => {
                if (x !== b) x.classList.remove("active");
            });
            state.designSystem = on ? b.dataset.ds : null;
        };
    });

    // 视口切换。
    document.querySelectorAll(".vp").forEach((b) => {
        b.onclick = () => setViewport(b.dataset.vp);
    });

    // 输入。
    $("composer").onsubmit = (ev) => {
        ev.preventDefault();
        handleSend();
    };
    $("prompt-input").addEventListener("keydown", (ev) => {
        if (ev.key === "Enter" && !ev.shiftKey) {
            ev.preventDefault();
            handleSend();
        }
    });
    document.addEventListener("paste", handlePaste);

    // 导出。
    $("export-html").onclick = () => doExport("html");
    $("export-react").onclick = () => doExport("react");

    // 生态出口（issue#33/DS-2）：发给 Code / 存 Artifacts。
    $("send-to-code").onclick = async () => {
        if (!state.session) {
            addMsg("bot", t("noSession"));
            return;
        }
        try {
            const d = await api("/api/send-to-code", {
                session: state.session,
                version: state.currentVersion || undefined,
            });
            addMsg("bot", t("sentCode", { n: (d.files || []).length }));
        } catch (e) {
            addMsg("bot", t("reqFailed", { e: e.message }));
        }
    };
    $("save-artifact").onclick = async () => {
        if (!state.session) {
            addMsg("bot", t("noSession"));
            return;
        }
        try {
            const d = await api("/api/save-artifact", {
                session: state.session,
                version: state.currentVersion || undefined,
                kind: state.kind || "design",
            });
            if (d.engine_degraded) {
                addMsg("bot", t("savedArtifactDegraded", { e: d.error || "" }));
            } else {
                addMsg("bot", t("savedArtifact"));
            }
        } catch (e) {
            addMsg("bot", t("reqFailed", { e: e.message }));
        }
    };

    // 撤销/重做（issue#35：版本栈指针移动）。
    $("undo-btn").onclick = () => stepVersion(-1);
    $("redo-btn").onclick = () => stepVersion(1);

    // 保存 .fusiondesign（issue#42）。
    $("save-file").onclick = saveFile;

    // 服务健康 + 模型选择（排除 embedding/rerank 等非对话模型）。
    try {
        const h = await api("/api/health?deep=1");
        const chip = $("svc-status");
        if (h.mlx && h.mlx.available) {
            chip.dataset.state = "ok";
            chip.textContent = "MLX ✓";
            // 只留对话模型：白名单家族 − 黑名单模态（图/视频/TTS/OCR/嵌入/视觉）。
            const fam = /qwen|llama|glm|deepseek|jais|nemotron|kimi|minimax|vibe/i;
            const bad = /image|vl|ocr|tts|embed|bge|e5|minilm|gte|rerank|whisper|clip|speech|audio|video|flux|schnell|dflash|flash2|fl2va|sft_|musetalk|ltx|wan2|kokoro/i;
            const models = (h.mlx.models || []).filter((m) => fam.test(m) && !bad.test(m));
            const sel = $("model-select");
            const saved = localStorage.getItem("fd-model");
            const current = h.mlx.chat_model || saved || models[0] || h.mlx.model || "";
            state.model = saved && models.includes(saved) ? saved : current;
            sel.innerHTML = "";
            for (const m of models) {
                const o = document.createElement("option");
                o.value = m;
                o.textContent = m;
                sel.appendChild(o);
            }
            if (state.model && !models.includes(state.model)) {
                const o = document.createElement("option");
                o.value = state.model;
                o.textContent = state.model;
                sel.appendChild(o);
            }
            sel.value = state.model || "";
            sel.onchange = () => {
                state.model = sel.value || null;
                localStorage.setItem("fd-model", state.model || "");
            };
        } else {
            chip.dataset.state = "err";
            chip.textContent = lang === "zh" ? "MLX 不可用" : "MLX down";
        }
    } catch {
        $("svc-status").dataset.state = "err";
        $("svc-status").textContent = "API ✗";
    }

    // 落地页跳转带入：kind + 预填提示词（光标停在末尾，等客户输入）。
    const params = new URLSearchParams(window.location.search);
    const prompt = params.get("prompt");
    if (prompt) {
        const input = $("prompt-input");
        input.value = prompt;
        input.focus();
        input.setSelectionRange(prompt.length, prompt.length);
    }
    state.kind = params.get("kind") || "design";

    // issue#42 内嵌模式（PRD §3.3）：?embedded=1 隐藏自有导航（返回入口），
    // shell iframe 内不留逃逸链接；其余功能不变。
    if (params.get("embedded") === "1") {
        document.body.classList.add("embedded");
    }

    setViewport("desktop");
    await initCanvas();
    refreshVersions();
    loadTokenPanel();
    booted = true;
}

boot();
