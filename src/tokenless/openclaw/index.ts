/**
 * Token-Less Unified Plugin for OpenClaw v5
 *
 * Combines two complementary optimisation strategies into a single plugin:
 *
 *   1. RTK command rewriting  — transparently rewrites exec tool commands to
 *      their RTK equivalents (delegated to `rtk rewrite`).
 *   2. Response compression   — compresses API/tool responses via
 *      `tokenless compress-response`.
 *
 * Stats recording is delegated to `tokenless stats record` CLI — no direct
 * SQL access, no injection risk.
 *
 * Context passing uses environment variables (TOKENLESS_AGENT_ID,
 * TOKENLESS_SESSION_ID, TOKENLESS_TOOL_USE_ID) which are inherited by
 * child processes and read by RTK's stats patch.
 */

import { execSync, execFileSync } from "child_process";

// ---- Binary availability cache ------------------------------------------------

let rtkAvailable: boolean | null = null;
let tokenlessAvailable: boolean | null = null;

function checkRtk(): boolean {
  if (rtkAvailable !== null) return rtkAvailable;
  try {
    execSync("which rtk", { stdio: "ignore" });
    rtkAvailable = true;
  } catch {
    rtkAvailable = false;
  }
  return rtkAvailable;
}

function checkTokenless(): boolean {
  if (tokenlessAvailable !== null) return tokenlessAvailable;
  try {
    execSync("which tokenless", { stdio: "ignore" });
    tokenlessAvailable = true;
  } catch {
    tokenlessAvailable = false;
  }
  return tokenlessAvailable;
}

// ---- Subprocess helpers -------------------------------------------------------

