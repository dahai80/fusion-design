# 部署指南

> Fusion-Design 三渠道部署。100% 离线——HTTP 仅至 `127.0.0.1` / 私有段。

## 渠道概览

Fusion-Design 经三渠道分发，各自分发 + 校验方式不同：

1. **主渠道——fusion-studio DMG**（macOS 桌面）：已由 fusion-studio 自身发布管线 codesign + 公证。主分发方式；`fd-cli` 内嵌其中。
2. **CLI tarball**（独立）：`dist/fusion-design-<version>-aarch64-apple-darwin.tar.gz`，经 `Scripts/build.sh` 产出。独立 CLI + WASM 件。未签名时受 Gatekeeper 限（见 OPS-7）。
3. **WASM 件**（内嵌）：`fd_host_web_bg.wasm` + `fd_host_web.js`，fusion-studio WKWebView 从 `target/wasm32-unknown-unknown/` 拉取。

## 前置依赖

- **工具链**：Rust `1.94`（见 `rust-toolchain.toml`）。用 `~/.cargo/bin/cargo +1.94`——Homebrew `/opt/homebrew/bin/cargo` 是 1.96，忽略 `rust-toolchain.toml`。
- **WASM target**：`rustup target add wasm32-unknown-unknown`（渠道 2 + 3 需要）。
- **wasm-bindgen-cli**：`0.2.126`（固定，须匹配 `wasm-bindgen` runtime 版本）。装：`cargo +1.94 install wasm-bindgen-cli --version 0.2.126`。缺则 build.sh warn 跳过 bindgen 后处理（studio 回退陈旧 wasm）。
- **fusion-mlx**：本地推理 `127.0.0.1:11434`，或 fusion-gateway `127.0.0.1:11432`。启动：`~/claude-home/fusion-mlx/start.sh start`。
- **100% 离线**：无云端 API 调用。HTTP 仅至 `127.0.0.1` + 私有段（RFC1918 + 链路本地）。

## 渠道 1——fusion-studio DMG

主分发方式。`fd-cli` 内嵌在已签名公证的 DMG 中（fusion-studio 自身发布管线 `Scripts/build.sh` 产出）。

**部署**：
1. 从 GitHub Release 下载 `fusion-studio-<version>.dmg`。
2. 打开，拖 `Fusion Studio.app` 到 `Applications`。
3. 启动——app 内嵌 `fd_host_web_bg.wasm` + `fd_host_web.js`（studio 构建时从本仓 `target/wasm32-unknown-unknown/` 拉取）+ `fusion-design` CLI 二进制。
4. CLI 在 app bundle 内；终端用需把 bundle 的 `Contents/MacOS` 加 `PATH`，或用 `fusion-studio` 的 CLI shim。

**校验**：app 启动正常，无 Gatekeeper「未识别开发者」提示（已公证）。若 shim 配好，终端 `fusion-design --version` 可用。

**本仓无需操作**渠道 1——本仓产出 WASM + CLI 件供 fusion-studio 构建消费。

## 渠道 2——CLI tarball

独立 CLI + WASM 件，经 `Scripts/build.sh` 产出 → `dist/fusion-design-<version>-aarch64-apple-darwin.tar.gz`。

**构建**：
```bash
./Scripts/build.sh
# 产物: dist/fusion-design-<version>-aarch64-apple-darwin.tar.gz
# 含: fusion-design CLI + fd_host_web_bg.wasm + fd_host_web.js + VERSION/README.md/INSTALL.md
```

**部署**（见 INSTALL.md）：
```bash
sudo tar -xzf fusion-design-<version>-aarch64-apple-darwin.tar.gz -C /usr/local
ln -sf /usr/local/fusion-design-<version>-aarch64-apple-darwin/fusion-design /usr/local/bin/fusion-design
```

**完整性校验**（OPS-7 SHA256，无需 Apple secret）：
```bash
shasum -a 256 fusion-design-<version>-aarch64-apple-darwin.tar.gz > fusion-design-<version>-aarch64-apple-darwin.tar.gz.sha256   # 生成端
shasum -a 256 -c fusion-design-<version>-aarch64-apple-darwin.tar.gz.sha256                                                       # 校验端
```

**Gatekeeper**：**未签名**独立 tarball 二进制首跑受 Gatekeeper 限——用户须右键 → 打开，或 `xattr -dr com.apple.quarantine /usr/local/fusion-design-<version>-*/fusion-design`。已签名 tarball（OPS-7，配好 Apple secret 时）免此限。主渠道（fusion-studio DMG）已签名公证，是多数用户的推荐路径。

