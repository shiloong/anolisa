# History Compression Pipeline Design — cosh Agent

**Date**: 2026-04-25
**Author**: Claude (with user input)
**Status**: Draft — includes tokenless integration design

## 1. Problem Statement

In long multi-turn conversations, the cosh (copilot-shell) agent's history context grows linearly, consuming increasing amounts of tokens. The current `ChatCompressionService` uses a single-layer, threshold-triggered approach (compress at 70% of context window, keep only the last 30% of history). This has several shortcomings:

- **No incremental pruning**: History grows unchecked until the 70% threshold is hit, wasting tokens every round.
- **Binary compression**: The entire 70% of history is compressed at once — either a full LLM summarization pass or nothing.
- **High compression cost**: Full-history LLM compression requires sending the entire history as input, costing ~50-200K tokens per compression event.
- **No tool-output control**: Large shell output, repeated file reads, and verbose grep results accumulate unchecked.
- **No phase-awareness**: Compression is purely ratio-based, unaware of the current task phase (exploration, implementation, verification).

**Goal**: Reduce overall token consumption in long conversations while maintaining semantic completeness of critical context.

## 2. Agent Interaction Model Analysis

### 2.1 Recursive Agentic Loop

cosh/clopenclaw-style agents use a **recursive agentic loop** rather than simple request-response. The core flow in `GeminiClient.sendMessageStream()` is:

```
用户输入 → sendMessageStream()
  │
  ├─ ① stripThoughtsFromHistory()   ← 仅在非 continuation 时执行
  ├─ ② tryCompressChat(force=false) ← 70% 阈值触发压缩
  ├─ ③ sessionTokenLimit 检查
  ├─ ④ IDE context injection         ← 全量或增量 delta 注入
  ├─ ⑤ loopDetector.turnStarted()    ← 循环检测
  ├─ ⑥ system-reminders 注入        ← 子 agent/plan 模式提示
  │
  ▼
Turn.run() → LLM 生成流
  │
  ├─ Thought 事件 → UI 渲染
  ├─ Content 事件 → UI 渲染
  ├─ ToolCallRequest → CoreToolScheduler
  │
  ▼
Tool 执行 → ToolResult
  │
  ├─ secret redaction
  ├─ truncateAndSaveToFile() ← 已有 Shell 截断机制！
  ├─ PostToolUse hook
  │
  ▼
handleCompletedTools() → submitQuery(isContinuation=true)
  │
  ▼
sendMessageStream() ←─ 递归调用！
```

### 2.2 Key Findings Affecting Compression Design

| Finding | Impact on Design |
|---------|-----------------|
| **Shell truncation already exists** (`truncateAndSaveToFile` in `coreToolScheduler.ts`) | Layer 2 should **reuse and enhance** existing truncation, not reimplement |
| **Each recursive call injects content** (IDE context delta, system reminders, tool results, thoughts) | Layer 1 must also operate on `sendMessageStream()` continuation calls, not just `addHistory()` |
| **`stripThoughtsFromHistory` only runs on non-continuation** | Should run on every `sendMessageStream()` entry to prune thoughts from recursive calls too |
| **Subagents have independent `GeminiChat` instances** | Compression pipeline must be available to subagent history too |
| **Truncation is configurable** (`getTruncateToolOutputThreshold`, `getTruncateToolOutputLines`) | Layer 2 configuration should extend existing config, not create separate settings |
| **IDE context injection uses delta (incremental) mode** | System-reminder and IDE context dedup is critical — these accumulate per recursive call |
| **Hook system (PreToolUse/PostToolUse) can modify tool output** | Layer 2 must respect hook modifications — summarize AFTER hooks, not before |

### 2.3 Subagent (openclaw-style) Context Flow

Subagents (`Subagent` class) have independent history (`GeminiChat`) and statistics tracking. The main agent only receives the final result from a subagent — not its full history. This means:

- **Subagent token consumption is invisible to the main agent's compression** — subagent must have its own pipeline instance
- **Subagent results entering main history** are typically compact (just a summary string), so Layer 2 on the main agent side focuses on other tool outputs
- **The pipeline should be usable in subagent context** via `Config` injection

## 3. Design Decisions

| Decision | Choice | Reason |
|----------|--------|--------|
| Architecture | Progressive extension (new module, no core class changes) | User preference; allows incremental rollout |
| Trigger timing | Full-session dynamic management (not just threshold) | Every recursive call (continuation) has pruning opportunity, not just at 70% |
| Compression approach | Hybrid: rule-trim + LLM summarization + deep compression | Low-cost rules first, then LLM summary, then deep compression at threshold |
| Integration point | Hook into `GeminiClient.sendMessageStream()`, `CoreToolScheduler`, and subagent flow | Must cover recursive continuation calls, not just `addHistory()` |
| Shell truncation | Reuse existing `truncateAndSaveToFile()` in `CoreToolScheduler` | Avoid reimplementing; extend threshold and add LLM summarization on top |
| Thought stripping | Run `stripThoughtsFromHistory` on every `sendMessageStream()` entry | Recursive continuation calls also produce thoughts that need pruning |
| Subagent coverage | Pipeline available via `Config` injection for subagent history | Subagents consume tokens independently; must have their own pipeline |
| Configuration | `historyPipeline` config alongside existing `chatCompression` | Gradual migration, backward compatible |

## 3. Architecture Overview

### 3.1 Module Relationships

```
┌─────────────────────────────────────────────────────┐
│                    GeminiChat                        │
│  addHistory(content) ──→ HistoryPipeline.process()   │
│  processStreamResponse() ──→ ToolOutputSummarizer    │
├─────────────────────────────────────────────────────┤
│                  HistoryPipeline                     │
│  ┌─────────┐  ┌──────────────┐  ┌─────────────────┐ │
│  │Layer 1  │→ │Layer 2       │→ │Layer 3          │ │
│  │RuleTrim │  │ToolSummarize │  │DeepCompress     │ │
│  └─────────┘  └──────────────┘  └─────────────────┘ │
│    (per-round)   (per-tool)      (threshold)         │
└─────────────────────────────────────────────────────┤
│              HistoryPipelineConfig                   │
│  (per-layer thresholds, toggles, strategy params)    │
└─────────────────────────────────────────────────────┘
```

