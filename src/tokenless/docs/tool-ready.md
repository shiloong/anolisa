# Tool Ready 设计方案与功能说明

## 1. 概述

Tool Ready 是 tokenless 项目的一个环境就绪检测与自动修复子系统，通过 copilot-shell hook 机制在 LLM 工具调用前后介入，解决以下核心问题：

- **LLM 在环境故障上的 token 浪费**：当工具因缺少依赖、权限不足、网络不通等环境问题失败时，LLM 会反复重试，每次消耗大量 token，却无法通过"多试几次"解决环境问题
- **环境问题与逻辑错误的混淆**：LLM 无法区分"命令找不到"（环境问题）和"算法写错"（逻辑错误），导致调试方向错误
- **依赖安装的硬编码**：每个依赖的安装逻辑写死在脚本里，新增依赖需要改代码

核心设计原则：**fail-open** — 任何环节失败都不阻断工具执行，原始数据透传。

---

## 2. 整体架构

```
┌───────────────────────────────────────────────────────────────────┐
│                      LLM 工具调用生命周期                          │
│                                                                   │
│  LLM请求 ──▶ PreToolUse ──▶ 工具执行 ──▶ PostToolUse ──▶ LLM响应 │
│              │                        │                           │
│              │                        │                           │
│    ┌─────────▼──────────┐    ┌────────▼──────────┐               │
│    │  tokenless-tool-   │    │  tokenless-compress│               │
│    │  ready.sh          │    │  -response.sh      │               │
│    │                    │    │                     │               │
│    │  环境就绪检测       │    │  失败归因分析        │               │
│    │  + 自动修复尝试     │    │  + 响应压缩         │               │
│    │                    │    │  + TOON编码          │               │
│    └─────────┬──────────┘    └────────┬──────────┘               │
│              │                        │                           │
│    ┌─────────▼──────────┐             │                           │
│    │  tool-ready-spec   │             │                           │
│    │  .json             │             │                           │
│    │  (配置驱动)         │             │                           │
│    └─────────┬──────────┘             │                           │
│              │                        │                           │
│    ┌─────────▼──────────┐             │                           │
│    │  tokenless-env-    │             │                           │
│    │  fix.sh            │             │                           │
│    │  (通用安装引擎)     │             │                           │
│    └────────────────────┘             │                           │
│                                       │                           │
│  ┌────────────────────────────────────▼─────────────────────────┐ │
│  │                    tokenless env-check CLI                    │ │
│  │         --tool / --all / --fix / --checklist                  │ │
│  └──────────────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────────────┘
```

---

## 3. 主流程详解

### 3.1 PreToolUse 阶段：环境就绪检测 (`tokenless-tool-ready.sh`)

**触发时机**：copilot-shell 在每次工具执行前触发 PreToolUse 事件，matcher="" 匹配所有工具。

**流程**：

```
1. 读取 stdin JSON → 提取 tool_name
2. 从 tool-ready-spec.json 加载该工具的依赖声明
3. normalize_deps: 统一格式
   - 字符串 "jq"           → {binary:"jq", package:"jq", manager:"apt"}
   - 字符串 "rtk>=0.35"    → {binary:"rtk", version:">=0.35", package:"rtk", manager:"apt"}
   - 对象 {binary:"curl"...} → 直接使用
4. 五维度扫描：
   ├─ required 二进制   → command -v + 版本约束检查
   ├─ recommended 二进制 → 同上
   ├─ config_files      → 路径存在性（支持 ~ 展开）
   ├─ permissions        → file_read/file_write/exec_shell
   └─ network            → https_outbound 连通性
5. 判断 PRE_FIX_STATUS：
   ├─ 缺 required 或 permission → NOT_READY
   ├─ 缺 recommended/config/net → PARTIAL
   └─ 全部满足              → READY
6. 如有缺失依赖 → 调用 env-fix.sh fix-all 自动安装
7. 重新扫描 → POST_FIX_STATUS
8. 输出决策：
   ├─ READY → 静默退出（不干扰 LLM）
   ├─ NOT_READY → 注入 additionalContext + "Skip retry"
   └─ PARTIAL → 注入 additionalContext（无 Skip retry）
```

