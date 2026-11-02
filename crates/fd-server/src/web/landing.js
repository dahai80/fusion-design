// Artifacts 落地页（三 tab + 创建新内容 + 列表分页）。
// 数据源：fd-server /api/artifacts（转发本机 artifacts-engine）。离线。
// 创建卡片跳 workbench.html，携带各类型的预填提示词，等客户输入。
"use strict";

const BASE = new URL(".", import.meta.url).pathname;
const url = (p) => BASE + String(p).replace(/^\/+/, "");

// 预填提示词：光标停在末尾等用户接话。
const PROMPTS = {
    design: "Make a Design artifact for ",
    document: "Make a Document artifact about ",
    slides: "Make a Slides artifact about ",
};

const I18N = {
    zh: {
        tabAll: "全部", tabMine: "我的", tabShared: "与您共享",
        searchPh: "搜索 Artifacts", sortOpened: "最近打开", sortCreated: "创建日期",
        sortName: "名称", sortPinned: "置顶优先", createNew: "创建新内容",
        docTitle: "文档", docSub: "写作、总结与说明",
        slidesTitle: "幻灯片", slidesSub: "演示与汇报",
        designTitle: "设计", designSub: "界面与原型",
        allArtifacts: "所有 Artifacts", emptyList: "还没有内容。从上面的「创建新内容」开始。",
        prev: "上一页", next: "下一页",
        degraded: "Artifacts 引擎未就绪（{e}），列表暂不可用；创建新内容不受影响。",
        sharedNote: "本机单租户：显示已分享出去的产物。",
        open: "打开", pinned: "置顶", count: "{n} 项",
    },
    en: {
        tabAll: "All", tabMine: "Mine", tabShared: "Shared with you",
        searchPh: "Search artifacts", sortOpened: "Last opened", sortCreated: "Date created",
        sortName: "Name", sortPinned: "Pinned first", createNew: "Create new",
        docTitle: "Document", docSub: "Writing, summaries, briefs",
        slidesTitle: "Slides", slidesSub: "Presentations and decks",
        designTitle: "Design", designSub: "Interfaces and prototypes",
        allArtifacts: "All artifacts", emptyList: "Nothing yet — start from Create new above.",
        prev: "Previous", next: "Next",
        degraded: "Artifacts engine unavailable ({e}); listing is disabled. Creating still works.",
        sharedNote: "Single-tenant local: shows artifacts you have shared out.",
        open: "Open", pinned: "Pinned", count: "{n} items",
    },
};

let lang = "zh";
const t = (k, vars) => {
    let s = (I18N[lang] || I18N.zh)[k] || k;
    if (vars) for (const [key, v] of Object.entries(vars)) s = s.replaceAll(`{${key}}`, v);
    return s;
};

const $ = (id) => document.getElementById(id);
const state = { tab: "all", page: 1, pageSize: 24, sort: "updated_at", q: "", lastTotal: 0 };

function currentTheme() {
    return document.documentElement.classList.contains("light") ? "light" : "dark";
}

function parentShellLight() {
    try {
        const doc = window.parent !== window ? window.parent.document : null;
        return doc ? doc.documentElement.classList.contains("light") : null;
    } catch {
        return null;
    }
}

function applyTheme(theme) {
    document.documentElement.classList.toggle("light", theme === "light");
}