### 3.2 Layer Responsibilities

| Layer | Component | Trigger | Cost | Purpose |
|-------|-----------|---------|------|---------|
| Layer 1 | `RuleTrimPass` | Per round, after `addHistory()` | 0 tokens | Dedup/truncate/clean redundant content |
| Layer 2 | `ToolOutputSummarizer` | Per tool execution, before result enters history | Low tokens (Flash model) | Summarize large tool outputs |
| Layer 3 | `ChatCompressionService` (enhanced) | Token count > 70% of context window | High tokens | Deep context compression with LLM |

### 4.3 Architecture Relationships (Updated for Agent Interaction Model)

```
┌──────────────────────────────────────────────────────────────┐
│                     GeminiClient                              │
│  sendMessageStream()                                          │
│    ├─ stripThoughtsFromHistory() ← [NEW] run on EVERY call   │
│    ├─ tryCompressChat() ← Layer 3                            │
│    ├─ IDE context injection → [NEW] dedup by HistoryPipeline │
│    ├─ system-reminders injection → [NEW] dedup               │
│    └─ Turn.run() → LLM generation                            │
│                                                                │
│  CoreToolScheduler                                            │
│    ├─ truncateAndSaveToFile() ← [EXISTING] extend + enhance  │
│    ├─ PostToolUse hook                                        │
│    └─ [NEW] ToolOutputSummarizer AFTER hooks                  │
│                                                                │
│  handleCompletedTools() → submitQuery(isContinuation=true)    │
│    └─ [NEW] RuleTrimPass BEFORE recursive call               │
├──────────────────────────────────────────────────────────────┤
│                     HistoryPipeline                            │
│  ┌──────────┐  ┌─────────────────┐  ┌─────────────────────┐  │
│  │ Layer 1  │→ │ Layer 2          │→ │ Layer 3             │  │
│  │ RuleTrim │  │ ToolSummarize    │  │ DeepCompress        │  │
│  └──────────┘  └─────────────────┘  └─────────────────────┘  │
│  (every call)    (after tool+hooks)   (threshold trigger)     │
│                                                                │
│  [NEW] Also injectable into Subagent via Config               │
└──────────────────────────────────────────────────────────────┘
│              HistoryPipelineConfig                             │
│  (per-layer thresholds, toggles, strategy params)             │
└──────────────────────────────────────────────────────────────┘
```

### 4.4 Integration Points (Updated)

| Integration Point | Layer | When | Where |
|-------------------|-------|------|-------|
| `sendMessageStream()` entry | 1 | Every call (including continuation) | `GeminiClient.sendMessageStream()` — before Turn.run() |
| IDE context + system-reminder injection | 1 | Every non-continuation call | `GeminiClient.sendMessageStream()` — after injection, before Turn |
| Tool result processing | 2 | After PostToolUse hooks complete | `CoreToolScheduler._schedule()` — after hook execution, before `convertToFunctionResponse()` |
| Shell truncation enhancement | 2 | Reuse existing `truncateAndSaveToFile()` | `CoreToolScheduler.attemptExecutionOfScheduledCalls()` — extend existing logic |
| Chat compression | 3 | When token > 70% threshold | `GeminiClient.tryCompressChat()` — enhance existing service |
| Subagent history | 1+2 | During subagent execution | `Subagent.run()` — pipeline via `Config` |

## 4. Layer 1 — RuleTrimPass

### 5.1 Trim Rules

| Rule | Description | Implementation | Estimated Savings |
|------|-------------|----------------|-------------------|
| FileRead de-duplication | Same file read multiple times — keep only the latest read, replace earlier reads with a marker | Maintain `readFileCache: Map<filePath, contentIndex>`, replace earlier content with `[READ: path (superseded by turn N)]` | High (long conversations often re-read the same files) |
| Shell output truncation | Shell output exceeding threshold — reuse and enhance existing `truncateAndSaveToFile()` | Extend existing truncation: reduce default threshold, add structured head/tail | Medium |
| System-reminder de-duplication | Duplicate system-reminders — keep only the most recent | Hash-based dedup on content, keep latest instance | **High** (reminders accumulate per recursive call — each continuation can inject the same reminder) |
| IDE context delta dedup | Repeated IDE context injections — keep only the latest full context | Track last IDE context injection index, replace older ones with `[IDE context superseded]` | **Medium-High** (IDE context is injected every `sendMessageStream()` call) |
| Thought part pruning | Strip `thought: true` parts from history on every `sendMessageStream()` call, not just first call | Move `stripThoughtsFromHistory()` to run on every call in `GeminiClient.sendMessageStream()` | Medium (thoughts can be very long) |
| Stale plan cleanup | Steps marked `[DONE]` in plan — compress to single line | Replace completed plan blocks with compact summary | Low |
| Tool error preservation | Tool error messages — never trim | Skip trimming entirely | 0 (preserves semantics) |

### 5.2 Key Design Decisions

- **FileRead de-dup does not delete entries**: Replaces content with a short marker to preserve user/model role alternation structure in history
- **Shell truncation reuses existing `truncateAndSaveToFile()`**: The existing implementation in `CoreToolScheduler` already saves truncated output to a file and returns a pointer. Layer 2 extends this by lowering the threshold and adding LLM summarization as an additional step.
- **System-reminder and IDE context dedup**: These are injected per `sendMessageStream()` call (including continuations), so they accumulate rapidly. The dedup runs at `sendMessageStream()` entry time, before the request is sent to the LLM.
- **Thought stripping on every call**: Currently `stripThoughtsFromHistory()` only runs on non-continuation calls. Moving it to every call ensures thoughts from recursive tool-result rounds are also pruned.
- **All trim operations record metadata**: Add `{text: "...", metadata: {trimmedFrom: "original_content_hash"}}` in Content parts for audit trail

