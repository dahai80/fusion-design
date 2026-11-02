// Artifact 查看器：点落地页卡片就地看正文，不跳外部应用（对齐 Claude 的
// 「chat + 就地预览」形状）。数据源 /api/artifact/<id>，转发本机 artifacts-engine。
// 内容为 HTML 时用 sandbox iframe 渲染（禁脚本），其他类型走纯文本。
"use strict";

import { syncShell } from "./shell-sync.js";

const BASE = new URL(".", import.meta.url).pathname;
const url = (p) => BASE + String(p).replace(/^\/+/, "");

const I18N = {
    zh: {
        download: "下载", openInWorkbench: "在工作台继续", empty: "产物正文不可用。",
        loading: "加载中…", degraded: "Artifacts 引擎不可用：{e}",
        meta: "v{v} · {d}", back: "返回 Artifacts",
    },
    en: {
        download: "Download", openInWorkbench: "Continue in workbench",
        empty: "Artifact content is unavailable.", loading: "Loading…",
        degraded: "Artifacts engine unavailable: {e}", meta: "v{v} · {d}", back: "Back to artifacts",
    },
};

let lang = "zh";
const t = (k, vars) => {
    let s = (I18N[lang] || I18N.zh)[k] || k;
    if (vars) for (const [key, v] of Object.entries(vars)) s = s.replaceAll(`{${key}}`, v);
    return s;
};

const $ = (id) => document.getElementById(id);
const PROMPTS = {
    design: "Make a Design artifact for ",
    document: "Make a Document artifact about ",
    slides: "Make a Slides artifact about ",
};

let current = { name: "artifact", content: "" };

function fmtDate(ts) {
    if (!ts) return "";
    const d = new Date(ts * 1000);
    if (Number.isNaN(d.getTime())) return "";
    const pad = (n) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function applyI18n() {
    document.querySelectorAll("[data-i18n]").forEach((el) => {
        const v = t(el.dataset.i18n);
        if (v) el.textContent = v;
    });
    $("back-to-artifacts").title = t("back");
    document.documentElement.lang = lang === "zh" ? "zh" : "en";
}

function applyTheme(theme) {
    document.documentElement.classList.toggle("light", theme === "light");
}

function sanitizeName(name) {
    return (name || "artifact").replace(/[\\/:*?"<>|]+/g, "-").slice(0, 80) || "artifact";
}

function showContent(content) {
    const frame = $("artifact-frame");
    const code = $("artifact-code");
    const looksHtml = /^\s*<(!doctype|html|head|body|div|section|main)/i.test(content);
    if (looksHtml) {
        frame.classList.remove("hidden");
        code.classList.add("hidden");
        frame.srcdoc = content;
    } else {
        frame.classList.add("hidden");
        code.classList.remove("hidden");
        code.textContent = content;
    }
    $("artifact-empty").classList.add("hidden");
}

function wireWorkbench(artifact) {
    const link = $("artifact-open-workbench");
    const kind = (artifact.kind || "design").toLowerCase();
    const prompt = PROMPTS[kind] || "";
    link.href = `workbench.html?kind=${encodeURIComponent(kind)}&prompt=${encodeURIComponent(prompt)}&artifact=${encodeURIComponent(artifact.id)}`;
    link.classList.remove("hidden");
}

function wireDownload() {
    $("artifact-export").onclick = () => {
        const blob = new Blob([current.content], { type: "text/html;charset=utf-8" });
        const a = document.createElement("a");
        a.href = URL.createObjectURL(blob);
        a.download = `${sanitizeName(current.name)}.html`;
        a.click();
        setTimeout(() => URL.revokeObjectURL(a.href), 1000);
    };
}

async function load() {
    const id = new URLSearchParams(window.location.search).get("id") || "";
    if (!id) {
        $("artifact-title").textContent = "Artifact";
        $("artifact-empty").classList.remove("hidden");
        return;
    }
    let data;
    try {
        const resp = await fetch(url(`api/artifact/${encodeURIComponent(id)}`));
        data = await resp.json();
    } catch (e) {
        data = { degraded: true, error: e.message };
    }
    if (data.degraded) {
        $("artifact-title").textContent = id;
        $("artifact-empty").textContent = t("degraded", { e: data.error || "unknown" });
        $("artifact-empty").classList.remove("hidden");
        return;
    }
    const a = data.artifact || {};
    current = { name: a.name || id, content: data.content || "" };
    $("artifact-title").textContent = a.name || id;
    $("artifact-kind").textContent = a.kind || a.type || "";
    $("artifact-meta").textContent = t("meta", { v: a.current_version || 1, d: fmtDate(a.updated_at) });
    wireWorkbench(a);
    if (!current.content) {
        $("artifact-empty").classList.remove("hidden");
        return;
    }
    showContent(current.content);
}

function boot() {
    // 主题/语言统一跟随 shell（spec §2.1，模块不自带切换入口）。
    syncShell((theme, l) => {
        applyTheme(theme);
        lang = l;
        applyI18n();
    });
    wireDownload();
    load();
}

boot();