## 渠道 3——WASM 件

`fd_host_web_bg.wasm` + `fd_host_web.js` 内嵌在 fusion-studio WKWebView。非独立分发——studio 构建时消费。

**构建**：
```bash
cargo +1.94 build -p fd-host-web --release --target wasm32-unknown-unknown
# wasm-bindgen 后处理（build.sh 已含此步，独立构建时手动跑）:
wasm-bindgen --target web --out-dir target/wasm32-unknown-unknown/release \
    target/wasm32-unknown-unknown/release/fd_host_web.wasm
# 产物: fd_host_web_bg.wasm + fd_host_web.js
```

**Studio 同步**：fusion-studio 的 `Scripts/build.sh` 从本仓 `target/wasm32-unknown-unknown/{release,debug}/` 拉 `fd_host_web_bg.wasm` + `fd_host_web.js`。本仓 `build.sh` 跑 bindgen 后处理，确保 `_bg.wasm` 落该目录——缺此步 studio 同步脚本找不到 `_bg.wasm`，回退陈旧内置件。

**本地 harness 测试**（无需 studio）：见 `docs/harness/`（8 用例，页面加载自动跑）+ `Scripts/wasm-harness-check.mjs`（headless Playwright runner，OPS-11）。

## OPS-7——tarball 签名（可选）

`Scripts/build.sh` 含 env-gated 签名块。**无 Apple secret → warn + skip，仍产出未签名 tarball（构建不阻断）。**

**所需 secret**（加到 GitHub repo Settings → Secrets，或本地 export）：
- `APPLE_DEV_ID`——Apple ID 邮箱（开发者账号）
- `APP_SPECIFIC_PW`——app 专用密码（appleid.apple.com）
- `TEAM_ID`——开发者 Team ID

**三者齐时** build.sh 跑：
1. `codesign --force --options runtime --sign "$APPLE_DEV_ID" <binary>`
2. `xcrun notarytool submit <tarball> --apple-id ... --password ... --team-id ... --wait`
3. `xcrun stapler staple <binary>`

**缺任一** → stderr：`[warn] OPS-7: 跳过签名（缺 APPLE_DEV_ID/APP_SPECIFIC_PW/TEAM_ID secret），独立 tarball 受 Gatekeeper 限`，继续出未签名 tarball。

SHA256 校验件（`.tar.gz.sha256`）**无论是否签名都生成**——无需 secret。

**CI 启用**：`.github/workflows/ci.yml` release-pack job 以 `env:` 注入三 secret。加到 repo secrets 后下次 tag 触发构建即启用签名。

## 回滚

- **CLI tarball**：保留上版 tarball。回滚即重新解压：`sudo tar -xzf fusion-design-<prev-version>-*.tar.gz -C /usr/local` + 重建符号链接。无状态迁移（`.fusiondesign` 是 JSON，同 schema 版本内前后兼容）。
- **fusion-studio DMG**：保留上版 DMG，替换 `Applications` 里的 app。
- **WASM 件**：上版 `fd_host_web_bg.wasm` 在 git 历史 + 既往 tarball 中；从上版 tag 重建。
- **基于 tag**：每个 release 是 git tag（`v0.1.x`）。`git checkout v0.1.<prev> && ./Scripts/build.sh` 复现任意既往 tarball。

## 验证清单

打 release tag 前确认：

- [ ] `~/.cargo/bin/cargo +1.94 test --workspace`——0 failed
- [ ] `~/.cargo/bin/cargo +1.94 clippy --workspace --all-targets -- -D warnings`——clean
- [ ] `~/.cargo/bin/cargo +1.94 fmt --all -- --check`——0 diff
- [ ] `Scripts/deny-panic.sh` + `Scripts/deny-unwrap-expect.sh` + `Scripts/deny-external-http.sh`——过
- [ ] `~/.cargo/bin/cargo +1.94 build -p fd-host-web --release --target wasm32-unknown-unknown` + wasm-bindgen——clean
- [ ] `./Scripts/build.sh`——产出 `dist/fusion-design-<version>-*.tar.gz` + `.sha256`
- [ ] `shasum -a 256 -c dist/*.sha256`——OK
- [ ] 解压 tarball，跑 `fusion-design --version` + `fusion-design check-mlx`（MLX 运行中）
- [ ] `node Scripts/wasm-harness-check.mjs`——8 harness 用例 PASS（OPS-11）
- [ ] CI 在 tag 上绿（`gh run watch --exit-status`）
- [ ] 若启用签名：`spctl --assess --verbose <binary>` 显 "notarized"