**状态判定规则**：

| 条件 | 状态 | LLM 收到的指导 |
|------|------|---------------|
| 全部满足 | READY | 无输出（静默退出） |
| 缺必需依赖/权限 | NOT_READY | "Skip retry — environment issue, not logic error" |
| 缺推荐依赖/配置/网络 | PARTIAL | 诊断信息（不阻断执行） |

**输出格式**：

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "additionalContext": "[tokenless env-check] Shell tool: NOT_READY ( required missing: fakebin99; available: jq✓ curl✓;). Auto-fix attempted: fakebin99 install failed. Skip retry — environment issue, not logic error."
  }
}
```

### 3.2 自动修复引擎 (`tokenless-env-fix.sh`)

**配置驱动的通用安装引擎**，从 tool-ready-spec.json 或命令行参数读取依赖声明，按声明的包管理器类型安装。

**安装流程**：

```
1. 检查 binary 是否已存在 → 已存在则跳过
2. 检查版本约束 → 已满足则跳过
3. 检查 24h 内是否已成功修复 → 防止重复安装
4. 按 manager 调用对应的安装函数（主策略）
5. 验证主策略安装结果
6. 主策略失败 → 依次尝试 fallback 列表中的每个备选策略
7. 记录结果到 ~/.tokenless/env-fix.log
```

**支持的 11 种包管理器**：

| Manager | 安装方式 | 适用场景 |
|---------|---------|---------|
| `apt` | apt-get / yum / dnf / apk | 系统级包（jq, curl, bash） |
| `rpm` | yum / dnf / rpm | Red Hat 系列 |
| `pip` | pip / pip3 | Python 包 |
| `uv` | uv tool install / uv pip install | Python 包（现代替代 pip） |
| `npm` | npm install -g | Node.js 全局包 |
| `npx` | npx -y（验证可用性，不安装） | Node.js 临时执行 |
| `cargo` | cargo install --locked | Rust 工具（从 crates.io） |
| `cargo_build` | cargo build --manifest-path + 复制到 /usr/local/bin | 从本地源码构建 |
| `symlink` | ln -sf source /usr/local/bin/binary | 预编译二进制链接 |
| `path` | export PATH + 写入 shell rc | PATH 目录注入 |
| `dir` | mkdir -p | 目录创建 |

**Fallback 策略示例**（rtk 的三层回退）：

```json
{
  "binary": "rtk",
  "manager": "cargo",
  "fallback": [
    { "method": "symlink", "binary": "rtk", "source": "/usr/share/tokenless/bin/rtk" },
    { "method": "cargo_build", "manifest": "/usr/share/tokenless/third_party/rtk/Cargo.toml", "binary": "rtk" }
  ]
}
```

```
rtk 安装尝试顺序：
  1. cargo install rtk --locked        → 从 crates.io 安装
  2. ln -sf /usr/share/tokenless/bin/rtk → 从预编译路径链接
  3. cargo build --manifest-path ...     → 从本地源码编译
```

**CLI 调用方式**：

```bash
# 修复单个依赖（JSON 对象）
tokenless-env-fix.sh fix '{"binary":"jq","package":"jq","manager":"apt"}'

# 修复多个依赖（JSON 数组）
tokenless-env-fix.sh fix-all '[{...},{...}]'

# 简单修复（名称 + 可选包管理器）
tokenless-env-fix.sh fix-simple jq apt

# 从 spec 文件修复某工具的所有依赖
tokenless-env-fix.sh fix-tool Shell

