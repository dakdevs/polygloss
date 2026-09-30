// Helpers for driving `polygloss-cli mcp` in bun tests: an SDK client over
// stdio (the 2025 `initialize` handshake Claude Code uses) and a raw
// line-oriented JSON-RPC session for byte-level checks.
import { Client } from "@modelcontextprotocol/client";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";
import { cliBin } from "./bins";

export const expectedTools = [
  "open_diff",
  "list_reviews",
  "list_threads",
  "get_thread",
  "reply",
  "resolve",
  "unresolve",
  "create_comment",
  "edit_comment",
  "delete_comment",
  "wait_for_review",
  "request_rereview",
  "focus",
];

/** Connects an SDK client to a fresh `polygloss-cli mcp` with `env`. */
export async function connectMcp(opts: {
  env: Record<string, string>;
  args?: string[];
  clientName?: string;
  clientVersion?: string;
  cwd?: string;
}): Promise<{ client: Client; stderr: () => string; close: () => Promise<void> }> {
  const transport = new StdioClientTransport({
    command: cliBin(),
    args: ["mcp", ...(opts.args ?? [])],
    env: opts.env,
    stderr: "pipe",
    cwd: opts.cwd,
  });
  let stderr = "";
  transport.stderr?.on("data", (chunk: Buffer) => {
    stderr += chunk.toString();
  });
  const client = new Client({
    name: opts.clientName ?? "polygloss-tests",
    version: opts.clientVersion ?? "0.0.0",
  });
  await client.connect(transport);
  return {
    client,
    stderr: () => stderr,
    close: async () => {
      await client.close();
    },
  };
}

/** The `initialize` request Claude Code sends (2025 handshake). */
export function initializeRequest(id: number, clientName = "raw-test"): string {
  return JSON.stringify({
    jsonrpc: "2.0",
    id,
    method: "initialize",
    params: {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: clientName, version: "1.0.0" },
    },
  });
}

/**
 * Spawns `polygloss-cli mcp`, writes `lines` (one JSON-RPC message each), waits
 * for `waitFor` responses (messages with an `id`), then closes stdin and
 * returns everything the server printed once it exits.
 */
export async function rawMcpSession(opts: {
  env: Record<string, string>;
  lines: string[];
  waitFor: number;
  timeoutMs?: number;
}): Promise<{ stdout: string; stderr: string; exitCode: number }> {
  const proc = Bun.spawn([cliBin(), "mcp"], {
    env: opts.env,
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe",
  });
  for (const line of opts.lines) proc.stdin.write(`${line}\n`);
  await proc.stdin.flush();

  const decoder = new TextDecoder();
  let stdout = "";
  const reader = proc.stdout.getReader();
  const deadline = Date.now() + (opts.timeoutMs ?? 10_000);
  const responses = () =>
    stdout
      .split("\n")
      .filter((l) => l.trim() !== "")
      .filter((l) => {
        try {
          return "id" in (JSON.parse(l) as object);
        } catch {
          return false;
        }
      }).length;
  while (responses() < opts.waitFor && Date.now() < deadline) {
    const chunk = await Promise.race([
      reader.read(),
      Bun.sleep(Math.max(0, deadline - Date.now())).then(() => null),
    ]);
    if (chunk === null || chunk.done) break;
    stdout += decoder.decode(chunk.value, { stream: true });
  }
  proc.stdin.end();
  for (;;) {
    const chunk = await reader.read();
    if (chunk.done) break;
    stdout += decoder.decode(chunk.value, { stream: true });
  }
  const exitCode = await proc.exited;
  const stderr = await new Response(proc.stderr).text();
  return { stdout, stderr, exitCode };
}
