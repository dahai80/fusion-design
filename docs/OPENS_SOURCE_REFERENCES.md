# Open-Source References — Fusion-Design 开源软件参考清单

> 核实日期：2026-07-16。所有「本地状态」均经 `ls`/`find` 实际验证，不臆造。

## 一、推荐组合（V0.1 最终选型）

**底座决策已变更**：放弃 PRD 原方案的「tldraw + OpenUI + Plasmic 拼装」，改用 **OpenPencil (Rust) 一体化底座**。

理由：OpenPencil 原生 Rust（与 fusion-mlx 生态对齐）、自带画布+AI+代码导出+MCP+Figma 导入一体化 crate，裁剪成本远低于 3 库手工拼接。

| 角色 | 选型 | 本地路径 | 状态 |
|------|------|---------|------|
| 主底座（画布+AI+代码导出+MCP） | **OpenPencil** | `~/design/openpencil` | ✅ 完整（1656 文件，Rust workspace） |
| 备用/补充参考 | Penpot | `~/design/penpot` | ✅ 完整（5885 文件，Clojure+ClojureScript） |
| 备用/补充参考 | Plasmic | `~/design/plasmic` | ✅ 完整（7674 文件，TypeScript） |
| 备用/补充参考 | OpenUI | `~/design/openui` | ❌ **本地损坏**（git HEAD 失效，工作树空） |
| 未下载（按需克隆） | tldraw | — | 📦 提供 clone 命令 |
| 未下载（按需克隆） | Superdesign | — | 📦 提供 clone 命令 |
| 未下载（按需克隆） | Stitches | — | 📦 提供 clone 命令 |

---

## 二、主底座：OpenPencil（已确认完整）

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/ZSeven-W/openpencil |
| 本地路径 | `~/design/openpencil` |
| 文件数 | 1656 |
| 语言 | Rust（workspace，edition 2021） |
| 版本 | 0.8.1（Cargo.toml） |
| License | 见仓库 LICENSE |
| 一句话 | "The world's first open-source AI-native vector design tool" — Concurrent Agent Teams · Design-as-Code · Built-in MCP Server · Multi-model Intelligence |

### 可直接复用的 crate（`~/design/openpencil/crates/`）

| Crate | 用途 | Fusion-Design 模块 |
|-------|------|-------------------|
| `op-editor-core` | 矢量画布内核 | 模块 1 |
| `op-ai` | AI 调用抽象（**后端需替换为 fusion-mlx**） | 模块 2 桥接层 |
| `op-ai-skills` | AI 设计技能定义 | 模块 2 文生 UI / 图生 UI |
| `op-codegen` | 代码生成 | 模块 4 导出 React/HTML/Tailwind |
| `op-design-lint` | 设计规范检查 | 模块 3 设计系统约束 |
| `op-mcp` | MCP 协议 Server | 模块 6 生态联动 |
| `op-figma` | Figma 文件解析 | V0.2 Figma 导入 |
| `op-host-desktop` | 桌面宿主 | 嵌入 Fusion-Desk WKWebView 的桥 |
| `op-host-web` | Web 宿主 (wasm) | 编译 wasm 供 WKWebView 加载 |
| `op-host-web-server` | 本地 web 服务 | 本地私有接口 |
| `op-cli` | 命令行 | 模块 6 Fusion CLI 联动 |
| `op-config-store` | 配置存储 | 设计 Token / 工程配置 |
| `op-i18n` | 国际化 | 中文支持 |
| `op-git` | Git 集成 | `.fusiondesign` 版本管理 |

### 改造要点

1. **剥离云端模型**：`op-ai` 中所有 Anthropic/OpenAI/云端 API 调用代码删除，替换为 fusion-mlx 本地推理接口
2. **剥离协作/账号**：删除 agent teams 的多人协作逻辑（V0.1 单人）
3. **裁剪 host**：保留 `op-host-web` (wasm) + `op-host-web-server`，删除 `op-host-desktop` 的独立桌面壳（改为嵌入 Fusion-Desk）
4. **禁用外网**：删除所有 CDN、更新检测、遥测、崩溃上报

---

## 三、备用参考项目