# 列出所有可修复项
tokenless-env-fix.sh check
```

### 3.3 PostToolUse 阶段：失败归因 + 响应压缩 + TOON 编码 (`tokenless-compress-response.sh`)

**触发时机**：copilot-shell 在每次工具执行完成后触发 PostToolUse 事件。

**三条管线顺序执行**：

```
原始 tool_response
     │
     ▼
 ┌─────────────────┐
 │  失败归因分析     │  ← Tool Ready 新增功能
 │  (Attribution)   │
 └────────┬────────┘
          │
          ▼
 ┌─────────────────┐
 │  响应压缩        │
 │  (tokenless      │
 │   compress-      │
 │   response)      │
 └────────┬────────┘
          │
          ▼
 ┌─────────────────┐
 │  TOON 编码       │  ← 可选（toon 不存在时跳过）
 │  (toon -e)       │
 └────────┬────────┘
          │
          ▼
  additionalContext → LLM
```

**归因分析逻辑**：

从 `tool_response` 中提取 `exit_code`、`stderr`、`error` 字段，对错误文本进行模式匹配，分为 5 类环境错误和逻辑错误：

| 错误模式 | 归因类别 | 修复提示 | 示例 |
|----------|---------|---------|------|
| `command not found` / `which: no` | ENV_DEPENDENCY_MISSING | Install missing dependency: X | `command not found: rtk` |
| `Permission denied` / `Access denied` | ENV_PERMISSION | Check file/dir permissions | `Permission denied: /root/file` |
| `No such file or directory` / `ENOENT` | ENV_FILE_MISSING | Create or locate the required file | `No such file: config.json` |
| `Connection refused` / `ETIMEDOUT` | ENV_NETWORK | Check network connectivity | `Connection refused: localhost:8080` |
| `ModuleNotFoundError` / `ImportError` | ENV_PACKAGE_MISSING | Install the required module | `ModuleNotFoundError: requests` |

环境错误 → 注入 "Skip retry — this is an environment issue, not a logic error"
逻辑错误 → 不注入 Skip retry 指导（LLM 可以正常调试）

**响应压缩 + TOON 编码**：

1. 去除 debug/null/空值字段，截断长字符串和大数组
2. 压缩结果为有效 JSON → TOON 编码（消除 JSON 语法开销）
3. 压缩结果非 JSON → 直接使用压缩结果

**最终输出**：

```json
{
  "suppressOutput": true,
  "hookSpecificOutput": {
    "hookEventName": "PostToolUse",
    "additionalContext": "[tokenless env-attribution] Shell tool failed: ENV_DEPENDENCY_MISSING (Install missing dependency: fakebin99). Skip retry...\n\n[tokenless] Tool response from Shell response compressed + TOON encoded (4% token savings).\nTOON is a compact notation...\n<压缩后的内容>"
  }
}
```

### 3.4 CLI 环境检查 (`tokenless env-check`)

Rust 实现的 CLI 工具，支持从终端直接检查和修复环境：

```bash
tokenless env-check --tool Shell    # 检查单个工具
tokenless env-check --all           # 检查所有工具
tokenless env-check --all --fix     # 检查 + 自动修复
tokenless env-check --checklist     # 生成完整就绪清单
```

**输出示例**：

```
Tool Environment Ready Checklist
=================================
✅ Shell      — READY     (jq ✓ (apt), bash ✓ (apt), rtk ✓ (cargo), tokenless ✓ (cargo), toon ✓ (cargo))
✅ Bash       — READY     (bash ✓ (apt), jq ✓ (apt), rtk ✓ (cargo), tokenless ✓ (cargo))
✅ WebFetch   — READY     (curl ✓ (apt))
✅ Read       — READY     (no dependencies)
✅ Write      — READY     (no dependencies)