function watchShellTheme() {
    try {
        const doc = window.parent !== window ? window.parent.document : null;
        if (!doc) return;
        new MutationObserver(() => {
            const shell = parentShellLight();
            if (shell !== null) applyTheme(shell ? "light" : "dark");
        }).observe(doc.documentElement, { attributes: true, attributeFilter: ["class"] });
    } catch {
        // 跨域访问失败：仅用本地主题。
    }
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

function createHref(kind) {
    return `workbench.html?kind=${encodeURIComponent(kind)}&prompt=${encodeURIComponent(PROMPTS[kind] || "")}`;
}

function refreshCreateLinks() {
    document.querySelectorAll(".create-card").forEach((a) => {
        a.href = createHref(a.dataset.kind);
    });
}

function fmtDate(ts) {
    if (!ts) return "";
    const d = new Date(ts * 1000);
    if (Number.isNaN(d.getTime())) return "";
    const pad = (n) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function kindLabel(a) {
    const k = (a.kind || a.type || "").toLowerCase();
    const map = { document: t("docTitle"), slides: t("slidesTitle"), app: t("designTitle"), code: "Code", tool: "Tool", game: "Game", template: "Template", html: t("designTitle"), markdown: t("docTitle"), react: t("designTitle") };
    return map[k] || (a.kind || a.type || "artifact");
}

function renderCards(items) {
    const grid = $("artifact-grid");
    grid.innerHTML = "";
    $("list-empty").classList.toggle("hidden", items.length > 0);
    for (const a of items) {
        const card = document.createElement("button");
        card.className = "artifact-card";
        card.onclick = () => {
            const kind = (a.kind || "design").toLowerCase();
            const p = PROMPTS[kind] || `Continue the ${kind} artifact about `;
            location.href = `workbench.html?kind=${encodeURIComponent(kind)}&prompt=${encodeURIComponent(p)}&artifact=${encodeURIComponent(a.id)}`;
        };
        const head = document.createElement("div");
        head.className = "card-head";
        const title = document.createElement("span");
        title.className = "card-title";
        title.textContent = a.name || a.id;
        head.appendChild(title);
        if (a.is_pinned) {
            const pin = document.createElement("span");
            pin.className = "badge";
            pin.textContent = t("pinned");
            head.appendChild(pin);
        }
        const kindEl = document.createElement("div");
        kindEl.className = "card-kind";
        kindEl.textContent = kindLabel(a);
        const sub = document.createElement("p");
        sub.className = "card-summary";
        sub.textContent = a.summary || "";
        const meta = document.createElement("div");
        meta.className = "card-meta";
        meta.textContent = `${fmtDate(a.updated_at)} · v${a.current_version || 1}`;
        card.append(head, kindEl, sub, meta);
        grid.appendChild(card);
    }
}

async function load() {
    const params = new URLSearchParams({
        tab: state.tab,
        page: String(state.page),
        page_size: String(state.pageSize),
        sort: state.sort,
    });
    if (state.q) params.set("q", state.q);
    let data;
    try {
        const resp = await fetch(url(`api/artifacts?${params}`));
        data = await resp.json();
    } catch (e) {
        data = { artifacts: [], total: 0, degraded: true, error: e.message };
    }
    if (data.degraded) {
        $("list-empty").classList.remove("hidden");
        $("list-empty").textContent = t("degraded", { e: data.error || "unknown" });
        renderCards([]);
        $("list-count").textContent = "";
        $("page-label").textContent = "";
        $("pager").classList.add("hidden");
        return;
    }
    $("list-empty").textContent = t("emptyList");
    renderCards(data.artifacts || []);
    state.lastTotal = data.total || 0;
    $("list-count").textContent = t("count", { n: state.lastTotal });
    const pages = Math.max(1, Math.ceil(state.lastTotal / state.pageSize));
    $("page-label").textContent = `${state.page} / ${pages}`;
    $("prev-page").disabled = state.page <= 1;
    $("next-page").disabled = state.page >= pages;
    $("pager").classList.toggle("hidden", state.lastTotal <= state.pageSize);
    if (state.tab === "shared" && state.lastTotal > 0) {
        $("list-count").textContent += ` · ${t("sharedNote")}`;
    }
}

function setTab(tab) {
    state.tab = tab;
    state.page = 1;
    document.querySelectorAll("#tab-row .tab").forEach((b) => {
        const on = b.dataset.tab === tab;
        b.classList.toggle("active", on);
        b.setAttribute("aria-selected", on ? "true" : "false");
    });
    load();
}

function boot() {
    const shellTheme = parentShellLight();
    applyTheme(shellTheme !== null ? (shellTheme ? "light" : "dark") : localStorage.getItem("fd-theme") || "dark");
    watchShellTheme();
    $("theme-toggle").onclick = () => {
        const next = currentTheme() === "dark" ? "light" : "dark";
        applyTheme(next);
        localStorage.setItem("fd-theme", next);
    };

    lang = localStorage.getItem("fd-lang") || "zh";
    $("lang-select").value = lang;
    applyI18n();
    refreshCreateLinks();
    $("lang-select").onchange = () => {
        lang = $("lang-select").value;
        localStorage.setItem("fd-lang", lang);
        applyI18n();
        refreshCreateLinks();
        load();
    };

    document.querySelectorAll("#tab-row .tab").forEach((b) => {
        b.onclick = () => setTab(b.dataset.tab);
    });

    $("sort-select").onchange = () => {
        state.sort = $("sort-select").value;
        state.page = 1;
        load();
    };

    $("search-toggle").onclick = () => {
        const input = $("search-input");
        input.classList.toggle("hidden");
        if (!input.classList.contains("hidden")) input.focus();
    };
    let searchTimer = null;
    $("search-input").addEventListener("input", (ev) => {
        clearTimeout(searchTimer);
        const v = ev.target.value.trim();
        searchTimer = setTimeout(() => {
            state.q = v;
            state.page = 1;
            load();
        }, 250);
    });

    $("prev-page").onclick = () => {
        if (state.page > 1) {
            state.page -= 1;
            load();
        }
    };
    $("next-page").onclick = () => {
        state.page += 1;
        load();
    };

    load();
}

boot();