### 3.1 Penpot（已确认完整）

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/penpot/penpot |
| 本地路径 | `~/design/penpot` |
| 文件数 | 5885 |
| 语言 | Clojure（后端）+ ClojureScript（前端） |
| License | MPL-2.0 |
| 用途参考 | Flex/Grid 布局计算、组件实例、样式继承逻辑（移植到 OpenPencil 补齐专业布局） |
| 不整包集成理由 | 工程庞大、依赖复杂、自带完整后端/数据库/协作系统，冗余过多 |

### 3.2 Plasmic（已确认完整）

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/plasmicapp/plasmic |
| 本地路径 | `~/design/plasmic` |
| 文件数 | 7674 |
| 语言 | TypeScript |
| 用途参考 | 图层树解析、Tailwind/React 代码生成逻辑（若 `op-codegen` 不够用，移植补充） |
| 不整包集成理由 | 低代码平台、云端托管、团队协作冗余；OpenPencil `op-codegen` 已覆盖核心 |

### 3.3 OpenUI（本地损坏）

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/wandb/openui |
| 本地路径 | `~/design/openui` |
| 状态 | ❌ **损坏** — git HEAD 失效，工作树空（`git log` 报「当前分支似乎已损坏」） |
| 修复命令 | `cd ~/design && rm -rf openui && git clone https://github.com/wandb/openui.git` |
| 用途参考 | 自然语言转 UI、局部编辑、多框架代码导出（**OpenPencil `op-ai-skills` 已覆盖，可不解**） |

---

## 四、未下载项目（按需克隆）

### 4.1 tldraw（轻量化无限画布）

```bash
cd ~/design && git clone --depth 1 https://github.com/tldraw/tldraw.git
```
用途：若 OpenPencil 的 wasm 画布在 WKWebView 中体积过大，参考 tldraw 的轻量化渲染优化。

### 4.2 Superdesign（IDE 配套 AI 设计工具）

```bash
cd ~/design && git clone --depth 1 https://github.com/superdesignai/superdesign.git
```
用途：组件化 AI 生成、多版本设计对比、IDE 代码联动交互参考。

### 4.3 Stitches（CSS 设计 Token）

```bash
cd ~/design && git clone --depth 1 https://github.com/modulz/stitches.git
```
用途：若 OpenPencil `op-design-lint` 的 Token 管理不足，参考 Stitches 的全局样式 Token 统一管控逻辑。

---

## 五、全仓库批量克隆脚本（一次性拉取）

```bash
# 创建根目录并进入
mkdir -p ~/design && cd ~/design

# 批量克隆所有仓库（--depth 1 仅拉最新提交，省空间省时间）
git clone --depth 1 https://github.com/penpot/penpot.git
git clone --depth 1 https://github.com/tldraw/tldraw.git
git clone --depth 1 https://github.com/ZSeven-W/openpencil.git
git clone --depth 1 https://github.com/wandb/openui.git
git clone --depth 1 https://github.com/superdesignai/superdesign.git
git clone --depth 1 https://github.com/modulz/stitches.git
git clone --depth 1 https://github.com/plasmicapp/plasmic.git
```

> 注：当前网络 github.com 连接超时（实测 `git clone` 失败），建议网络恢复后执行。已下载的 4 个项目（openpencil/penpot/plasmic 完整，openui 损坏需重新克隆）。

---

## 六、选型决策矩阵

| 能力需求 | 首选 | 备用 | 决策 |
|---------|------|------|------|
| 矢量画布内核 | OpenPencil `op-editor-core` | tldraw | OpenPencil（Rust 对齐，一体化） |
| AI 文生 UI | OpenPencil `op-ai`+`op-ai-skills` | OpenUI | OpenPencil（OpenUI 损坏且功能已覆盖） |
| 设计系统/Token | OpenPencil `op-design-lint` | Stitches | OpenPencil（待验证，不足则移植 Stitches） |
| 代码导出 | OpenPencil `op-codegen` | Plasmic | OpenPencil（不足则移植 Plasmic 解析逻辑） |
| Figma 导入 | OpenPencil `op-figma` | — | OpenPencil（V0.2 启用） |
| MCP 生态联动 | OpenPencil `op-mcp` | — | OpenPencil（原生支持） |
| Flex/Grid 布局 | OpenPencil | Penpot | OpenPencil（不足则移植 Penpot 布局计算） |

**结论**：OpenPencil 一库覆盖 6/7 能力，唯一备用场景是「Flex/Grid 布局」可能需移植 Penpot 逻辑。