### 5.3 Core Interface

```typescript
interface RuleTrimConfig {
  maxShellOutputLines: number;        // default: 100 (extends existing config)
  readFileDeDup: boolean;             // default: true
  systemReminderDeDup: boolean;       // default: true
  ideContextDeDup: boolean;           // default: true
  stripThoughtsEveryCall: boolean;    // default: true (move stripThoughts to every call)
}

class RuleTrimPass {
  private readFileCache: Map<string, number>; // filePath -> history index
  private lastSystemReminderHash: string;     // dedup reminders per sendMessageStream call
  private lastIdeContextIndex: number;        // dedup IDE context injections

  process(history: Content[]): Content[] {
    // 1. Scan history, build readFileCache
    // 2. Dedup system-reminders (keep latest per content hash)
    // 3. Dedup IDE context injections (keep latest full context)
    // 4. Strip thought parts if stripThoughtsEveryCall=true
    // 5. Mark positions to be trimmed/deduped
    // 6. Return trimmed history
  }
}
```

## 5. Layer 2 — ToolOutputSummarizer

### 6.1 Enhancements over Existing Summarizer & Truncation

Current `summarizer.ts` provides LLM summarization for tool outputs > 2000 tokens. Current `truncateAndSaveToFile()` in `CoreToolScheduler` provides shell output truncation with file-save. Enhancements:

| Enhancement | Description | Integration Point |
|-------------|-------------|-------------------|
| Lower truncation threshold | Extend existing `truncateAndSaveToFile()` — reduce default from 10000 chars to configurable value | `CoreToolScheduler.attemptExecutionOfScheduledCalls()` |
| LLM summarization after truncation | After truncation saves to file, optionally use LLM to summarize the saved content for inline context | New step after existing truncation |
| Category-specific strategies | Different summarization instructions per tool type (extending existing `llmSummarizer`) | `CoreToolScheduler` — after PostToolUse hooks |
| Fallback mechanism | On LLM summarization failure, keep truncated content (not original) | Truncation is always the base; LLM summary is additive |
| Subagent result summarization | When subagent returns results to main agent, summarize if > threshold | `Subagent.run()` — after execution, before returning to main agent |

**Key principle**: Layer 2 operates AFTER hooks (PreToolUse, PostToolUse) have completed, so hook modifications to tool output are respected. The summarization is applied to the final tool result after all hook transformations.

### 6.2 Category-Specific Summarization Strategies

| Tool Type | Summarization Instruction | Preserved Content |
|-----------|--------------------------|-------------------|
| `shell` | "Extract command name, key results, error stack, warnings" | Command + result summary + full error stack |
| `read_file` / `read_many_files` | "Extract key file structure: imports, class/function signatures, key config items" | File path + structure summary |
| `grep` / `glob` | "Keep only match result summary: file count + line count statistics" | Match statistics + key match lines |
| `edit` | "Keep edit location and intent, remove diff context" | File path + edit summary |
| `web_search` / `web_fetch` | "Extract key findings and conclusions" | Core information summary |

### 6.3 Core Interface

```typescript
interface ToolOutputSummarizeConfig {
  tokenThreshold: number;           // default: 800 (extends existing truncation threshold)
  useFlashModel: boolean;           // default: true
  enabledTools: string[];           // default: all
  extendExistingTruncation: boolean;// default: true (reuses truncateAndSaveToFile)
  summarizeAfterHooks: boolean;     // default: true (apply after PostToolUse hooks)
}

class ToolOutputSummarizer {
  summarize(result: ToolResult, context: Content[], toolName: string): Promise<ToolResult> {
    // 1. Check if truncation already applied (existing truncateAndSaveToFile)
    // 2. Estimate result token count
    // 3. If < threshold, return original result
    // 4. Select summarization strategy based on tool name
    // 5. Call LLM summarization (Flash model)
    // 6. On LLM failure, keep truncated content (not original)
    // 7. Return summarized ToolResult
  }
}
```

### 6.4 Execution Timing

Layer 2 operates at two levels:

1. **Shell truncation** (existing): In `CoreToolScheduler.attemptExecutionOfScheduledCalls()`, after `PostToolUse` hooks complete, the existing `truncateAndSaveToFile()` is called. This remains unchanged.

2. **LLM summarization** (new): After truncation is applied (or if no truncation was needed but content exceeds the new lower threshold), the `ToolOutputSummarizer` is called. This happens **after hooks** to respect any modifications hooks made to the tool output.

3. **Subagent result summarization**: When a subagent completes execution in `Subagent.run()`, its result is summarized if it exceeds the threshold before being returned to the main agent.

### 6.5 Relationship with Existing Truncation

```
ToolResult (原始)
  │
  ├─ PostToolUse hook → 可能修改/添加上下文
  │
  ├─ truncateAndSaveToFile() → 截断 + 保存文件 (现有逻辑)
  │   └─ 返回截断内容 + 文件路径指针
  │
  ├─ [NEW] ToolOutputSummarizer → LLM摘要截断内容 (可选)
  │   └─ 失败时保持截断内容 (不回退到原始)
  │
  ▼
最终 ToolResult → convertToFunctionResponse() → 写入 history
```

## 6. Layer 3 — Deep Compression Enhancement

### 6.1 Improvements to Existing ChatCompressionService

Keep the existing 70% threshold trigger, but enhance compression quality:

