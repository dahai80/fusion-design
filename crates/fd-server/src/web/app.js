// Fusion Design D1 工作台逻辑（issue#31）：
// 左对话（生成/迭代/截图还原）→ 中 WASM 画布（三视口）→ 右版本（回退）+ 导出。
// 离线：同源 fetch，无任何外部资源。zh/en + dark/light。
"use strict";

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
        exportFailed: "导出失败：{e}",
        reqFailed: "请求失败：{e}",
        busy: "生成中…", model: "模型",
        noSession: "尚无会话，先在左侧生成一个设计",
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
        exportFailed: "Export failed: {e}",
        reqFailed: "Request failed: {e}",
        busy: "Generating…", model: "Model",
        noSession: "No session yet — generate a design first",
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

// ── API ──
async function api(path, body) {
    const opt = body
        ? { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }
        : {};
    const resp = await fetch(path, opt);
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
        const mod = await import("/fd_host_web.js");
        await mod.default("/fd_host_web_bg.wasm");
        // mount 校验 <canvas> 存在（canvas 2D 路径）；DOM 渲染管线落 #fusion-dom-root。
        mod.mount("fd-canvas-mount");
        state.wasmReady = true;
        // 拉取并丢弃 wasm 出站事件队列（node click 等，v1 不消费，仅防堆积）。
        setInterval(() => {
            const q = navigator.__fd_host_post;
            if (Array.isArray(q)) while (q.length) q.shift();
        }, 2000);
    } catch (e) {
        console.warn("canvas wasm unavailable:", e);
        addMsg("bot", `画布 WASM 未加载：${e.message}（生成/导出不受影响，预览降级为空）`);
    }
}

// PenDocument → 画布。经 window.postMessage 驱动 fd-host-web 桥（浏览器路径）。
function renderDoc(docJson) {
    if (!state.wasmReady) return;
    window.postMessage(
        { kind: "page.render-dom", payload: { document: docJson }, schema_version: 1 },
        window.location.origin
    );
    // 设计系统 token CSS 注入画布（tokens.apply）。
    const ds = pickDesignSystem(docJson);
    if (ds) {
        fetch(`/api/tokens?ds=${encodeURIComponent(ds)}&theme=${currentTheme()}`)
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

// ── 版本列表 ──
async function refreshVersions() {
    const list = $("fd-versions");
    if (!state.session) {
        list.innerHTML = `<p class="side-note">${t("noSession")}</p>`;
        return;
    }
    const data = await api(`/api/session/${state.session}`);
    state.currentVersion = data.current_version;
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
            });
            busyMsg.textContent = t("iterated", { v: d.current_version, n: nodeCount(d.document) });
        } else {
            d = await api("/api/generate", {
                prompt,
                design_system: state.designSystem || undefined,
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
function currentTheme() {
    return document.documentElement.dataset.theme === "dark" ? "dark" : "light";
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
    // 主题：localStorage > 系统偏好。
    const savedTheme = localStorage.getItem("fd-theme");
    const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    document.documentElement.dataset.theme = savedTheme || (prefersDark ? "dark" : "light");
    $("theme-toggle").onclick = () => {
        const next = currentTheme() === "dark" ? "light" : "dark";
        document.documentElement.dataset.theme = next;
        localStorage.setItem("fd-theme", next);
        if (state.document) renderDoc(state.document);
    };

    // 语言：localStorage > 中文。
    lang = localStorage.getItem("fd-lang") || "zh";
    $("lang-select").value = lang;
    applyI18n();
    $("lang-select").onchange = () => {
        lang = $("lang-select").value;
        localStorage.setItem("fd-lang", lang);
        applyI18n();
        refreshVersions();
    };

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

    // 服务健康。
    try {
        const h = await api("/api/health?deep=1");
        const chip = $("svc-status");
        if (h.mlx && h.mlx.available) {
            chip.dataset.state = "ok";
            chip.textContent = `MLX ✓ ${h.mlx.model || ""}`;
        } else {
            chip.dataset.state = "err";
            chip.textContent = lang === "zh" ? "MLX 不可用" : "MLX down";
        }
    } catch {
        $("svc-status").dataset.state = "err";
        $("svc-status").textContent = "API ✗";
    }

    setViewport("desktop");
    await initCanvas();
    refreshVersions();
}

boot();
