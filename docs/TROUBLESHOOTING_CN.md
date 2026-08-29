# Fusion-Design 场景化排障手册

> [English](TROUBLESHOOTING.md) | [中文](TROUBLESHOOTING_CN.md)
>
> 按错误现象速查：现象 → 根因 → 解决。配套 [USER_GUIDE_CN.md](USER_GUIDE_CN.md)。

## 速查索引

| 现象 | 关键词 | 跳转 |
|------|--------|------|
| A | 连接失败 / 健康检查失败 / 服务没起 | [现象 A](#现象-amlx-服务不通连接失败--健康检查失败) |
| B | 502 / 503 / 模型加载中 / 卡住 | [现象 B](#现象-b生成返回-502--503模型加载中) |
| C | 假绿 / 列表有模型生成却失败 | [现象 C](#现象-ccheck-mlx-假绿列表有模型生成却失败) |
| D | 401 / API key / 鉴权 | [现象 D](#现象-d鉴权失败401--api-key-无效) |
| E | 流式断 / 乱码 / U+FFFD | [现象 E](#现象-e流式中途断--cjk-乱码ufffd) |
| F | 反序列化 / 超深 / 超限 | [现象 F](#现象-f反序列化-fusiondesign-报错超深--超限) |
| G | IPC / 消息丢失 / 路径遍历 | [现象 G](#现象-gipc-消息丢失--路径遍历拦截) |
| H | var(--*) 未解析 / Token 颜色 | [现象 H](#现象-h导出-token-颜色未解析var--出现在产物) |
| I | wasm / 前端加载 / studio 陈旧 | [现象 I](#现象-iwasm-前端加载失败--studio-同步陈旧) |
| J | 模型不存在 / 跨部署漂移 | [现象 J](#现象-j模型不存在--跨部署模型列表漂移) |
| K | lint 报告 / 13 规则含义 | [现象 K](#现象-klint-报告看不懂13-规则含义) |
| L | XSS / 注入 / 安全护栏 | [现象 L](#现象-l生成内容含-xss--注入安全护栏触发) |
| M | 无日志文件 / 现场诊断 / 文件日志 | [现象 M](#现象-m磁盘无日志文件--需现场诊断件) |

## 现象 A：MLX 服务不通（连接失败 / 健康检查失败）

**现象**：`fusion-design health` 或 `check-mlx` 报"连接失败 / connection refused / 服务未起"。

**根因**：fusion-mlx（11434）或 fusion-gateway（11432）未运行，或端口被占。

**排查**：

```bash
# 1. 查 fusion-mlx 状态
~/claude-home/fusion-mlx/start.sh status

# 2. 查端口
lsof -i :11434    # fusion-mlx
lsof -i :11432    # fusion-gateway

# 3. 直连探活
curl -s -m 5 http://127.0.0.1:11434/v1/models -H "Authorization: Bearer $FUSION_MLX_API_KEY" | head -c 100
```

**解决**：

```bash
# fusion-mlx 没起
~/claude-home/fusion-mlx/start.sh start
~/claude-home/fusion-mlx/start.sh doctor   # 起后做健康检查

# gateway 没起但 MLX 在 → CLI 改直连
export FUSION_MLX_BASE_URL=http://127.0.0.1:11434
fusion-design check-mlx
```

**预防**：用前先 `start.sh status` 确认；脚本里加 `health` 前置检查，非零即停。

## 现象 B：生成返回 502 / 503（模型加载中）

**现象**：`generate` / `chat` 返 502 或 503，或首次请求卡住 30s+。

**根因**：fusion-mlx 首次加载大模型需时间，加载期间返 503（"model loading"）；模型被驱逐后返 502。属瞬时错误，非永久故障。

**机制**：fd-ai-adapter **M-5 重试退避**已内置——三处 HTTP 路径（`blocking_post` / `chat_stream_messages` / `check_generate`）对 502/503 指数退避重试（500ms→1s→2s→4s→8s 封顶，默认 4 次），等模型加载完成即自动成功。4xx（鉴权/请求格式）不重试直接失败。

**解决**：
- **默认行为**：无需操作，CLI 自动重试，耐心等待（最多约 3.5s 退避 + 推理时间）。
- **关闭重试**（调试时想立即看到错误）：`export FUSION_MLX_RETRY_MAX=1`。
- **调大重试**（模型大、加载慢）：`export FUSION_MLX_RETRY_MAX=8`。

**真验证**：日志出现 `blocking_post: 瞬时错误，退避后重试 attempt=0 code=503` 即重试生效。设 `RUST_LOG=info` 可见。

**注意**：流式（`chat --stream`）重试仅覆盖**建连阶段**，流已建立后中途断流不重试（语义复杂）。建连 502 经 gateway 已修（见现象 E 上游说明）。

## 现象 C：check-mlx 假绿（列表有模型，生成却失败）

**现象**：`curl /v1/models` 返一长串模型名，但 `generate` 报 502 / 模型未加载。

**根因**：fusion-gateway 的 `/v1/models` 会「假绿」——列出云端 + 本地全部模型名，但 MLX 实际未加载该模型。仅看列表会误判可用。

**机制**：`fusion-design check-mlx` 做三段真探测破假绿：endpoint 解析 → `/v1/models` 鉴权+列表 → **1-token 真推理探针**。最终判定用真 chat 调用，非列表。

**解决**：
- **用 check-mlx 而非 curl 列表**判可用性：
  ```bash
  fusion-design check-mlx --model Qwen3.5-9B-4bit
  ```
- **显式传本地 mlx 模型 id**（模型解析优先级：`--model` > `FUSION_MLX_MODEL` env > 列表首个）。列表首个可能是未加载的云端模型，故建议显式传。
- 探针返 `model_loaded: false` → 模型确实没加载，去 MLX 侧加载（`start.sh` + 下载模型）。

**真验证**：`check-mlx` 成功 = 三段全过 + 真推理返 1 token。返非零退出码 + 诊断文案即 fail visibly。

## 现象 D：鉴权失败（401 / API key 无效）

**现象**：返 401 / "Unauthorized" / "invalid api key"。

**根因**：`FUSION_MLX_API_KEY` 未设 / 设错 / 与 gateway/MLX 配置的 key 不一致。fd-ai-adapter 经 `Authorization: Bearer <key>` 鉴权。

**排查**：

```bash
echo "env key: $FUSION_MLX_API_KEY"
# 直连 MLX 验 key
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:11434/v1/models \
  -H "Authorization: Bearer $FUSION_MLX_API_KEY"
# gateway 验 key
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:11432/v1/models \
  -H "Authorization: Bearer $FUSION_MLX_API_KEY"
```

**解决**：
```bash
export FUSION_MLX_API_KEY=fg-admin-key   # 换成实际配置的 key
```

**机制**：401 属 4xx 永久错误，M-5 重试**不重试**直接失败（重试只会浪费时间）。key 不匹配即立刻 bail，日志含鉴权失败诊断。

## 现象 E：流式中途断 / CJK 乱码（U+FFFD）

**现象**：`chat --stream` 流式输出中，中文出现 `U+FFFD`（替换字符 / 乱码菱形），或流中途断开。

**根因（乱码）**：早期 SSE 流按固定字节块读取，CJK 字符（3 字节 UTF-8）跨 chunk 切分导致半个字符解码失败。**已修**（L6 修复）：字节缓冲，跨 chunk 不完整 UTF-8 字符留到下个 chunk 拼齐再解码，无 U+FFFD。

**根因（中途断）**：流已建立后网络抖动 / MLX 重启。M-5 流式重试**仅覆盖建连阶段**，中途断流不重试（语义复杂，本轮不覆盖）。

**解决**：
- **乱码**：升级到 v0.1.12+（已含 L6 字节缓冲修复），无此问题。若仍现 → 报 issue（附模型 + prompt + 完整流日志）。
- **中途断**：重跑命令即可（M-5 会在下次建连时重试）。若频繁断 → 查 MLX 稳定性（`start.sh status` 看内存/进程）。
- **gateway 流式 502**（建连阶段）：fusion-gateway#108 已修（PR #111，2026-08-25，local-first ordering）。若遇上游回退重现 502，临时直连：
  ```bash
  export FUSION_MLX_BASE_URL=http://127.0.0.1:11434
  fusion-design chat --stream --json ...
  ```

## 现象 F：反序列化 .fusiondesign 报错（超深 / 超限）

**现象**：`export` / `lint` / `codegen` 等读 `.fusiondesign` 报"嵌套超限 / 节点总数超限 / 反序列化失败"。

**根因**：安全护栏触发。`.fusiondesign` 反序列化限制节点嵌套深度 ≤64、节点总数 ≤100000，防恶意输入栈溢出 / OOM。

**机制**：`validate_limits` 进入反序列化边界，超限直接拒绝（不部分加载），fail visibly 返回诊断。

**解决**：
- **正常文件触发**：文件确实过大（如 AI 生成异常膨胀）。用 `export --format json` 导出原始 JSON 检查节点数，人工裁剪。
- **恶意 / 损坏文件**：护栏拦得对，拒收该文件。从 Git 历史恢复前一版本（`.fusiondesign` 是 JSON，支持 Git 版本管理）。
- **自己构造文件超限**：拆成多页（`--page`）或多文件，单文件勿超 10 万节点。

## 现象 G：IPC 消息丢失 / 路径遍历拦截

**现象**：`--ipc-base` 经 fd-ecosystem 推送后对端没收到，或报"路径遍历拦截 / 非法路径"。

**根因（路径遍历）**：IPC 消息文件路径含 `..` / 绝对路径越界，被路径遍历防护拦截（防逃出 IPC 目录）。

**根因（消息丢失）**：旧版解析失败会静默吞错丢消息。**已修**（P2-3/R-A2）：解析失败**保留文件不静默吞**，日志告警，可人工恢复。

**解决**：
- **路径遍历**：IPC 文件名只用扁平名（如 `login.fusiondesign`），勿含 `../` 或绝对路径。对端目录约定见 fd-ecosystem 文档。
- **消息丢失**：升级 v0.1.12+（已含非破坏 consume）。查 IPC 目录残留文件 + 日志告警定位。文件大小护栏 ≤8MB，超限拒收。
- **对端没起**：fd-ecosystem 是文件式 IPC，对端（Fusion Code 等）须在约定的 ipc_base 目录监听。确认对端服务在跑。

## 现象 H：导出 Token 颜色未解析（var(--*) 出现在产物）

**现象**：`export --format png/svg/pdf` 产物里颜色仍是 `var(--color-primary)` 文本，而非 hex 色值；图片颜色异常。

**根因**：节点样式用了 Token 引用（`var(--color-primary)` 或 `token:color-primary`），但导出时未解析成 hex。**已修**（#8）：rasterization 前把 `var(--*)` / `token:*` 解析为 hex。

**解决**：
- **仍现未解析**：升级 v0.1.12+（已含 #8 解析）。确认激活的规范含该 Token 定义（`fusion-design token-css` 查），缺 Token 定义则无法解析 → 补 Token 或换规范。
- **PNG/SVG 颜色错**：Token 解析依赖激活规范。导出前 `fusion-design activate <正确规范>` 确保 Token 表齐。

**验证**：`fusion-design token-css` 输出的变量表 = 导出解析依据。Token 名对不上即解析失败。

## 现象 I：wasm 前端加载失败 / studio 同步陈旧

**现象**：Fusion-Desk WKWebView 加载画布空白 / 报 wasm 错；fusion-studio 拉到陈旧 wasm。

**根因**：wasm 产物（`fd_host_web_bg.wasm` + `fd_host_web.js`）缺失，或 studio 同步脚本拉错目录。

**产物链**：
```bash
# 1. 编译 raw wasm（须用 rustup cargo，homebrew cargo 无 wasm target）
~/.cargo/bin/cargo build -p fd-host-web --target wasm32-unknown-unknown
# → target/wasm32-unknown-unknown/debug/fd_host_web.wasm (14MB raw)

# 2. wasm-bindgen 后处理（emit _bg.wasm + .js）
~/.cargo/bin/wasm-bindgen --target web \
  --out-dir target/wasm32-unknown-unknown/debug \
  target/wasm32-unknown-unknown/debug/fd_host_web.wasm
# → fd_host_web_bg.wasm (1.5MB) + fd_host_web.js (27KB)
```

**解决**：
- **wasm 加载错**：确认走完步骤 2（bindgen 后处理），缺此步只有 raw wasm，WKWebView 无法用。`build.sh` 已含该步。
- **studio 陈旧**：fusion-studio `Scripts/build.sh` 从本仓 `target/wasm32-unknown-unknown/{release,debug}/` 拉取。改了 wasm 后须重跑 bindgen 落到该目录，否则 studio 拉到旧件。
- **can't find crate for std**：homebrew cargo（`/opt/homebrew/bin/cargo`）无 wasm target。改用 `~/.cargo/bin/cargo`（rustup 代理，toolchain 1.94 含 wasm32-unknown-unknown）。

**验证**：`ls -la target/wasm32-unknown-unknown/debug/fd_host_web_bg.wasm` 存在且时间新。

## 现象 J：模型不存在 / 跨部署模型列表漂移

**现象**：`--model <id>` 报模型不存在；换台机器默认模型就报错。

**根因**：默认 model `Qwen3.5-9B-4bit` 是内置 MLX 常用文本模型（真推理验证通过），但 **MLX 部署的模型列表随环境变**——不同机器下载的模型不同，gateway 又混列云端/本地名。

**解决（跨部署稳健做法）**：

```bash
# 1. 探出本机真可用的模型 id（真推理探针，破假绿）
fusion-design check-mlx --endpoint http://127.0.0.1:11434
# 输出里挑 model_loaded: true 的 id

# 2. 显式传该 id
fusion-design generate --model <探到的-id> --prompt "..."
```

**环境变量固化**（多命令复用）：
```bash
export FUSION_MLX_MODEL=<本机真可用-id>
fusion-design generate --prompt "..."        # 不传 --model 即用 env
```

**注意**：`--model` 优先级 > `FUSION_MLX_MODEL` env > 列表首个。列表首个可能是未加载云端模型（假绿），勿依赖。

**模型下载**：缺模型用镜像站，勿直连 HuggingFace：
```bash
HF_MIRROR=https://hf-mirror.com huggingface-cli download <model-id>
```

## 现象 K：lint 报告看不懂（13 规则含义）

**现象**：`lint` 输出一堆规则名 + 违规位置，不知每条啥意思、怎么修。

**13 规则含义 + 修法**：

| 规则 | 检测 | 自动可修 |
|------|------|---------|
| `contrast-check` | 文字/背景对比度不足（可读性差） | 否（需手调色） |
| `unlabeled-input` | 输入框无 label/placeholder | 否 |
| `text-effects` | 异常文字特效（如全大写旋转） | 否 |
| `abnormal-rotation` | 节点异常旋转角度 | 否 |
| `empty-effects` | 空特效节点（无视觉效果却存在） | 是（`--fix` 清理） |
| `token-inconsistency` | 颜色/字号未用 Token（硬编码） | 是（`--fix` 转 Token 引用） |
| `unnamed-node` | 节点未命名（默认名） | 是（`--fix` 自动命名） |
| `text-overflow` | 文字超出容器 | 否 |
| `overlapping-nodes` | 节点相互重叠 | 否 |
| `hardcoded-spacing` | 间距硬编码未用 Token | 否 |
| `hardcoded-font-size` | 字号硬编码未用 Token | 否 |
| `missing-interaction-state` | 交互元素缺 hover/active/disabled 态 | 否 |
| `layout-inconsistency` | 布局属性不一致 | 否 |

**用法**：

```bash
# 只跑关心的规则
fusion-design lint --input x.fusiondesign --rules token-inconsistency,unnamed-node,empty-effects

# 自动修可修项（3 条），先预览
fusion-design lint --input x.fusiondesign --fix --dry-run

# 落地修复
fusion-design lint --input x.fusiondesign --fix
```

**修不了的**：标"否"的规则需人工在画布调整（对比度/重叠/溢出等语义判断，非机械可修）。

## 现象 L：生成内容含 XSS / 注入（安全护栏触发）

**现象**：`codegen` HTML 产物里 `<script>` / `onerror=` 等被转义成实体；或导出 CSS 含异常内容被净化。

**根因**：安全护栏主动净化。codegen 对 HTML 内容做**实体转义**（XSS 防护，`<`→`&lt;`），CSS 注入净化（防恶意 CSS）。这是**预期行为**，非 bug。

**机制**：
- codegen XSS 实体转义：AI 生成或用户输入的 HTML 特殊字符被转义，防 `<script>` 注入。
- CSS 注入净化：异常 CSS 值（如 `url(javascript:)`、表达式）被剔除。
- 离线 allowlist：HTTP 出站仅允许 `127.0.0.1`（回环 + RFC1918 + 链路本地），拒公网。生成的代码不注入网络请求。

**解决**：
- **生成代码被转义**：若你需要原始 HTML 标签（如确实要嵌 `<script>`），这是设计权衡——fusion-design 优先安全。手工在产物里恢复需谨慎，确认来源可信。
- **`javascript:` URL 被拦**：护栏拒危险协议，用正常 `http(s)` 或本地路径。
- **想确认护栏生效**：故意传含 `<script>alert(1)</script>` 的 prompt，产物应见 `&lt;script&gt;` 而非可执行标签。

**这是特性**：100% 离线 + XSS/注入防护是 fusion-design 的核心安全承诺，勿当作 bug 修掉。

## 现象 M：磁盘无日志文件 / 需现场诊断件

**现象**：fd-cli 跑过（经 fusion-studio WKWebView 内嵌或终端直跑），但无 `fusion-design.log.*` 文件；或现场故障已发生却无持久诊断件。

**根因**：fd-cli 经 `tracing-appender` 写日轮转日志（OPS-13，v0.1.14）。默认路径 macOS `~/Library/Logs/fusion-design/`（Linux `~/.local/share/fusion-design/logs`）。无文件说明：env 禁用了、目录建不出来（权限）、或进程退出前未刷盘。

**解决**：
- **默认路径**：`ls ~/Library/Logs/fusion-design/` —— 找 `fusion-design.log.YYYY-MM-DD`。
- **文件被禁**：`FUSION_LOG_DISABLE_FILE=1`（或 `=true`）强制 stdout-only。取消即恢复文件日志：
  ```sh
  unset FUSION_LOG_DISABLE_FILE
  ```
- **重定向到自定义目录**（捕获某次会话用）：
  ```sh
  FUSION_LOG_DIR=/tmp/fd-session RUST_LOG=debug fusion-design --version
  ls /tmp/fd-session/   # → fusion-design.log.YYYY-MM-DD
  ```
- **调高日志级别**：默认过滤 `warn`。设 `RUST_LOG=info`（或 `=debug`）捕获诊断细节到文件：
  ```sh
  RUST_LOG=info FUSION_LOG_DIR=/tmp/fd-session fusion-design list-design-systems
  ```
- **目录创建失败 → 回退 stdout**：`init_logging` 若 `mkdir -p` 日志目录失败（权限/磁盘满），会往 stderr 打 `日志目录创建失败 ...，回退 stdout` 并回退 stdout-only —— CLI 照常跑。检查打印的路径与磁盘空间。
- **文件为空**：`--version` / `--help` 在任何 `tracing` 事件触发前即退出，轮转文件可能 0 字节。跑真子命令（`list-design-systems`、`health`、`check-mlx`）才能捕获事件。

**Guard 生命周期**：文件 writer 在 `init_logging` 的 `WorkerGuard` 进程退出时 drop 刷盘。正常 CLI 关机刷盘正常；`SIGKILL` 可能丢最后一行缓冲。

## 环境变量参考

全部可选，未设即默认。完整表见 `README.md` § 环境变量；此处为排障导向速查（OPS-16）。

| 变量 | 默认值 | 作用 | 排障用途 |
|------|--------|------|----------|
| `FUSION_MLX_BASE_URL` | `http://127.0.0.1:11432` | 推理 endpoint。CLI `--endpoint` 覆盖。多节点逗号分隔。 | 现象 A/B/D：gateway→直连 `11434`，或加故障转移节点。 |
| `FUSION_MLX_API_KEY` | (无) | Bearer 鉴权 key。 | 现象 D：须与 gateway/MLX 配置的 key 一致。 |
| `FUSION_MLX_MODEL` | (列表首个) | `check-mlx` 默认模型 id。 | 现象 C/J：显式传本地 mlx id 破假绿。 |
| `FUSION_MLX_RETRY_MAX` | `4` | 502/503 最大尝试次数。`1`=关。 | 现象 B：模型加载慢调大，想快速看错调小。 |
| `FUSION_MLX_RETRY_DEADLINE_SECS` | `300` | 重试总 deadline。 | 现象 B：模型加载超 5 分钟调大。 |
| `FUSION_MLX_SSE_BUFFER_CAP` | `8388608` | SSE 缓冲上限字节，超即 bail。 | 现象 E：防模型输出失控 OOM。 |
| `FUSION_MLX_STREAM_IDLE_SECS` | `60` | SSE chunk 间最大空闲秒数（FAULT-1，v0.1.14）。 | 现象 E：中途断流现以失败可见替代无限挂起。 |
| `FUSION_LOG_DISABLE_FILE` | (未设) | `1`/`true` = stdout-only，不写文件（OPS-13，v0.1.14）。 | 现象 M：取消即恢复 `~/Library/Logs/fusion-design/` 文件日志。 |
| `FUSION_LOG_DIR` | (平台默认) | 覆盖文件日志目录（OPS-13，v0.1.14）。 | 现象 M：把某会话日志重定向到 `/tmp/...` 捕获。 |
| `FUSION_VENV_ROOT` | (自动探测) | ecosystem 工具调用的共享 `.venv` 根。 | 现象 G：venv 非同址时覆盖。 |
| `FUSION_TRAINER_BIN` | `fusion-trainer` | fusion-trainer 二进制路径。 | 现象 G：不在 `PATH` 时覆盖。 |