| Improvement | Description |
|-------------|-------------|
| Pre-trim before compress | Before deep compression, run Layer 1 RuleTrim on the history-to-compress, reducing LLM summarization input cost |
| Enhanced state_snapshot structure | Add `<tool_context>` and `<active_files>` fields to the XML snapshot |
| Incremental compression | Instead of re-summarizing the entire history each time, merge new turns into the existing snapshot |
| Compression quality validation | After compression, verify that key info is retained (current work files, unfinished plans) |

### 6.2 Enhanced state_snapshot Structure

```xml
<state_snapshot>
    <overall_goal>...</overall_goal>

    <key_knowledge>
        <!-- Existing: key facts and constraints -->
    </key_knowledge>

    <tool_context>               <!-- NEW -->
        <!-- Tool execution environment info -->
        <!-- e.g.: installed dependencies, command aliases, git status -->
    </tool_context>

    <active_files>               <!-- NEW (extracted from file_system_state) -->
        <!-- Currently active files and their latest status -->
        <!-- e.g.: MODIFIED: auth.ts - replaced JWT library -->
    </active_files>

    <file_system_state>
        <!-- Existing: file change history (simplified) -->
    </file_system_state>

    <recent_actions>...</recent_actions>
    <current_plan>...</current_plan>
</state_snapshot>
```

### 6.3 Incremental Compression Mechanism

```typescript
class ChatCompressionService {
  async compress(chat, promptId, force, model, config, hasFailed): Promise<...> {
    // 1. Get curated history
    const curatedHistory = chat.getHistory(true);

    // 2. [NEW] Pre-trim before compression
    const trimmedHistory = pipeline.trimBeforeCompress(curatedHistory);

    // 3. Find split point (existing logic)
    const splitPoint = findCompressSplitPoint(trimmedHistory, ...);

    // 4. [NEW] Check for previous compression snapshot
    const previousSnapshot = this.findPreviousSnapshot(curatedHistory);

    // 5. [NEW] Build compression input: previous snapshot + new history since last compression
    const compressionInput = previousSnapshot
      ? [previousSnapshot, ...newHistorySinceLastCompression]
      : historyToCompress;

    // 6. Call LLM compression (existing logic + enhanced prompt)
    // 7. [NEW] Validate compression quality
    // 8. Return compression result
  }
}
```

### 6.4 Incremental Compression Cost Savings

| Scenario | Full Compression Cost | Incremental Compression Cost | Savings |
|----------|----------------------|------------------------------|---------|
| 10 rounds, 1st compression | ~50K input tokens | ~50K input (no incremental data) | 0% |
| 20 rounds, 2nd compression | ~120K input tokens | ~30K input (only incremental) | ~75% |
| 30 rounds, 3rd compression | ~200K input tokens | ~40K input (only incremental) | ~80% |

## 7. Dynamic Context Window Management

### 7.1 Phase-Aware Relevance Scoring

```typescript
class DynamicContextManager {
  detectPhase(history: Content[]): TaskPhase {
    // 'exploration': searching files, reading code → keep recent file reads
    // 'implementation': editing files, running commands → keep current file context
    // 'verification': running tests → keep error info and fix history
    // 'planning': making plans → keep all planning-related discussion
  }

  trimByRelevance(history: Content[], phase: TaskPhase, budget: TokenBudget): Content[] {
    // Calculate relevance score for each Content
    // Sort by score, keep most relevant within budget
  }
}
```

### 7.2 Relevance Scoring Rules

| Factor | Weight | Description |
|--------|--------|-------------|
| Temporal proximity | 0.3 | More recent = more relevant |
| Topic match | 0.3 | Content matching current task phase |
| Tool type match | 0.2 | Priority for outputs from phase-relevant tools |
| File association | 0.2 | Priority for context about currently active files |

### 7.3 Integration with Layer 3

When deep compression is triggered, `DynamicContextManager.trimByRelevance()` is called first to select which history to keep (beyond the 30% fixed ratio). This replaces the rigid `COMPRESSION_PRESERVE_THRESHOLD = 0.3` with a phase-aware selection.

## 8. Overall Data Flow (Updated for Recursive Agent Loop)

```
用户输入 → GeminiClient.sendMessageStream(isContinuation=false)
  │
  ├─ [NEW] stripThoughtsFromHistory() ← 每次调用都执行
  ├─ tryCompressChat() → Layer 3 检查阈值
  │   ├─ [NEW] pipeline.trimBeforeCompress() → Layer 1 预裁剪
  │   └─ [NEW] 增量压缩（使用上次 snapshot）
  │
  ├─ IDE context injection → [NEW] dedup by RuleTrimPass
  ├─ system-reminders injection → [NEW] dedup by RuleTrimPass
  ├─ [NEW] RuleTrimPass.process(history) → Layer 1 裁剪后 history
  │
  ▼
Turn.run() → LLM 生成
  │
  ├─ Thought → UI (思考部分，后续 stripThoughts 清理)
  ├─ Content → UI (流式文本)
  ├─ ToolCallRequest → CoreToolScheduler
  │
  ▼
CoreToolScheduler:
  │
  ├─ PreToolUse hook → 修改/阻止/确认
  ├─ Tool 执行 → ToolResult
  ├─ truncateAndSaveToFile() → 现有截断 (Shell)
  ├─ PostToolUse hook → 修改/添加上下文
  ├─ [NEW] ToolOutputSummarizer → Layer 2 LLM摘要 (AFTER hooks)
  │
  ▼
handleCompletedTools() → submitQuery(isContinuation=true)
  │
  ├─ [NEW] RuleTrimPass.process(history) → Layer 1 (continuation 轮也裁剪)
  │
  ▼
sendMessageStream(isContinuation=true) ← 递归调用
  │
  ├─ [NEW] stripThoughtsFromHistory()
  ├─ tryCompressChat() → Layer 3 (可能在递归轮也触发)
  │
  ▼
最终返回 → 等待下一用户输入

子 Agent 流程:
Subagent.run() → 独立 GeminiChat
  │
  ├─ [NEW] HistoryPipeline via Config → 同样的三层管道
  ├─ Subagent 完成 → 返回结果给主 Agent
  ├─ [NEW] ToolOutputSummarizer → 摘要子 Agent 返回结果 (如超过阈值)
```

