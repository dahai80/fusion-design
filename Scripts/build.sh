#!/usr/bin/env bash
# Fusion-Design 发布打包脚本。
# 产物：dist/fusion-design-<version>-aarch64-apple-darwin.tar.gz
#   - fusion-design CLI 二进制（release，strip+lto）
#   - fd_host_web_bg.wasm + fd_host_web.js（WKWebView 前端，wasm-bindgen --target web 产物）
#   - VERSION / README.md
set -euo pipefail

# 确保 rustup/wasm target 可用（CI/非交互 shell 可能未载入 cargo env）
export PATH="$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*= *"([^"]+)".*/\1/')
DIST="$ROOT/dist"
PKG="fusion-design-${VERSION}-aarch64-apple-darwin"
STAGE="$DIST/${PKG}"

echo "==> 版本: ${VERSION}"
rm -rf "$STAGE" "$DIST/${PKG}.tar.gz"
mkdir -p "$STAGE"

echo "==> 编译 release CLI 二进制"
cargo build --release -p fd-cli
cp "target/release/fusion-design" "$STAGE/fusion-design"
chmod +x "$STAGE/fusion-design"

echo "==> 编译 fd-host-web wasm"
cargo build -p fd-host-web --release --target wasm32-unknown-unknown
# wasm-bindgen 后处理：cargo 只产出原始 fd_host_web.wasm，
# 而 WKWebView 集成（fusion-studio）需 wasm-bindgen --target web 产物
# fd_host_web_bg.wasm + fd_host_web.js。缺此步则 studio 同步脚本找不到 _bg.wasm，
# 回退到陈旧内置件——本步补齐 bindgen 产物到 target 目录供 studio 拉取。
WASM_PROFILE_DIR="target/wasm32-unknown-unknown/release"
if ! command -v wasm-bindgen >/dev/null 2>&1; then
    echo "  [warn] wasm-bindgen 未安装，跳过 bindgen 后处理（studio 将用陈旧 wasm）"
else
    wasm-bindgen --target web --out-dir "$WASM_PROFILE_DIR" \
        "$WASM_PROFILE_DIR/fd_host_web.wasm"
    echo "  已生成 fd_host_web_bg.wasm + fd_host_web.js"
fi
# 打包用：优先 bindgen 产物（_bg.wasm），回退原始 wasm。
if [ -f "$WASM_PROFILE_DIR/fd_host_web_bg.wasm" ]; then
    cp "$WASM_PROFILE_DIR/fd_host_web_bg.wasm" "$STAGE/fd_host_web_bg.wasm"
    [ -f "$WASM_PROFILE_DIR/fd_host_web.js" ] && \
        cp "$WASM_PROFILE_DIR/fd_host_web.js" "$STAGE/fd_host_web.js"
else
    cp "$WASM_PROFILE_DIR/fd_host_web.wasm" "$STAGE/fd_host_web.wasm"
fi

echo "==> 附带元信息"
echo "${VERSION}" > "$STAGE/VERSION"
cp "$ROOT/README.md" "$STAGE/README.md" 2>/dev/null || echo "（无 README.md，跳过）"
cat > "$STAGE/INSTALL.md" <<EOF
# Fusion-Design ${VERSION} (Apple Silicon, offline)

## 校验完整性（可选，tarball 同目录附 .sha256）
  shasum -a 256 -c ${PKG}.tar.gz.sha256

## 安装
  sudo tar -xzf ${PKG}.tar.gz -C /usr/local
  ln -sf /usr/local/${PKG}/fusion-design /usr/local/bin/fusion-design

## 前置
  启动 fusion-mlx 本地推理（127.0.0.1:11434）或 fusion-gateway（11432）。
  经 FUSION_MLX_BASE_URL 覆盖 endpoint；FUSION_MLX_API_KEY 设鉴权 key。

## Gatekeeper 说明
  独立 tarball 未签名公证时，首次运行可能被 Gatekeeper 拦截：
  xattr -d com.apple.quarantine /usr/local/${PKG}/fusion-design
  主渠道经 fusion-studio DMG（已签名公证）安装则无此限。

## 验证
  fusion-design --version
  fusion-design check-mlx
EOF

echo "==> 打包 tar.gz"
tar -czf "$DIST/${PKG}.tar.gz" -C "$DIST" "$PKG"

# OPS-7：SHA256 校验件（无需 Apple secret，始终生成）。
# 在 $DIST 内对相对文件名跑 shasum，否则 .sha256 记录绝对路径，
# 用户解压后 shasum -c 因路径不存在而失败（已发布件路径为 CI /Users/runner/...）。
( cd "$DIST" && shasum -a 256 "${PKG}.tar.gz" ) > "$DIST/${PKG}.tar.gz.sha256"
echo "  已生成 ${PKG}.tar.gz.sha256"
echo "  校验：shasum -a 256 -c ${PKG}.tar.gz.sha256"

# OPS-7：可选全签名管线（env-gated，缺 secret 时 warn+skip 不阻断）。
# 主渠道 fd-cli 继承 fusion-studio 已签名公证容器；独立 tarball 受 Gatekeeper 限。
# 三个 secret 齐全 → codesign + notarytool + stapler + 签后真验；缺任一 → 跳过出未签名件。
# secret 齐全时签名/公证/装订任一失败 = 致命（outward-facing 交付物失败须红，对齐
# ci.yml release-pack 注释「release 是 outward-facing 交付物，失败必须红」）。
# 无 secret 时 warn+skip 出未签名件不阻断——本行为正确，不动。
if [ -n "${APPLE_DEV_ID:-}" ] && [ -n "${APP_SPECIFIC_PW:-}" ] && [ -n "${TEAM_ID:-}" ]; then
    echo "==> OPS-7: 签名管线启用（APPLE_DEV_ID/APP_SPECIFIC_PW/TEAM_ID 齐全）"
    BIN="$STAGE/fusion-design"
    echo "  [1/3] codesign --options runtime"
    if codesign --force --options runtime --sign "$APPLE_DEV_ID" "$BIN"; then
        echo "    codesign OK"
    else
        echo "    [error] codesign 失败，签名管线中止（outward-facing 交付物失败须红）" >&2
        exit 1
    fi
    echo "  [2/3] notarytool submit --wait"
    if xcrun notarytool submit "$DIST/${PKG}.tar.gz" \
        --apple-id "$APPLE_DEV_ID" --password "$APP_SPECIFIC_PW" \
        --team-id "$TEAM_ID" --wait; then
        echo "    notarytool OK"
    else
        echo "    [error] notarytool 失败（网络/凭证问题），签名管线中止" >&2
        exit 1
    fi
    echo "  [3/3] stapler staple"
    if xcrun stapler staple "$BIN"; then
        echo "    stapler OK"
    else
        echo "    [error] stapler 失败，签名管线中止（已签名未装订不可交付）" >&2
        exit 1
    fi
    # 签后真验：codesign --verify --strict 验签名完整性，spctl --assess 验 Gatekeeper
    # 公证装订。任一失败 = 伪签名件，致命（闭合 P3 伪签名缺口：原 warn 回退会出伪签件）。
    echo "  [verify] codesign --verify --strict"
    if codesign --verify --strict --verbose=2 "$BIN" 2>&1; then
        echo "    codesign verify OK"
    else
        echo "    [error] codesign verify 失败（签名无效或被篡改）" >&2
        exit 1
    fi
    echo "  [verify] spctl --assess (Gatekeeper)"
    if spctl --assess --type execute -vv "$BIN" 2>&1; then
        echo "    spctl assess OK"
    else
        echo "    [error] spctl assess 失败（公证未装订/未通过 Gatekeeper）" >&2
        exit 1
    fi
    # 签名+真验后重新打包（codesign 改了二进制）+ 刷新 SHA256。
    tar -czf "$DIST/${PKG}.tar.gz" -C "$DIST" "$PKG"
    ( cd "$DIST" && shasum -a 256 "${PKG}.tar.gz" ) > "$DIST/${PKG}.tar.gz.sha256"
    echo "  签名+真验后重打包 + 刷新 sha256"
else
    echo "==> OPS-7: [warn] 跳过签名（缺 APPLE_DEV_ID/APP_SPECIFIC_PW/TEAM_ID secret）" >&2
    echo "    独立 tarball 受 Gatekeeper 限，主渠道经 fusion-studio 已签名公证容器" >&2
fi

echo "==> 完成: dist/${PKG}.tar.gz"
ls -lh "$DIST/${PKG}.tar.gz"
