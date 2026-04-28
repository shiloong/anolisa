# Token-Less

**LLM token optimization toolkit** — schema/response compression + command rewriting + tool environment readiness.

Token-Less combines complementary strategies to minimize LLM token consumption:

- **Schema & Response Compression** — Compresses OpenAI Function Calling tool definitions and API responses via the `tokenless-schema` library, cutting structural overhead before tokens ever reach the context window.
- **TOON Context Compression** — Encodes JSON responses to TOON (Token-Oriented Object Notation) format via the `toon` binary, reducing token usage by 15-40% for structured data.
- **Command Rewriting** — Integrates [RTK](https://github.com/rtk-ai/rtk) to filter and rewrite CLI command output, eliminating noise that would otherwise waste 60–90% of tokens.
- **Tool Ready** — Pre-checks tool execution environments (binaries, configs, permissions, network), auto-fixes missing dependencies, and classifies execution failures as environment issues vs logic errors — reducing wasted retry tokens.

Two integration paths are available:

- **OpenClaw plugin** — covers command rewriting and response compression in one plugin. Schema compression is not yet supported by OpenClaw's hook system.
- **copilot-shell hook** — intercepts Shell commands via a PreToolUse hook and delegates to RTK for command rewriting + output filtering.

## Features

| Capability | Token Savings | Details |
|---|---|---|
| Schema compression | ~57% | Compresses OpenAI Function Calling tool schemas |
| Response compression | ~26–78% | Compresses API / tool responses (varies by content type) |
| TOON context compression | 15–40% | Encodes JSON to TOON format for LLMs |
| Command rewriting | 60–90% | Filters CLI output via RTK (70+ commands supported) |
| Tool Ready | reduces retry waste | Pre-check env, auto-fix deps, failure attribution |
| OpenClaw plugin | — | Command rewriting ✅, Response compression ✅, Schema compression ⏳ |
| copilot-shell hooks | — | Tool Ready ✅, Command rewriting ✅, Response compression ✅, TOON ✅, Schema compression ⏳ |
| Zero runtime deps | — | Pure Rust, single static binary |

## Architecture

```
Token-Less/
├── crates/tokenless-schema/   # Core library: SchemaCompressor + ResponseCompressor
├── crates/tokenless-cli/      # CLI binary: `tokenless` command (env-check, compress, stats)
├── openclaw/                  # Unified OpenClaw plugin (TypeScript delegate)
├── hooks/copilot-shell/       # copilot-shell hooks (tool-ready + rewrite + compression)
│   ├── tool-ready-spec.json   # Per-tool dependency declarations (config-driven)
│   ├── tokenless-tool-ready.sh  # PreToolUse: env readiness check
│   ├── tokenless-env-fix.sh     # Auto-fix engine (11 package managers)
│   ├── tokenless-compress-response.sh # PostToolUse: compress + attribution + TOON
│   ├── tokenless-rewrite.sh    # PreToolUse: command rewriting via RTK
│   └── tokenless-compress-schema.sh    # BeforeModel: schema compression
├── third_party/rtk/           # RTK submodule (command rewriting engine)
├── third_party/toon/          # TOON submodule (JSON to TOON encoding)
├── Makefile                   # Unified build system
└── scripts/install.sh         # One-step installer
```

## Quick Start

```bash
# Clone with submodules
git clone --recursive <repo-url>
cd Token-Less

# Full setup: build + install binaries + deploy OpenClaw plugin
make setup
```

Or use the install script directly:

```bash
./scripts/install.sh
```

Both methods install `tokenless` and `rtk` to `/usr/share/tokenless/bin`, deploy the OpenClaw plugin, and install the copilot-shell hook.

## CLI Usage

### compress-schema

Compress a single tool schema:

```bash
# From file
tokenless compress-schema -f tool.json

# From stdin
cat tool.json | tokenless compress-schema
```

Compress a batch of tools (JSON array):

```bash
tokenless compress-schema -f tools.json --batch
```

### compress-response

Compress an API response:

```bash
# From file
tokenless compress-response -f response.json

# From stdin
curl -s https://api.example.com/data | tokenless compress-response
```

### compress-toon / decompress-toon

Encode JSON to TOON format (or decode back to JSON):

```bash
# Encode JSON to TOON
echo '{"name":"Alice","age":30}' | tokenless compress-toon
# name: Alice
# age: 30

# Decode TOON back to JSON
echo 'name: Alice\nage: 30' | tokenless decompress-toon
# {"name":"Alice","age":30}
```

## copilot-shell Hooks

Four copilot-shell hooks provide token optimization at different stages:

| Hook | Event | Status | Savings |
|------|-------|--------|--------|
| Tool environment check | PreToolUse (all tools) | ✅ Available | Reduces retry waste |
| Command rewriting | PreToolUse (Shell) | ✅ Available | 60–90% |
| Response compression + attribution + TOON | PostToolUse | ✅ Available | 30–60% (combined) |
| Schema compression | BeforeModel | ⏳ Placeholder | ~57% |

### Install

```bash
make copilot-shell-install
```

Then add the hook configs to your `~/.copilot-shell/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-tool-ready.sh",
            "name": "tokenless-tool-ready",
            "timeout": 3000
          }
        ]
      },
      {
        "matcher": "Shell",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-rewrite.sh",
            "name": "tokenless-rewrite",
            "timeout": 5000
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-compress-response.sh",
            "name": "tokenless-compress-response",
            "timeout": 10000
          }
        ]
      }
    ],
    "BeforeModel": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-compress-schema.sh",
            "name": "tokenless-compress-schema",
            "timeout": 10000
          }
        ]
      }
    ]
  }
}
```

For detailed usage and troubleshooting, see [`hooks/copilot-shell/README.md`](hooks/copilot-shell/README.md).

## Tool Ready

Tool Ready prevents wasted LLM tokens from retrying commands that fail due to missing environment dependencies.

**How it works**: Before each tool call, the `tokenless-tool-ready.sh` hook checks the tool's dependency list (from `tool-ready-spec.json`). If dependencies are missing, it reports `NOT_READY` with "Skip retry" guidance so the LLM doesn't waste tokens retrying a command that can't succeed. After a tool call fails, the compression hook classifies the error (missing binary, permissions, network, etc.) and injects attribution context.

### env-check CLI

```bash
# Check a specific tool
tokenless env-check --tool Shell

# Check all tools
tokenless env-check --all

# Generate checklist
tokenless env-check --checklist

# Check and auto-fix missing deps
tokenless env-check --tool Shell --fix
```

### Configuration

Per-tool dependencies are declared in `tool-ready-spec.json` using object format:

```json
{
  "Shell": {
    "required": [
      { "binary": "jq", "package": "jq", "manager": "apt" }
    ],
    "recommended": [
      { "binary": "rtk", "version": ">=0.35", "package": "rtk", "manager": "cargo",
        "fallback": [
          { "method": "symlink", "binary": "rtk", "source": "/usr/share/tokenless/bin/rtk" }
        ]
      }
    ]
  }
}
```

String format `"jq"` is also supported (auto-converts to object). See [`docs/tool-ready.md`](docs/tool-ready.md) for full spec details.

## OpenClaw Plugin

The plugin hooks into the OpenClaw agent loop at two stages:

| Hook | Event | Action | Status |
|---|---|---|---|
| Command rewriting | `before_tool_call` | Rewrites `exec` commands to RTK equivalents for filtered output | ✅ Active |
| Response compression | `tool_result_persist` | Compresses tool results before they enter the context window | ✅ Active |
| Schema compression | — | Not supported by OpenClaw's hook system (no hook exposes tool schemas) | ⏳ Blocked |

**Response compression details:**
- Automatically compresses results from all tool types (`web_search`, `web_fetch`, `read_file`, etc.)
- Skips `exec` tool results when RTK is enabled — RTK already produces optimized output, avoiding double-compression
- Observed savings: **~78%** on `web_fetch` results, varies by content type

Each hook degrades gracefully — if the corresponding binary (`rtk` or `tokenless`) is not installed, that hook is silently skipped.

### Configuration

Options in `openclaw.plugin.json`:

| Option | Default | Description |
|---|---|---|
| `rtk_enabled` | `true` | Enable RTK command rewriting |
| `schema_compression_enabled` | `true` | Enable tool schema compression (pending OpenClaw support) |
| `response_compression_enabled` | `true` | Enable tool response compression via `tool_result_persist` |
| `verbose` | `true` | Log detailed rewrite/compression info |

## Build

| Target | Description |
|---|---|
| `make build` | Build `tokenless` + `rtk` + `toon` (release mode) |
| `make build-tokenless` | Build `tokenless` only |
| `make build-rtk` | Build `rtk` only |
| `make build-toon` | Build `toon` only |
| `make install` | Build and install binaries to `INSTALL_DIR` |
| `make test` | Run all tests (Rust + hooks) |
| `make test-hooks` | Run hook integration tests |
| `make lint` | Run clippy checks |
| `make fmt` | Format code |
| `make clean` | Clean build artifacts |
| `make openclaw-install` | Install OpenClaw plugin |
| `make openclaw-uninstall` | Remove OpenClaw plugin |
| `make copilot-shell-install` | Install copilot-shell hooks |
| `make copilot-shell-uninstall` | Remove copilot-shell hooks |
| `make setup` | Full setup: build + install + OpenClaw plugin |

Override install paths:

```bash
make install INSTALL_DIR=/usr/local/bin
make openclaw-install OPENCLAW_DIR=~/.openclaw/extensions/tokenless
```

## Project Structure

| Path | Description |
|---|---|
| `crates/tokenless-cli/` | CLI binary — `tokenless` command (compress, stats, env-check) |
| `crates/tokenless-schema/` | Core Rust library — `SchemaCompressor` and `ResponseCompressor` |
| `openclaw/` | OpenClaw plugin — TypeScript delegate calling `tokenless` and `rtk` |
| `hooks/copilot-shell/` | copilot-shell hooks — tool-ready, rewrite, response & schema compression |
| `third_party/rtk/` | RTK git submodule — command rewriting engine (70+ commands) |
| `third_party/toon/` | TOON git submodule — JSON to TOON format encoding |
| `scripts/install.sh` | One-step build + install + plugin deployment script |
| `Makefile` | Unified build system for the entire workspace |

## Prerequisites

- **Rust** toolchain >= 1.88 — required by toon submodule (darling, image, time crates). Install via [rustup](https://rustup.rs)
- **Git** — for submodule management

## License

Apache License 2.0 — see [LICENSE](LICENSE).