## 9. Configuration Schema

```typescript
interface HistoryPipelineConfig {
  enabled: boolean;                    // Master toggle, default: true

  layer1: {
    enabled: boolean;                  // RuleTrim toggle, default: true
    maxShellOutputLines: number;       // default: 100 (extends existing truncate config)
    readFileDeDup: boolean;            // default: true
    systemReminderDeDup: boolean;      // default: true
    ideContextDeDup: boolean;          // default: true
    stripThoughtsEveryCall: boolean;   // default: true (move stripThoughts to every call)
  };

  layer2: {
    enabled: boolean;                  // ToolOutput summary toggle, default: true
    tokenThreshold: number;            // default: 800
    useFlashModel: boolean;            // default: true
    extendExistingTruncation: boolean; // default: true (reuses truncateAndSaveToFile)
    summarizeAfterHooks: boolean;      // default: true
    subagentResultThreshold: number;   // default: 2000 (summarize subagent results if > threshold)
  };

  layer3: {
    enabled: boolean;                  // Deep compression toggle, default: true
    contextPercentageThreshold: number;// default: 0.7 (existing)
    incrementalCompression: boolean;   // Incremental compression toggle, default: true
    preTrimBeforeCompress: boolean;    // Pre-trim before compression, default: true
  };
}
```

Configuration is set via `settings.json`'s `historyPipeline` field, alongside the existing `chatCompression` config. This enables gradual migration — both systems coexist until `historyPipeline.enabled` is confirmed stable, at which point `chatCompression` can be deprecated.

## 10. File Structure

New files to create within `src/copilot-shell/packages/core/src/`:

```
services/
  historyPipeline/
    HistoryPipeline.ts               # Pipeline orchestrator
    HistoryPipeline.test.ts
    RuleTrimPass.ts                   # Layer 1: rule-based trimming
    RuleTrimPass.test.ts
    ToolOutputSummarizer.ts           # Layer 2: tool output summarization
    ToolOutputSummarizer.test.ts
    DynamicContextManager.ts          # Phase-aware context management
    DynamicContextManager.test.ts
    types.ts                          # Shared types and configs
    index.ts                          # Module exports
```

Modified files:

```
core/client.ts                         # [NEW] stripThoughts on every call, RuleTrim on sendMessageStream entry
core/coreToolScheduler.ts              # [NEW] ToolOutputSummarizer AFTER hooks, extend truncateAndSaveToFile
services/chatCompressionService.ts     # Add incremental compression, pre-trim
core/geminiChat.ts                     # HistoryPipeline integration
core/prompts.ts                        # Enhanced state_snapshot prompt
config/config.ts                        # Add HistoryPipelineConfig to settings
subagents/subagent.ts                  # [NEW] Pipeline integration for subagent history
```

## 11. Testing Strategy

### 11.1 Unit Tests (per layer)

- **RuleTrimPass**: Test each trim rule independently (file-read dedup, shell truncation, reminder dedup)
- **ToolOutputSummarizer**: Test each tool category strategy, LLM failure fallback, threshold logic
- **DynamicContextManager**: Test phase detection, relevance scoring, budget-based selection
- **ChatCompressionService**: Test incremental compression, pre-trim integration, quality validation

### 11.2 Integration Tests

- **Pipeline orchestration**: Verify Layer 1→2→3 ordering, config toggles (disable individual layers)
- **History consistency**: Verify user/model role alternation preserved after all trimming
- **Long conversation simulation**: Simulate 30+ turn conversations, measure token savings vs baseline

### 11.3 Regression Tests

- **Semantic completeness**: After compression, verify the agent can still correctly answer questions about earlier context
- **No critical info loss**: Verify error messages, active files, and current plans are preserved
- **No history corruption**: Verify role alternation structure (user→model→user→model) is never broken

## 12. Expected Token Savings

Estimated savings based on typical long-conversation scenarios (20-30 turns):

| Source | Baseline Tokens | After Layer 1 | After Layer 2 | After Layer 3 | Total Savings |
|--------|----------------|---------------|---------------|---------------|---------------|
| Repeated file reads | ~8K | ~2K (dedup) | ~1.5K | unchanged | ~81% |
| Shell outputs | ~15K | ~12K (extend existing truncation) | ~5K (LLM summary) | ~2K | ~87% |
| System-reminders + IDE context (per recursive call) | ~3K/call × ~10 calls = ~30K | ~5K (dedup) | unchanged | ~2K | ~93% |
| Grep/glob results | ~6K | ~4K | ~1.5K (summary) | unchanged | ~75% |
| Thought parts (pruned every call) | ~10K | ~0K (stripped) | unchanged | unchanged | ~100% |
| Subagent results | ~5K | ~3K | ~1K (summary) | unchanged | ~80% |
| Full history compression | ~200K input | ~150K (pre-trim) | ~150K | ~30K (incremental) | ~85% |
| Overall context per round | ~60K avg | ~30K | ~25K | ~15K after compress | ~75% |

These are estimates; actual savings will be measured during implementation and validation.

## 13. Migration Plan

