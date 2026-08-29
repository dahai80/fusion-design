# Deployment Guide

> Fusion-Design deployment across three distribution channels. 100% offline — HTTP only to `127.0.0.1` / private segments.

## Channels

Fusion-Design ships via three channels, each with distinct distribution + verification:

1. **Primary — fusion-studio DMG** (macOS desktop): already codesigned + notarized by fusion-studio's release pipeline. This is the main distribution; `fd-cli` is bundled inside.
2. **CLI tarball** (independent): `dist/fusion-design-<version>-aarch64-apple-darwin.tar.gz`, built by `Scripts/build.sh`. Standalone CLI + WASM assets. Gatekeeper-limited when unsigned (see OPS-7).
3. **WASM assets** (embedded): `fd_host_web_bg.wasm` + `fd_host_web.js`, pulled by fusion-studio WKWebView from `target/wasm32-unknown-unknown/`.

## Prerequisites

- **Toolchain**: Rust `1.94` (see `rust-toolchain.toml`). Use `~/.cargo/bin/cargo +1.94` — Homebrew `/opt/homebrew/bin/cargo` is 1.96 and ignores `rust-toolchain.toml`.
- **WASM target**: `rustup target add wasm32-unknown-unknown` (for channels 2 + 3).
- **wasm-bindgen-cli**: `0.2.126` (pinned, must match `wasm-bindgen` runtime version). Install: `cargo +1.94 install wasm-bindgen-cli --version 0.2.126`. Missing it → build.sh warns and skips bindgen post-step (studio falls back to stale wasm).
- **fusion-mlx**: local inference at `127.0.0.1:11434`, or fusion-gateway at `127.0.0.1:11432`. Start: `~/claude-home/fusion-mlx/start.sh start`.
- **100% offline**: no cloud API calls. HTTP only to `127.0.0.1` + private segments (RFC1918 + link-local).

## Channel 1 — fusion-studio DMG

Primary distribution. `fd-cli` is bundled inside the signed + notarized DMG produced by fusion-studio's own release pipeline (`Scripts/build.sh` in `fusion-studio`).

