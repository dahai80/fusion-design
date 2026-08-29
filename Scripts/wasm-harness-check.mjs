// OPS-11：WASM E2E harness headless 校验（CI ubuntu + 本地 macOS）。
// 复用 docs/harness/ 8 用例（harness.js 加载即 runAll，写 #summary DOM）。
// 用 Playwright headless chromium 加载 index.html，等 #summary 填充，
// 抓文本断言全 PASS（无 FAIL/failed）。失败时输出 #summary + console log 便于定位。
//
// 前置（CI/本地）：
//   1. cargo build -p fd-host-web --target wasm32-unknown-unknown
//   2. wasm-bindgen --target web --out-dir docs/harness/pkg \
//        target/wasm32-unknown-unknown/debug/fd_host_web.wasm
//   3. npx playwright install --with-deps chromium
//
// 运行：node Scripts/wasm-harness-check.mjs
// 退出码：0 = 全 PASS；1 = 有 FAIL 或加载超时。

import { chromium } from "playwright";
import http from "node:http";
import fs from "node:fs";
import path from "node:path";

const HARNESS_DIR = path.resolve("docs/harness");
const SUMMARY_TIMEOUT_MS = 30_000;

function fail(msg) {
    console.error(`[wasm-harness-check] FAIL: ${msg}`);
    process.exit(1);
}

// file:// 下 ES module import 被 CORS 拦，起最小静态 http server 服 docs/harness。
const MIME = {
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".wasm": "application/wasm",
    ".json": "application/json",
    ".css": "text/css",
};
const server = http.createServer((req, res) => {
    const rel = decodeURIComponent(req.url.split("?")[0]).replace(/^\//, "");
    const file = path.join(HARNESS_DIR, rel || "index.html");
    if (!file.startsWith(HARNESS_DIR) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
        res.writeHead(404); res.end("not found"); return;
    }
    res.writeHead(200, { "Content-Type": MIME[path.extname(file)] || "application/octet-stream" });
    fs.createReadStream(file).pipe(res);
});
const port = await new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
const url = `http://127.0.0.1:${port}/index.html`;
console.log(`[wasm-harness-check] 静态服务 127.0.0.1:${port} → ${HARNESS_DIR}`);

const browser = await chromium.launch({ headless: true }).catch((e) =>
    fail(`chromium 启动失败（未装？npx playwright install --with-deps chromium）: ${e}`)
);
const page = await browser.newPage();

const consoleLogs = [];
page.on("console", (msg) => consoleLogs.push(`[${msg.type()}] ${msg.text()}`));
page.on("pageerror", (err) => consoleLogs.push(`[pageerror] ${err}`));

console.log(`[wasm-harness-check] 加载 ${url}`);
await page.goto(url, { waitUntil: "load" }).catch((e) => {
    console.error(`[wasm-harness-check] console 日志:\n${consoleLogs.join("\n")}`);
    fail(`加载 index.html 失败: ${e}`);
});

// 等 #summary 填充（harness.js runAll 完成后写「N passed, M failed」）。
try {
    await page.waitForFunction(
        () => {
            const el = document.getElementById("summary");
            const t = el ? el.textContent : "";
            return /\d+ passed, \d+ failed/.test(t);
        },
        { timeout: SUMMARY_TIMEOUT_MS }
    );
} catch (e) {
    console.error(`[wasm-harness-check] #summary 超时未填充（${SUMMARY_TIMEOUT_MS}ms）`);
    console.error(`[wasm-harness-check] console 日志:\n${consoleLogs.join("\n")}`);
    await browser.close();
    fail(`harness 未在超时内完成: ${e}`);
}

const summary = await page.textContent("#summary");
console.log(`[wasm-harness-check] #summary: ${summary}`);

// 抓完整日志（含每个 CASE 的 PASS/FAIL）便于 CI 失败定位。
const logText = await page.textContent("#log");
console.log(`[wasm-harness-check] harness 日志:\n${logText}`);

await browser.close();
server.close();

// 断言全 PASS：
//   - summary 形如「N passed, 0 failed」且 N>0（mount 成功才有用例跑）
//   - 日志无「mount 失败」「FAIL」（mount 失败会 0 passed 0 failed，须显式拦）
const m = summary.match(/(\d+) passed, (\d+) failed/);
if (!m) fail(`#summary 格式异常: ${summary}`);
const passed = parseInt(m[1], 10);
const failedCount = parseInt(m[2], 10);
if (passed === 0) fail(`harness 0 用例通过（疑似 mount 失败）: summary=${summary}`);
if (failedCount !== 0) {
    console.error(`[wasm-harness-check] console 日志:\n${consoleLogs.join("\n")}`);
    fail(`harness 有 ${failedCount} 个失败用例: summary=${summary}`);
}
if (/mount 失败|FAIL/.test(logText)) fail(`harness 日志含失败标记: ${logText}`);
console.log(`[wasm-harness-check] PASS: ${passed} 用例全通过`);
process.exit(0);