1. **Phase 1**: Implement `RuleTrimPass` (Layer 1) as standalone. Key changes: move `stripThoughtsFromHistory()` to every `sendMessageStream()` call, add system-reminder and IDE context dedup. Validate independently.
2. **Phase 2**: Implement `ToolOutputSummarizer` (Layer 2) by extending existing `truncateAndSaveToFile()` and adding LLM summarization after PostToolUse hooks. Add subagent result summarization. Validate independently.
3. **Phase 3**: Enhance `ChatCompressionService` with pre-trim and incremental compression (Layer 3). Validate against existing tests.
4. **Phase 4**: Implement `DynamicContextManager` and integrate all layers in `HistoryPipeline`. Wire pipeline into `GeminiClient.sendMessageStream()` and `Subagent.run()`.
5. **Phase 5**: Enable `historyPipeline` config by default, deprecate `chatCompression` config.

## 14. tokenless Integration Design

### 14.1 Overview

The three-layer History Compression Pipeline integrates into tokenless as a fourth capability alongside the existing Schema Compression, Response Compression, and Command Rewriting (RTK). Integration leverages tokenless's existing patterns:

- **Rust crate**: New `tokenless-history` crate in the workspace
- **CLI commands**: New `trim-history` and `compress-history` subcommands
- **copilot-shell hooks**: New BeforeModel/PreCompact shell scripts
- **OpenClaw plugin**: Extended hooks in `index.ts`
- **Stats recording**: Auto-record compression metrics via existing `tokenless-stats`

### 14.2 Layer-to-Integration Mapping

| Pipeline Layer | tokenless Integration | Trigger Event | Target Agent |
|---------------|----------------------|--------------|-------------|
| Layer 1 (RuleTrim) | `tokenless trim-history` CLI → BeforeModel hook / OpenClaw `before_model` | Every model call (before LLM request) | cosh, openclaw |
| Layer 2 (ToolOutput) | `tokenless compress-response --tool-name` CLI → PostToolUse hook / OpenClaw `tool_result_persist` | After tool execution + hooks | cosh, openclaw |
| Layer 3 (DeepCompress) | `tokenless compress-history` CLI → PreCompact hook / OpenClaw `before_model` (priority 20) | Token count > threshold | cosh, openclaw |

### 14.3 New Rust Crate: `tokenless-history`

```
crates/tokenless-history/
  Cargo.toml                # Dependencies: serde_json, regex, chrono
  src/
    lib.rs                  # Public exports: HistoryCompressor, TrimEngine
    rule_trim.rs            # Layer 1: Rule-based trimming engine
    tool_strategy.rs        # Layer 2: Tool-type-aware summarization strategies
    deep_compressor.rs      # Layer 3: Deep compression (state_snapshot XML)
    phase_detector.rs       # Phase detection: exploration/implementation/verification
    types.rs                # HistoryContent, TrimRule, Phase, CompressionSnapshot
```

**Key types**:

```rust
/// A message in the LLM conversation history
#[derive(Serialize, Deserialize)]
pub struct HistoryMessage {
    role: String,          // "user" | "model" | "system"
    content: String,       // Text content (or structured content)
}

/// Trim rules configuration
pub struct TrimRules {
    file_read_dedup: bool,
    system_reminder_dedup: bool,
    ide_context_dedup: bool,
    thought_strip: bool,
    max_shell_lines: usize,
}

/// Task phase for relevance scoring
pub enum Phase {
    Exploration,
    Implementation,
    Verification,
    Planning,
}

/// Compression snapshot for incremental compression
pub struct CompressionSnapshot {
    xml: String,           // <state_snapshot> XML content
    timestamp: DateTime<Local>,
    history_index: usize,  // Position where this snapshot was created
}
```

### 14.4 New CLI Commands

**`tokenless trim-history`** — Layer 1 rule-based trimming:

```bash
tokenless trim-history \
  [--file FILE | stdin] \
  --agent-id <agent> \
  [--session-id <session>] \
  [--rules <comma-separated>] \    # default: all enabled
  [--max-shell-lines <N>]          # default: 100
```

Input: JSON array of `{ role, content }` messages (from BeforeModel `llm_request.messages`)
Output: JSON array of trimmed messages (same structure, with dedup markers replacing redundant content)

Processing logic:
1. **File-read dedup**: Scan for repeated file-read content; keep only the latest, replace earlier with `[READ: path (superseded by turn N)]`
2. **System-reminder dedup**: Hash content of system-role messages; keep only the latest instance of each unique content
3. **IDE context dedup**: Track last full IDE context injection; replace older ones with `[IDE context superseded]`
4. **Thought strip**: Remove content segments that appear to be thought parts (lines starting with `thought:` or containing `thoughtSignature`)
5. **Shell output truncation**: Truncate content blocks from `exec` tool results exceeding `max-shell-lines`, keeping head + tail + `[... N lines truncated]`

**`tokenless compress-history`** — Layer 3 deep compression:

```bash
tokenless compress-history \
  [--file FILE | stdin] \
  --agent-id <agent> \
  [--session-id <session>] \
  [--previous-snapshot <FILE>] \   # Incremental: use previous snapshot
  [--preserve-ratio <0.0-1.0>] \   # default: 0.3
  [--phase <phase>]                # default: auto-detect
```

Input: JSON array of messages (full history)
Output: `<state_snapshot>` XML string (to be injected as `additionalContext`)

Processing logic:
1. **Phase detection**: Analyze recent messages to determine current task phase
2. **Pre-trim**: Run Layer 1 trim on history-to-compress (reduces LLM input cost)
3. **Find split point**: Determine compression boundary based on `preserve_ratio`
4. **Incremental compression**: If `previous-snapshot` provided, only compress history since last snapshot
5. **Generate `<state_snapshot>` XML**: Produce enhanced snapshot with `<tool_context>` and `<active_files>` fields

**`tokenless compress-response` extension** — Layer 2 tool-aware summarization:

The existing `compress-response` command gets two new optional flags:
- `--tool-name <tool>`: Select summarization strategy based on tool type
- `--summarize-strategy <auto|raw>`: `auto` = tool-aware strategy, `raw` = existing ResponseCompressor behavior

### 14.5 New copilot-shell Hooks

