#!/usr/bin/env node
// FastAF hook bridge — managed by the FastAF desktop app.
// Only collect events when both FASTAF_TASK_ID and FASTAF_EVENT_DIR are set;
// in other cases (user manually starting claude/codex) exit immediately with zero side effects.

import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const taskId = process.env.FASTAF_TASK_ID;
const eventDir = process.env.FASTAF_EVENT_DIR;
if (!taskId || !eventDir) {
  process.exit(0);
}

// Different agents use different payload field names: Claude uses hook_event_name / session_id,
// Codex uses event_name / conversation_id; then fall back to the agent's own environment variables.
const pick = (payload, ...keys) => {
  for (const k of keys) {
    const v = payload[k];
    if (typeof v === "string" && v) return v;
  }
  return "";
};

let raw = "";
let done = false;

// Persist the collected stdin content to disk and exit. Idempotent: whichever of
// end / error / uncaughtException fires, this runs only once and always exits 0 — never let a hook failure affect the agent.
function finish() {
  if (done) return;
  done = true;
  try {
    const payload = raw ? JSON.parse(raw) : {};
    const line =
      JSON.stringify({
        ts: Date.now(),
        task_id: taskId,
        agent: process.env.FASTAF_AGENT || "",
        event: pick(payload, "hook_event_name", "event_name", "hookEventName", "event"),
        session_id:
          pick(payload, "session_id", "conversation_id", "sessionId", "conversationId") ||
          process.env.CODEX_SESSION_ID ||
          process.env.CLAUDE_CODE_SESSION_ID ||
          "",
        transcript_path: pick(payload, "transcript_path", "transcriptPath", "rollout_path"),
        cwd: pick(payload, "cwd"),
        tool_name: pick(payload, "tool_name", "toolName"),
        permission_mode: pick(payload, "permission_mode", "permissionMode"),
      }) + "\n";
    mkdirSync(eventDir, { recursive: true });
    appendFileSync(join(eventDir, "events.jsonl"), line);
  } catch {
    // Never let a hook failure block the agent
  }
  process.exit(0);
}

process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  raw += chunk;
});
process.stdin.on("end", finish);
// Critical Windows fix: after the agent finishes writing the payload and closes the stdin pipe,
// reading the pipe stdin to EOF throws an 'error' event `EOF: end of file, read` (on Unix it cleanly
// fires 'end' instead). An 'error' on the stream with no listener becomes an uncaught exception,
// making the process exit 1, and the agent then reports "hook exited with code 1". At this point
// 'data' has already collected the full payload, so just persist it through the normal flow.
process.stdin.on("error", finish);
// Safety net: no unexpected sync/async exception may cause the hook to exit non-zero.
process.on("uncaughtException", finish);