**Deploy**:
1. Download `fusion-studio-<version>.dmg` from the GitHub Release.
2. Open, drag `Fusion Studio.app` to `Applications`.
3. Launch — the app embeds `fd_host_web_bg.wasm` + `fd_host_web.js` (pulled from this repo's `target/wasm32-unknown-unknown/` at studio build time) and the `fusion-design` CLI binary.
4. CLI is exposed inside the app bundle; for terminal use add the bundle's `Contents/MacOS` to `PATH` or use `fusion-studio`'s CLI shim.

**Verification**: app launches, no Gatekeeper "unidentified developer" prompt (already notarized). `fusion-design --version` works from terminal if shim configured.

**No action needed in this repo** for channel 1 — this repo produces the WASM + CLI artifacts that fusion-studio's build consumes.

## Channel 2 — CLI tarball

Standalone CLI + WASM assets, built by `Scripts/build.sh` → `dist/fusion-design-<version>-aarch64-apple-darwin.tar.gz`.

**Build**:
```bash
./Scripts/build.sh
# 产物: dist/fusion-design-<version>-aarch64-apple-darwin.tar.gz
# 含: fusion-design CLI + fd_host_web_bg.wasm + fd_host_web.js + VERSION/README.md/INSTALL.md
```

**Deploy** (from INSTALL.md):
```bash
sudo tar -xzf fusion-design-<version>-aarch64-apple-darwin.tar.gz -C /usr/local
ln -sf /usr/local/fusion-design-<version>-aarch64-apple-darwin/fusion-design /usr/local/bin/fusion-design
```

**Integrity check** (OPS-7 SHA256, no Apple secret needed):
```bash
shasum -a 256 fusion-design-<version>-aarch64-apple-darwin.tar.gz > fusion-design-<version>-aarch64-apple-darwin.tar.gz.sha256   # 生成端
shasum -a 256 -c fusion-design-<version>-aarch64-apple-darwin.tar.gz.sha256                                                       # 校验端
```

**Gatekeeper**: an **unsigned** standalone tarball binary is Gatekeeper-limited on first run — the user must right-click → Open, or `xattr -dr com.apple.quarantine /usr/local/fusion-design-<version>-*/fusion-design`. Signed tarballs (OPS-7, when Apple secrets configured) avoid this. The primary channel (fusion-studio DMG) is already signed + notarized and is the recommended path for most users.

## Channel 3 — WASM assets

`fd_host_web_bg.wasm` + `fd_host_web.js` embedded in fusion-studio's WKWebView. Not a standalone distribution — consumed at studio build time.

**Build**:
```bash
cargo +1.94 build -p fd-host-web --release --target wasm32-unknown-unknown
# wasm-bindgen 后处理（build.sh 已含此步，独立构建时手动跑）:
wasm-bindgen --target web --out-dir target/wasm32-unknown-unknown/release \
    target/wasm32-unknown-unknown/release/fd_host_web.wasm
# 产物: fd_host_web_bg.wasm + fd_host_web.js
```

**Studio sync**: fusion-studio's `Scripts/build.sh` pulls `fd_host_web_bg.wasm` + `fd_host_web.js` from this repo's `target/wasm32-unknown-unknown/{release,debug}/`. `build.sh` here runs the bindgen post-step so the `_bg.wasm` lands in that directory — without it studio's sync script falls back to a stale built-in copy.

**Local harness test** (no studio needed): see `docs/harness/` (8 cases, auto-run on page load) + `Scripts/wasm-harness-check.mjs` (headless Playwright runner, OPS-11).

## OPS-7 — Tarball signing (optional)

`Scripts/build.sh` includes an env-gated signing block. **No Apple secrets → warn + skip, unsigned tarball still produced (build not blocked).**

**Required secrets** (add to GitHub repo Settings → Secrets, or export locally):
- `APPLE_DEV_ID` — Apple ID email (Developer account)
- `APP_SPECIFIC_PW` — app-specific password (appleid.apple.com)
- `TEAM_ID` — Developer Team ID

**When all three set**, build.sh runs:
1. `codesign --force --options runtime --sign "$APPLE_DEV_ID" <binary>`
2. `xcrun notarytool submit <tarball> --apple-id ... --password ... --team-id ... --wait`
3. `xcrun stapler staple <binary>`

**When any missing** → stderr: `[warn] OPS-7: 跳过签名（缺 APPLE_DEV_ID/APP_SPECIFIC_PW/TEAM_ID secret），独立 tarball 受 Gatekeeper 限`, continues with unsigned tarball.

SHA256 checksum (`.tar.gz.sha256`) is **always** generated regardless of signing — no secret needed.

**Enable in CI**: `.github/workflows/ci.yml` release-pack job injects the three secrets as `env:`. Add them to repo secrets to activate signing on the next tag-triggered build.

## Rollback

- **CLI tarball**: keep the previous version's tarball. Roll back by re-extracting it: `sudo tar -xzf fusion-design-<prev-version>-*.tar.gz -C /usr/local` + re-create the symlink. No state migration needed (`.fusiondesign` files are JSON, forward/backward compatible within a schema version).
- **fusion-studio DMG**: keep the previous DMG, replace the app in `Applications`.
- **WASM assets**: the previous `fd_host_web_bg.wasm` lives in git history + prior tarballs; rebuild from the prior tag.
- **Tag-based**: every release is a git tag (`v0.1.x`). `git checkout v0.1.<prev> && ./Scripts/build.sh` reproduces any prior tarball.

## Verification checklist

Before tagging a release, confirm:

- [ ] `~/.cargo/bin/cargo +1.94 test --workspace` — 0 failed
- [ ] `~/.cargo/bin/cargo +1.94 clippy --workspace --all-targets -- -D warnings` — clean
- [ ] `~/.cargo/bin/cargo +1.94 fmt --all -- --check` — 0 diff
- [ ] `Scripts/deny-panic.sh` + `Scripts/deny-unwrap-expect.sh` + `Scripts/deny-external-http.sh` — pass
- [ ] `~/.cargo/bin/cargo +1.94 build -p fd-host-web --release --target wasm32-unknown-unknown` + wasm-bindgen — clean
- [ ] `./Scripts/build.sh` — produces `dist/fusion-design-<version>-*.tar.gz` + `.sha256`
- [ ] `shasum -a 256 -c dist/*.sha256` — OK
- [ ] Extract tarball, run `fusion-design --version` + `fusion-design check-mlx` (with MLX running)
- [ ] `node Scripts/wasm-harness-check.mjs` — 8 harness cases PASS (OPS-11)
- [ ] CI green on the tag (`gh run watch --exit-status`)
- [ ] If signing enabled: `spctl --assess --verbose <binary>` shows "notarized"