Summary: 5 ready, 0 partial, 0 not ready (total: 5)
```

内部结构与 Bash hook 一致：`DepEntry` / `FallbackEntry` 支持对象格式 + 字符串格式向后兼容，`normalize_dep()` 处理 `"rtk>=0.35"` 等版本约束字符串。

---

## 4. 配置驱动：tool-ready-spec.json

所有依赖声明与安装策略集中在一个 JSON 文件，新增工具/依赖只需修改配置，不需改代码。

### 4.1 当前配置

| 工具 | 必需依赖 | 推荐依赖 | 配置文件 | 权限 | 网络 |
|------|----------|----------|----------|------|------|
| **Shell** | jq (apt), bash (apt) | rtk >=0.35 (cargo→symlink→cargo_build), tokenless (cargo→symlink→cargo_build), toon (cargo→symlink→cargo_build) | ~/.copilot-shell/settings.json | — | — |
| **Bash** | bash (apt) | jq (apt), rtk >=0.35 (cargo→symlink), tokenless (cargo→symlink) | — | exec_shell | — |
| **WebFetch** | curl (apt) | — | — | — | https_outbound |
| **Read** | — | — | — | file_read | — |
| **Write** | — | — | — | file_write | — |

### 4.2 依赖项对象字段

```json
{
  "binary": "rtk",             // 必须：要检查的命令名（command -v 目标）
  "version": ">=0.35",         // 可选：版本约束
  "package": "rtk",            // 必须：包管理器中的包名（可与 binary 不同）
  "manager": "cargo",          // 必须：包管理器类型
  "pip_name": "...",           // 可选：pip 安装时的包名（默认 = package）
  "uv_name": "...",            // 可选：uv 安装时的包名（默认 = package）
  "npm_name": "...",           // 可选：npm 安装时的包名（默认 = package）
  "use_npx": true,             // 可选：是否用 npx 运行
  "fallback": [                // 可选：主策略失败时的回退列表
    { "method": "symlink", "binary": "rtk", "source": "/usr/share/tokenless/bin/rtk" },
    { "method": "cargo_build", "manifest": "...", "binary": "rtk", "features": ["cli"] }
  ]
}
```

### 4.3 向后兼容

字符串格式自动转为对象格式：

| 输入 | 转换结果 |
|------|---------|
| `"jq"` | `{binary:"jq", package:"jq", manager:"apt"}` |
| `"rtk>=0.35"` | `{binary:"rtk", version:">=0.35", package:"rtk", manager:"apt"}` |
| `{binary:"curl", package:"curl", manager:"apt"}` | 直接使用 |

以 `_` 开头的键（如 `_comment`）在解析时自动跳过。

---

## 5. 文件清单

| 文件 | 作用 | 语言 |
|------|------|------|
| `hooks/copilot-shell/tokenless-tool-ready.sh` | PreToolUse 环境就绪检测 hook | Bash |
| `hooks/copilot-shell/tokenless-env-fix.sh` | 配置驱动的通用安装引擎 | Bash |
| `hooks/copilot-shell/tool-ready-spec.json` | 依赖声明 + 安装策略配置 | JSON |
| `hooks/copilot-shell/tokenless-compress-response.sh` | PostToolUse 归因 + 压缩 + TOON hook | Bash |
| `crates/tokenless-cli/src/env_check.rs` | CLI 环境检查子命令 | Rust |
| `crates/tokenless-cli/src/main.rs` | CLI 入口（含 EnvCheck 子命令） | Rust |
| `scripts/install.sh` | 郻署脚本（含 hook 配置写入） | Bash |
| `hooks/copilot-shell/README.md` | 功能文档 | Markdown |

---

## 6. 扩展指南

新增一个工具的依赖声明只需编辑 `tool-ready-spec.json`，不需改动任何脚本代码。

**示例**：为 Python 工具添加依赖

```json
"PythonTool": {
  "required": [
    { "binary": "python3", "package": "python3", "manager": "apt" }
  ],
  "recommended": [
    { "binary": "pytest", "package": "pytest", "manager": "uv",
      "uv_name": "pytest",
      "fallback": [
        { "method": "pip", "package": "pytest" }
      ]
    }
  ],
  "config_files": [],
  "permissions": [],
  "network": []
}
```

新增后，PreToolUse hook 自动检测 PythonTool 的依赖，env-fix 自动按 `uv → pip` 回退链安装 pytest，CLI `tokenless env-check --tool PythonTool` 自动可用。