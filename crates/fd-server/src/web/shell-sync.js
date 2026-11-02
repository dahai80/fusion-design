// 跟随 fusion-web-shell 统一主题/语言（spec §2.1：Shell 管外壳，模块不实现、不复刻）。
// 嵌入时读外壳 html.light / documentElement.lang，并监听外壳 class/lang 属性变化
// 与 fusion:theme / fusion:lang 事件；独立打开时回退 shell 约定的
// localStorage 键（fusion.shell.theme / fusion.shell.lang），默认 dark / zh。
"use strict";

function shellDoc() {
    try {
        return window.parent !== window ? window.parent.document : null;
    } catch {
        return null;
    }
}

function shellWin() {
    try {
        return window.parent !== window ? window.parent : null;
    } catch {
        return null;
    }
}

function normLang(l) {
    if (!l) return null;
    return String(l).toLowerCase().startsWith("en") ? "en" : "zh";
}

// 应用外壳状态；onChange(theme, lang) 在初始与每次外壳切换时调用。
export function syncShell(onChange) {
    const apply = () => {
        const doc = shellDoc();
        const theme = doc
            ? (doc.documentElement.classList.contains("light") ? "light" : "dark")
            : (localStorage.getItem("fusion.shell.theme") || "dark");
        const lang = normLang(doc ? doc.documentElement.lang : null)
            || normLang(localStorage.getItem("fusion.shell.lang"))
            || "zh";
        onChange(theme, lang);
    };
    apply();
    const doc = shellDoc();
    if (doc) {
        new MutationObserver(apply).observe(doc.documentElement, {
            attributes: true,
            attributeFilter: ["class", "lang"],
        });
        const win = shellWin();
        if (win) {
            win.addEventListener("fusion:theme", apply);
            win.addEventListener("fusion:lang", apply);
        }
    }
}
