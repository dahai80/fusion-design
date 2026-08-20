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

## 安装
  sudo tar -xzf ${PKG}.tar.gz -C /usr/local
  ln -sf /usr/local/${PKG}/fusion-design /usr/local/bin/fusion-design

## 前置
  启动 fusion-mlx 本地推理（127.0.0.1:11434）或 fusion-gateway（11432）。
  经 FUSION_MLX_BASE_URL 覆盖 endpoint；FUSION_MLX_API_KEY 设鉴权 key。

## 验证
  fusion-design --version
  fusion-design check-mlx
EOF

echo "==> 打包 tar.gz"
tar -czf "$DIST/${PKG}.tar.gz" -C "$DIST" "$PKG"

echo "==> 完成: dist/${PKG}.tar.gz"
ls -lh "$DIST/${PKG}.tar.gz"