**`hooks/copilot-shell/tokenless-trim-history.sh`** — BeforeModel hook:

```bash
#!/usr/bin/env bash
# tokenless-hook-version: 7
# Token-Less copilot-shell hook — trims redundant history messages.
# Hook event: BeforeModel

# Read input, extract llm_request.messages
MESSAGES=$(echo "$INPUT" | jq -c '.llm_request.messages // empty')

# Call tokenless trim-history
TRIMMED=$(echo "$MESSAGES" | tokenless trim-history \
  --agent-id copilot-shell \
  ${SESSION_ID:+--session-id "$SESSION_ID"} \
  2>/dev/null)

# Build response with trimmed messages
jq -n \
  --argjson messages "$TRIMMED" \
  '{
    "hookSpecificOutput": {
      "hookEventName": "BeforeModel",
      "llm_request": {
        "messages": $messages
      }
    }
  }'
```

**`hooks/copilot-shell/tokenless-compress-history.sh`** — PreCompact hook:

```bash
#!/usr/bin/env bash
# tokenless-hook-version: 7
# Token-Less copilot-shell hook — deep history compression.
# Hook event: PreCompact

# Read input, extract trigger and session context
MESSAGES=$(echo "$INPUT" | jq -c '.llm_request.messages // empty')

# Call tokenless compress-history
SNAPSHOT=$(echo "$MESSAGES" | tokenless compress-history \
  --agent-id copilot-shell \
  ${SESSION_ID:+--session-id "$SESSION_ID"} \
  2>/dev/null)

# Build response with snapshot as additionalContext
jq -n \
  --arg snapshot "$SNAPSHOT" \
  '{
    "hookSpecificOutput": {
      "hookEventName": "PreCompact",
      "additionalContext": $snapshot
    }
  }'
```

### 14.6 OpenClaw Plugin Extension

The existing `openclaw/index.ts` plugin gains two new hooks:

```typescript
// 3. History trimming (before_model, priority 5 — runs before schema compression)
if (historyTrimEnabled && checkTokenless()) {
  api.on(
    "before_model",
    (event, ctx) => {
      const messages = event.llm_request?.messages;
      if (!messages || messages.length < 4) return; // Skip very short conversations

      const trimmed = tryTrimHistory(messages, ctx?.sessionId);
      if (!trimmed) return;

      return {
        hookSpecificOutput: {
          hookEventName: "BeforeModel",
          llm_request: { messages: trimmed },
        },
      };
    },
    { priority: 5 },
  );
}

// 4. Deep history compression (before_model, priority 20 — runs after trim)
if (deepCompressionEnabled && checkTokenless()) {
  api.on(
    "before_model",
    (event, ctx) => {
      const messages = event.llm_request?.messages;
      if (!messages) return;

      // Estimate token count; compress only if > threshold
      const estimatedTokens = estimateTokensFromMessages(messages);
      const contextWindow = 128000;
      if (estimatedTokens < contextWindow * 0.7) return; // Skip if below threshold

      const snapshot = tryCompressHistory(messages, ctx?.sessionId);
      if (!snapshot) return;

      return {
        hookSpecificOutput: {
          hookEventName: "BeforeModel",
          llm_request: {
            messages: [
              ...messages.slice(-Math.floor(messages.length * 0.3)),
              { role: "system", content: snapshot },
            ],
          },
        },
      };
    },
    { priority: 20 },
  );
}
```

Plugin config schema extension (`openclaw.plugin.json`):

```json
{
  "configSchema": {
    "properties": {
      "rtk_enabled": { "type": "boolean", "default": true },
      "schema_compression_enabled": { "type": "boolean", "default": true },
      "response_compression_enabled": { "type": "boolean", "default": true },
      "history_trim_enabled": { "type": "boolean", "default": true },
      "deep_compression_enabled": { "type": "boolean", "default": true },
      "verbose": { "type": "boolean", "default": false }
    }
  },
  "uiHints": {
    "history_trim_enabled": { "label": "Enable history rule trimming" },
    "deep_compression_enabled": { "label": "Enable deep history compression" }
  }
}
```

### 14.7 Responsibility Boundary: tokenless vs cosh Internal

Critical principle: **tokenless provides hook-level compression (operates on LLM request/response payloads), cosh internal implementation handles state-level operations that require access to GeminiChat internals.**

| Function | Responsibility | Reason |
|----------|---------------|--------|
| `stripThoughtsFromHistory()` | **cosh internal** (TypeScript) | Modifies `GeminiChat.history` internal state; hooks only see the serialized `messages` |
| Shell truncation (`truncateAndSaveToFile`) | **cosh internal** (TypeScript) | Writes truncated output to disk file; requires filesystem access during tool execution |
| IDE context delta dedup | **tokenless BeforeModel hook** | Operates on `llm_request.messages`; can replace older IDE context with markers |
| System-reminder dedup | **tokenless BeforeModel hook** | Operates on `llm_request.messages`; can dedup reminder content |
| File-read content dedup | **tokenless BeforeModel hook** | Operates on `llm_request.messages`; can replace earlier reads with markers |
| Thought content strip (from messages) | **tokenless BeforeModel hook** | Operates on serialized messages; removes thought-like content blocks |
| Tool output LLM summarization | **tokenless PostToolUse hook** | Operates on `tool_response`; extended with `--tool-name` strategy |
| Deep compression | **tokenless BeforeModel/PreCompact hook** | Operates on `messages`; generates `<state_snapshot>` XML |
| Phase detection | **tokenless Rust crate** | Pure logic; analyzes message patterns to determine phase |
| Subagent result summarization | **tokenless (via OpenClaw)** | OpenClaw's `tool_result_persist` hook already handles subagent results |

### 14.8 Stats Recording Extension

The existing `OperationType` enum in `tokenless-stats/src/record.rs` gains two new variants:

