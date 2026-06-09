# Tokenless macOS 构建与安装指南

## 平台兼容性

Tokenless 的 Rust 代码已完全兼容 macOS：

- 所有平台特定代码使用 `#[cfg(unix)]` 条件编译，macOS 属于 Unix 系统
- 无 Linux 专属依赖（无 eBPF、procfs 等）
- 所有依赖均为跨平台 Rust crate（serde、clap、rusqlite、chrono、dirs、libc 等）
- Homebrew Formula 已就绪，支持从源码编译安装

## 前置条件

| 依赖 | 最低版本 | 用途 | 安装方式 |
|---|---|---|---|
| Rust toolchain | >= 1.89 | 编译 tokenless/rtk/toon | `brew install rust` 或 [rustup](https://rustup.rs) |
| Node.js + npm | LTS | 编译 OpenClaw TypeScript 插件 | `brew install node` |
| jq | — | 运行时依赖 | `brew install jq` |
| Python 3 | >= 3.10 | hook 脚本运行 | `brew install python@3.13` |
| Git | — | clone rtk 源码 | `xcode-select --install` 自带 |

## 方法一：Homebrew 安装（推荐）

### 1. 生成源码 tarball 并更新 Formula

```bash
cd src/tokenless
make dist-brew
```

此命令会：
1. 清理历史构建产物（`make clean`）
2. 克隆 rtk v0.36.0 到 `third_party/rtk/` 并应用 patch
3. 打包 `tokenless-0.5.0.tar.gz`（排除 `target/`、`.git/`、`__pycache__/`、`node_modules/`）
4. 计算 SHA256 并自动更新 `Formula/tokenless.rb` 中的 `url`、`sha256`、`version`

### 2. 从本地 Formula 安装

```bash
brew install --formula Formula/tokenless.rb
```

Homebrew 会在沙箱中执行以下构建步骤：
1. `cargo build --release` — 编译 tokenless workspace（tokenless-cli、tokenless-schema、tokenless-stats）
2. `cargo build --release --manifest-path third_party/rtk/Cargo.toml` — 编译 rtk
3. `cargo install toon-format --version 0.4.6` — 安装 toon
4. `make stamp-adapter-templates` — 从 `.in` 模板生成带版本号的 adapter 配置文件
5. `npm install && npm run build` — 编译 OpenClaw TypeScript 插件
6. 安装二进制文件、adapter 资源、文档

### 3. 安装产物

| 类型 | 路径 |
|---|---|
| 主二进制 | `$(brew --prefix)/bin/tokenless` |
| 辅助二进制 | `$(brew --prefix)/bin/rtk`、`$(brew --prefix)/bin/toon`（symlink 到 libexec） |
| libexec | `$(brew --prefix)/libexec/anolisa/...`（rtk、toon 实际位置） |
| Adapter 资源 | `$(brew --prefix)/share/anolisa/adapters/tokenless/` |
| Cosh extension | `$(brew --prefix)/share/anolisa/extensions/tokenless/` |
| 文档 | `$(brew --prefix)/share/doc/tokenless/` |

### 4. 注册 AI 工具适配器

安装后根据使用的 AI 编码工具注册对应 adapter：

```bash
SHARE_DIR="$(brew --prefix)/share/anolisa/adapters/tokenless"

# Claude Code
"$SHARE_DIR/claude-code/scripts/install.sh"

# Copilot Shell (cosh) — 自动发现，无需手动注册
# Extension 位于: $(brew --prefix)/share/anolisa/extensions/tokenless/
```

### 5. 验证安装

```bash
tokenless --version
rtk --version
toon --version

# 测试 schema 压缩
echo '{"type":"object","properties":{"name":{"type":"string"}}}' | tokenless compress-schema

# 测试 TOON 编码
echo '{"name":"Alice","age":30}' | tokenless compress-toon
```

## 方法二：源码手动构建

### 1. 准备 rtk 源码

```bash
cd src/tokenless
just setup-rtk
# 或手动执行:
# git clone --depth 1 --branch v0.36.0 https://github.com/rtk-ai/rtk.git third_party/rtk
# patch --forward -p1 --no-backup-if-mismatch -d third_party/rtk < third_party/patches/rtk-tokenless-stats.patch
```

### 2. 编译

```bash
# 编译 tokenless + rtk + toon + OpenClaw 插件
make build

# 仅编译 tokenless + rtk
make build-tokenless

# 仅编译 toon
make build-toon
```

### 3. 安装到本地

```bash
# 默认安装到 ~/.local/
make install

# 自定义安装路径
make install PREFIX=/usr/local

# 系统级安装
make install INSTALL_PROFILE=system PREFIX=/usr
```

安装产物：

| 类型 | 路径 |
|---|---|
| 主二进制 | `~/.local/bin/tokenless` |
| 辅助二进制 | `~/.local/libexec/anolisa/tokenless/rtk`、`.../toon` |
| Symlink | `~/.local/bin/rtk` → libexec、`~/.local/bin/toon` → libexec |
| Adapter 资源 | `~/.local/share/anolisa/adapters/tokenless/` |
| Cosh extension | `~/.copilot-shell/extensions/tokenless/` |

### 4. 注册适配器

```bash
# 注册所有适配器（cosh + openclaw + hermes + qoder + claude-code + codex）
make adapter-install

# 仅注册特定适配器
make cosh-extension-install
make claude-code-install
make openclaw-install
make hermes-install
make qoder-install
make codex-install
```

### 5. 一键完整安装

```bash
make setup
# 等价于: make install + make adapter-install
```

### 6. 卸载

```bash
# Homebrew 安装
brew uninstall tokenless

# 手动安装
make uninstall
# 或手动删除:
# rm ~/.local/bin/tokenless ~/.local/bin/rtk ~/.local/bin/toon
# rm -rf ~/.local/libexec/anolisa/tokenless
# rm -rf ~/.local/share/anolisa/adapters/tokenless
```

## CLI 用法速查

### Schema 压缩（~57% token 节省）

```bash
# 单文件
tokenless compress-schema -f tool.json

# 从 stdin
cat tool.json | tokenless compress-schema

# 批量压缩（JSON 数组）
tokenless compress-schema -f tools.json --batch
```

### Response 压缩（~26–78% token 节省）

```bash
tokenless compress-response -f response.json
curl -s https://api.example.com/data | tokenless compress-response
```

### TOON 编码（15–40% token 节省）

```bash
# JSON -> TOON
echo '{"name":"Alice","age":30}' | tokenless compress-toon
# name: Alice
# age: 30

# TOON -> JSON
echo 'name: Alice\nage: 30' | tokenless decompress-toon
```

### 环境检查（Tool Ready）

```bash
# 检查单个工具的环境依赖
tokenless env-check --tool Shell

# 检查所有工具
tokenless env-check --all

# 生成检查清单
tokenless env-check --checklist

# 检查并自动修复缺失依赖
tokenless env-check --tool Shell --fix
```

### 统计查询

```bash
# 查看使用统计摘要
tokenless stats summary

# 查看最近记录
tokenless stats list

# 清除统计
tokenless stats clear
```

## 构建测试

```bash
# 运行 Rust 单元测试 + 集成测试
make test

# 仅运行 Rust 测试
make test-tokenless

# 仅运行 hook 集成测试
make test-hooks

# Lint
make lint

# 格式化
make fmt
```

## 目录结构（macOS 安装后）

```
~/.local/                          # 或 $(brew --prefix)/
├── bin/
│   ├── tokenless                  # 主 CLI
│   ├── rtk -> ../libexec/.../rtk  # 命令重写引擎
│   └── toon -> ../libexec/.../toon # TOON 编码器
├── libexec/anolisa/tokenless/
│   ├── rtk                        # RTK 二进制
│   └── toon                       # TOON 二进制
├── share/anolisa/adapters/tokenless/
│   ├── manifest.json              # Adapter 清单
│   ├── common/                    # 共享 hooks/spec/commands
│   │   ├── hooks/                 # tool_ready, rewrite, compress_response, compress_schema
│   │   ├── commands/              # Hook 命令配置
│   │   ├── cosh-extension.json    # Copilot Shell extension manifest
│   │   ├── tool-ready-spec.json   # 工具依赖声明
│   │   └── tokenless-env-fix.sh   # 自动修复脚本
│   ├── openclaw/                  # OpenClaw 插件
│   ├── hermes/                    # Hermes Agent 插件
│   ├── qoder/                     # Qoder CLI 插件
│   ├── claude-code/               # Claude Code 插件
│   └── codex/                     # Codex 插件
└── share/doc/tokenless/           # 文档
    ├── tokenless-user-manual-en.md
    ├── tokenless-user-manual-zh.md
    └── response-compression.md
```

## 注意事项

1. **Rust 版本**：CI 使用 Rust 1.89.0，本地构建版本可以更高但不得使用高于 CI 版本才稳定的 API
2. **网络依赖**：`cargo install toon-format` 和 rtk clone 需要网络访问，确保 crates.io 可达
3. **OpenClaw 插件编译**：需要 Node.js 和 npm，如不需要可跳过 `make build-openclaw-plugin`
4. **Adapter 注册是幂等的**：可重复执行 `make <adapter>-install` 覆盖安装
5. **Stats 数据库**：位于 `~/.local/share/tokenless/stats.db`（由 `getpwuid_r` 解析的 home 目录决定）
