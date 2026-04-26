---
name: ws-ckpt
description: >
  【仅限 OpenClaw 环境】ws-ckpt 工作区快照管理。用户说“保存一下”、“存个快照”时创建 openclaw 工作区 checkpoint；
  说“回滚”、“撤销”、“恢复到之前”时 rollback；说“删掉快照”时 delete；
  说“看看快照”、“有哪些快照”时 list；说“查看快照状态”、“查看快照剩余空间”时 status。
---

# ws-ckpt 工作区快照管理（OpenClaw Skill）

本 skill 仅适配 **OpenClaw**。基于 btrfs COW 快照，为工作区提供微秒级 checkpoint/rollback。非 OpenClaw 场景请勿使用

## 工作区路径

执行 skill 前，先检查 `~/.openclaw/workspace` 是否存在：

- **存在**：所有命令的 `-w` 参数统一使用 `~/.openclaw/workspace`
- **不存在**：停止执行，告知用户「ws-ckpt skill 仅支持 OpenClaw 环境，当前未检测到工作区路径 ~/.openclaw/workspace」

## 触发规则

| 用户说 | 执行命令 | 说明 |
|--------|----------|------|
| "保存一下"、"存个快照"、"checkpoint"、"备份当前状态" | `checkpoint` | 创建快照 |
| "回滚"、"撤销"、"恢复到之前"、"rollback"、"改坏了" | `rollback` | 回滚到指定快照 |
| "删掉快照"、"清理快照"、"delete snapshot" | `delete` | 删除指定快照 |
| "看看快照"、"有哪些快照"、"list"、"列一下" | `list` | 列出快照 |
| "状态"、"空间"、"status"、"工作区怎么样" | `status` | 查看工作区状态 |

## 命令用法

### checkpoint — 创建快照

```bash
ws-ckpt checkpoint -w <workspace> -i <id> [-m <message>]
```

- `-w`：工作区路径（必填）
- `-i`：快照 ID，自定义名称，同一工作区内唯一（必填）
- `-m`：快照描述（可选）

```bash
ws-ckpt checkpoint -w ~/.openclaw/workspace -i before-refactor -m "重构前备份"
```

### rollback — 回滚到快照

```bash
ws-ckpt rollback -w <workspace> -s <snapshot>
```

- `-w`：工作区路径（快照 ID 全局唯一时可省略）
- `-s`：目标快照 ID（必填）

```bash
ws-ckpt rollback -s before-refactor
ws-ckpt rollback -w ~/.openclaw/workspace -s before-refactor
```

### delete — 删除快照

```bash
ws-ckpt delete -s <snapshot> [--force] [-w <workspace>]
```

- `-s`：要删除的快照 ID（必填）
- `--force`：跳过确认
- `-w`：快照 ID 跨工作区重复时必须指定

```bash
ws-ckpt delete -s old-snap --force
```

### list — 列出快照

```bash
ws-ckpt list [-w <workspace>] [--format table|json]
```

- 省略 `-w` 列出所有工作区的快照

```bash
ws-ckpt list
ws-ckpt list -w ~/.openclaw/workspace
ws-ckpt list --format json
```

### status — 查看状态

```bash
ws-ckpt status [-w <workspace>]
```

- 省略 `-w` 查看全局状态

```bash
ws-ckpt status
ws-ckpt status -w ~/.openclaw/workspace
```

## 注意事项

- checkpoint 用 `-i` 指定快照 ID；rollback 和 delete 用 `-s` 指定快照 ID，不要混淆
- daemon 必须运行中（`systemctl status ws-ckpt` 确认），否则所有命令报连接错误
- 执行破坏性操作前务必先 checkpoint