```rust
pub enum OperationType {
    CompressSchema,       // Existing
    CompressResponse,     // Existing
    RewriteCommand,       // Existing
    TrimHistory,          // NEW — Layer 1
    CompressHistory,      // NEW — Layer 3
}
```

Stats recording follows the existing pattern — auto-record in CLI commands, fail-silent so compression output is never blocked by database errors.

### 14.9 Updated File Structure

New files in `src/tokenless/`:

```
crates/tokenless-history/
  Cargo.toml
  src/
    lib.rs
    rule_trim.rs
    tool_strategy.rs
    deep_compressor.rs
    phase_detector.rs
    types.rs

hooks/copilot-shell/
  tokenless-trim-history.sh          # NEW — BeforeModel hook
  tokenless-compress-history.sh      # NEW — PreCompact hook
  tokenless-compress-response.sh     # EXISTING (extended with --tool-name)
  tokenless-compress-schema.sh       # EXISTING
  tokenless-rewrite.sh               # EXISTING

openclaw/
  index.ts                           # MODIFIED (add before_model hooks)
  openclaw.plugin.json               # MODIFIED (add config fields)
```

Modified files in `src/tokenless/`:

```
Cargo.toml                           # Add tokenless-history to workspace members
crates/tokenless-cli/
  src/main.rs                        # Add trim-history and compress-history subcommands
  Cargo.toml                         # Add tokenless-history dependency
crates/tokenless-stats/
  src/record.rs                      # Add TrimHistory and CompressHistory to OperationType
Makefile                              # Add tokenless-history to build targets
```

### 14.10 Updated Data Flow (with tokenless Hooks)

```
BeforeModel hook payload (cosh):
{
  "session_id": "abc123",
  "llm_request": {
    "model": "gemini-2.5-flash",
    "messages": [ { role, content }, ... ],
    "config": { "tools": [...] }
  }
}

→ tokenless-trim-history.sh (BeforeModel hook)
  → tokenless trim-history (CLI)
    → Layer 1: dedup file reads, reminders, IDE context, strip thoughts
  → Return trimmed messages in hookSpecificOutput.llm_request.messages

→ tokenless-compress-schema.sh (BeforeModel hook, existing)
  → Schema compression on tools array
  → Return compressed tools in hookSpecificOutput.llm_request.tools

PostToolUse hook payload (cosh):
{
  "session_id": "abc123",
  "tool_name": "shell",
  "tool_response": { "result": "...long output..." }
}

→ tokenless-compress-response.sh (PostToolUse hook, existing + extended)
  → tokenless compress-response --tool-name shell (CLI)
    → Layer 2: tool-aware summarization strategy
  → Return compressed response as additionalContext

PreCompact hook payload (cosh):
{
  "session_id": "abc123",
  "trigger": "auto"
}

→ tokenless-compress-history.sh (PreCompact hook)
  → tokenless compress-history (CLI)
    → Layer 3: deep compression with incremental snapshot
  → Return <state_snapshot> XML as additionalContext

OpenClaw agent flow:
before_model event
  → tokenless trim-history (Layer 1, priority 5)
  → tokenless compress-schema (existing, priority 10)
  → tokenless compress-history (Layer 3, priority 20, threshold-triggered)

tool_result_persist event
  → tokenless compress-response (Layer 2, existing + --tool-name)
```

### 14.11 Build & Deployment

Updated `Makefile` targets:
- `build-tokenless` now also builds `tokenless-history` crate
- `copilot-shell-install` copies the two new hook scripts
- `openclaw-install` handles the updated plugin with new config fields
- `setup` installs all components including new hooks

Updated `Cargo.toml` workspace:
```toml
members = [
    "crates/tokenless-schema",
    "crates/tokenless-cli",
    "crates/tokenless-stats",
    "crates/tokenless-history",
]
```

### 14.12 Token Savings Estimates (with tokenless Hooks)

| Source | Baseline | After tokenless Layer 1 (BeforeModel hook) | After tokenless Layer 2 (PostToolUse hook) | After tokenless Layer 3 (PreCompact hook) |
|--------|----------|---------------------------------------------|--------------------------------------------|--------------------------------------------|
| File-read dedup | ~8K | ~2K (dedup markers) | unchanged | unchanged |
| System-reminders + IDE context | ~30K (10 recursive calls) | ~5K (dedup) | unchanged | ~2K (in snapshot) |
| Thought parts (stripped from messages) | ~10K | ~0K (stripped) | unchanged | unchanged |
| Shell output | ~15K | ~12K (truncation) | ~5K (tool-aware summary) | ~2K (in snapshot) |
| Grep/glob results | ~6K | ~4K | ~1.5K | unchanged |
| Full history (deep compression) | ~200K input | ~150K (pre-trim) | ~150K | ~30K (incremental snapshot) |
| **Overall per round** | ~60K avg | ~30K | ~25K | ~15K after compress |

### 14.13 Integration Test Strategy

**Hook-level tests** (shell script tests):
1. `tokenless-trim-history.sh` — test with synthetic BeforeModel payload containing repeated file reads, duplicate reminders, and thought content
2. `tokenless-compress-response.sh` — test with `--tool-name shell` flag, verify tool-aware summarization
3. `tokenless-compress-history.sh` — test with PreCompact payload, verify `<state_snapshot>` XML output

**Rust crate tests** (unit tests):
1. `rule_trim.rs` — test each trim rule independently (file-read dedup, reminder dedup, thought strip)
2. `deep_compressor.rs` — test incremental compression with previous snapshot
3. `phase_detector.rs` — test phase detection from message patterns

**Integration tests** (cosh + tokenless):
1. Configure all tokenless hooks in cosh `settings.json`
2. Run a 20+ turn conversation simulation
3. Measure token consumption vs baseline (without hooks)
4. Verify semantic completeness: agent can still answer questions about earlier context