function tryRtkRewrite(command: string): string | null {
  try {
    const result = spawnSync("rtk", ["rewrite", command], {
      encoding: "utf-8",
      timeout: 2000,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const rewritten = result.stdout?.trim();
    if ((result.status === 0 || result.status === 3) && rewritten && rewritten !== command) {
      return rewritten;
    }
    return null;
  } catch {
    return null;
  }
}

function tryCompressResponse(response: any, sessionId?: string, toolCallId?: string): any | null {
  try {
    const input = JSON.stringify(response);
    const args = ["compress-response", "--agent-id", "openclaw"];
    if (sessionId) args.push("--session-id", sessionId);
    if (toolCallId) args.push("--tool-use-id", toolCallId);
    const result = execFileSync("tokenless", args, {
      encoding: "utf-8",
      timeout: 3000,
      input,
    }).trim();
    return JSON.parse(result);
  } catch {
    return null;
  }
}

function tryTrimHistory(messages: any[], sessionId?: string): any[] | null {
  try {
    const input = JSON.stringify(messages);
    const args = ["trim-history", "--agent-id", "openclaw"];
    if (sessionId) args.push("--session-id", sessionId);
    const result = execFileSync("tokenless", args, {
      encoding: "utf-8",
      timeout: 2000,
      input,
    }).trim();
    return JSON.parse(result);
  } catch {
    return null;
  }
}

function tryCompressHistory(messages: any[], sessionId?: string): string | null {
  try {
    const input = JSON.stringify(messages);
    const args = ["compress-history", "--agent-id", "openclaw"];
    if (sessionId) args.push("--session-id", sessionId);
    const result = execFileSync("tokenless", args, {
      encoding: "utf-8",
      timeout: 5000,
      input,
    }).trim();
    return result;
  } catch {
    return null;
  }
}

// ---- Plugin entry point -------------------------------------------------------

export default {
  id: "tokenless-openclaw",
  name: "Token-Less",
  version: "5.0.0",
  description: "Unified RTK command rewriting + response compression",
  register(api: any) {
  const pluginConfig = api.config ?? {};
  const rtkEnabled = pluginConfig.rtk_enabled !== false;
  const responseCompressionEnabled = pluginConfig.response_compression_enabled !== false;
  const historyTrimEnabled = pluginConfig.history_trim_enabled !== false;
  const deepCompressionEnabled = pluginConfig.deep_compression_enabled !== false;
  const verbose = pluginConfig.verbose !== false;

  // ---- 1. RTK command rewriting (before_tool_call) ----------------------------

  if (rtkEnabled && checkRtk()) {
    api.on(
      "before_tool_call",
      (event: { toolName: string; params: Record<string, unknown> }, ctx: { sessionId?: string; sessionKey?: string; agentId?: string; toolCallId?: string; runId?: string }) => {
        if (event.toolName !== "exec") return;

        const command = event.params?.command;
        if (typeof command !== "string") return;

        // Set env vars so RTK can read agent/session/tool IDs
        process.env.TOKENLESS_AGENT_ID = "openclaw";
        if (ctx?.sessionId) process.env.TOKENLESS_SESSION_ID = ctx.sessionId;
        if (ctx?.toolCallId) process.env.TOKENLESS_TOOL_USE_ID = ctx.toolCallId;

        const rewritten = tryRtkRewrite(command);
        if (!rewritten) return;

        if (verbose) {
          console.log(`[tokenless/rtk] rewrite: ${command} -> ${rewritten}`);
        }

        return { params: { ...event.params, command: rewritten } };
      },
      { priority: 10 },
    );
  }

  // ---- 2. Response compression (tool_result_persist) --------------------------

  if (responseCompressionEnabled && checkTokenless()) {
    api.on(
      "tool_result_persist",
      (event: { toolName?: string; toolCallId?: string; message: any; isSynthetic?: boolean }, ctx: { agentId?: string; sessionKey?: string; toolName?: string; toolCallId?: string }) => {
        const beforeJson = JSON.stringify(event.message);
        // Skip small responses
        if (beforeJson.length < 200) return;

        const toolCallId = ctx?.toolCallId || event.toolCallId;

        const compressed = tryCompressResponse(event.message, ctx?.sessionKey, toolCallId);
        if (!compressed) return;

        if (verbose) {
          const beforeChars = JSON.stringify(event.message).length;
          const afterChars = JSON.stringify(compressed).length;
          console.log(
            `[tokenless/response] ${event.toolName}: ${beforeChars} -> ${afterChars} chars (${Math.round((1 - afterChars / beforeChars) * 100)}% reduction)`,
          );
        }

        return { message: compressed };
      },
      { priority: 10 },
    );
  }

  // ---- 3. History trimming (before_model, priority 5) -----------------------

  if (historyTrimEnabled && checkTokenless()) {
    api.on(
      "before_model",
      (event: { llm_request?: { messages?: any[]; model?: string } }, ctx: { sessionId?: string; sessionKey?: string }) => {
        const messages = event.llm_request?.messages;
        if (!messages || messages.length < 4) return;

        const trimmed = tryTrimHistory(messages, ctx?.sessionKey);
        if (!trimmed) return;

        if (verbose) {
          console.log(`[tokenless/history] trim: ${messages.length} -> ${trimmed.length} messages`);
        }

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

  // ---- 4. Deep history compression (before_model, priority 20) --------------

  if (deepCompressionEnabled && checkTokenless()) {
    api.on(
      "before_model",
      (event: { llm_request?: { messages?: any[]; model?: string } }, ctx: { sessionId?: string; sessionKey?: string }) => {
        const messages = event.llm_request?.messages;
        if (!messages) return;

        // Estimate tokens: ~4 chars per token
        const totalChars = messages.reduce((sum: number, m: any) => sum + (typeof m.content === "string" ? m.content.length : 0), 0);
        const estimatedTokens = Math.ceil(totalChars / 4);
        const contextWindow = 128000;
        if (estimatedTokens < contextWindow * 0.7) return;

        const snapshot = tryCompressHistory(messages, ctx?.sessionKey);
        if (!snapshot) return;

        if (verbose) {
          console.log(`[tokenless/history] deep compress: ${estimatedTokens} estimated tokens -> <state_snapshot>`);
        }

        const preserveCount = Math.floor(messages.length * 0.3);
        const preservedMessages = messages.slice(-preserveCount);
        const snapshotMessage = { role: "system", content: snapshot };

        return {
          hookSpecificOutput: {
            hookEventName: "BeforeModel",
            llm_request: {
              messages: [snapshotMessage, ...preservedMessages],
            },
          },
        };
      },
      { priority: 20 },
    );
  }

  // ---- Done -------------------------------------------------------------------

  if (verbose) {
    const features = [
      rtkEnabled && rtkAvailable ? "rtk-rewrite" : null,
      responseCompressionEnabled && tokenlessAvailable ? "response-compression" : null,
      historyTrimEnabled && tokenlessAvailable ? "history-trim" : null,
      deepCompressionEnabled && tokenlessAvailable ? "deep-compression" : null,
    ].filter(Boolean);
    console.log(`[tokenless] OpenClaw plugin v5.1 registered — active features: ${features.join(", ") || "none"}`);
  }
  },
};