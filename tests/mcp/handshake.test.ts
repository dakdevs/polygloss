// `polygloss mcp` handshake (T4.4, design §15.1): server info and instructions,
// the thirteen tools, `anthropic/alwaysLoad`, a clean stdout, no app launch at
// startup, session bookkeeping, and (with POLYGLOSS_PERF=1) cold-start latency.
import { Database } from "bun:sqlite";
import { afterAll, describe, expect, test } from "bun:test";
import { chmodSync, existsSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { builtVersion } from "../support/bins";
import { makeSandbox } from "../support/sandbox";
import {
  connectMcp,
  expectedTools,
  initializeRequest,
  rawMcpSession,
  timeToInitialize,
} from "../support/mcp";

const repoRoot = resolve(import.meta.dir, "../..");
const sandboxes: { cleanup: () => void }[] = [];
afterAll(() => {
  for (const s of sandboxes) s.cleanup();
});

/** A sandbox whose app launch override writes a marker file when run. */
function mcpSandbox(extra: Record<string, string> = {}): {
  env: Record<string, string>;
  dataDir: string;
  launchMarker: string;
} {
  const sandbox = makeSandbox();
  sandboxes.push(sandbox);
  const launchMarker = join(sandbox.home, "app-launched");
  const fakeApp = join(sandbox.home, "fake-app.sh");
  writeFileSync(fakeApp, `#!/bin/sh\necho "$@" > '${launchMarker}'\n`);
  chmodSync(fakeApp, 0o755);
  const env: Record<string, string> = {
    ...sandbox.env,
    POLYGLOSS_TEST: "1",
    POLYGLOSS_APP_BIN: fakeApp,
    ...extra,
  };
  return { env, dataDir: sandbox.dataDir, launchMarker };
}

async function designInstructions(): Promise<string> {
  const design = await Bun.file(join(repoRoot, "docs/design.md")).text();
  const section = design.slice(design.indexOf("### 15.4 Server instructions"));
  const start = section.indexOf("````text\n") + "````text\n".length;
  return section.slice(start, section.indexOf("\n````", start));
}

describe("polygloss mcp handshake", () => {
  test("initialize returns polygloss server info and instructions under 2048 chars", async () => {
    const { env } = mcpSandbox();
    const mcp = await connectMcp({ env });
    try {
      expect(mcp.client.getServerVersion()).toEqual({
        name: "polygloss",
        version: builtVersion(),
      });
      const instructions = mcp.client.getInstructions() ?? "";
      expect(instructions.length).toBeGreaterThan(0);
      expect(instructions.length).toBeLessThan(2048);
      expect(instructions).toBe(await designInstructions());
      expect(mcp.client.getServerCapabilities()?.tools).toBeDefined();
    } finally {
      await mcp.close();
    }
  });

  test("listTools returns exactly the thirteen tools with input schemas", async () => {
    const { env } = mcpSandbox();
    const mcp = await connectMcp({ env });
    try {
      const { tools } = await mcp.client.listTools();
      expect(tools.map((t) => t.name).sort()).toEqual(
        [...expectedTools].sort(),
      );
      for (const tool of tools) {
        expect(tool.description?.length ?? 0).toBeGreaterThan(20);
        expect(tool.inputSchema.type).toBe("object");
      }
      const byName = new Map(tools.map((t) => [t.name, t]));
      const required = (name: string) =>
        (byName.get(name)?.inputSchema.required ?? []).slice().sort();
      expect(required("get_thread")).toEqual(["thread_id"]);
      expect(required("reply")).toEqual(["body_md", "thread_id"]);
      expect(required("create_comment")).toEqual(["body_md", "kind"]);
      expect(required("wait_for_review")).toEqual(["review_id"]);
      expect(required("request_rereview")).toEqual(["review_id", "summary_md"]);
      expect(required("open_diff")).toEqual([]);
      const openProps = Object.keys(
        byName.get("open_diff")?.inputSchema.properties ?? {},
      ).sort();
      expect(openProps).toEqual(["assign", "label", "repo", "show", "source"]);
    } finally {
      await mcp.close();
    }
  });

  test("no tool, schema or resource description cites the design", async () => {
    // Agents never see internal references such as "§15.2" (T5.9 #15).
    const { env } = mcpSandbox();
    const mcp = await connectMcp({ env });
    try {
      const { tools } = await mcp.client.listTools();
      for (const tool of tools) {
        const text = JSON.stringify({
          description: tool.description,
          inputSchema: tool.inputSchema,
        });
        expect({ tool: tool.name, cites: text.includes("§") }).toEqual({
          tool: tool.name,
          cites: false,
        });
      }
      const { resourceTemplates } = await mcp.client.listResourceTemplates();
      expect(JSON.stringify(resourceTemplates)).not.toContain("§");
      expect(mcp.client.getInstructions() ?? "").not.toContain("§");
    } finally {
      await mcp.close();
    }
  });

  test("tools that name their target never wait for roots/list", async () => {
    // A client that declares roots but is slow to answer roots/list must not
    // hold up get_thread, reply, resolve, … (T5.9 #16): only open_diff needs
    // the repo default. This raw client never answers roots/list at all.
    const { env } = mcpSandbox();
    const init = JSON.parse(initializeRequest(1));
    init.params.capabilities = { roots: { listChanged: true } };
    const started = performance.now();
    const r = await rawMcpSession({
      env,
      lines: [
        JSON.stringify(init),
        JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
        JSON.stringify({
          jsonrpc: "2.0",
          id: 2,
          method: "tools/call",
          params: { name: "get_thread", arguments: { thread_id: "nope" } },
        }),
        JSON.stringify({
          jsonrpc: "2.0",
          id: 3,
          method: "tools/call",
          params: {
            name: "list_threads",
            arguments: { review_id: "nope" },
          },
        }),
      ],
      waitFor: 3,
    });
    const ms = performance.now() - started;
    const messages = r.stdout
      .split("\n")
      .filter((l) => l.trim() !== "")
      .map((l) => JSON.parse(l) as Record<string, any>);
    expect(messages.some((m) => m.method === "roots/list")).toBe(false);
    const answers = messages.filter((m) => m.id === 2 || m.id === 3);
    expect(answers).toHaveLength(2);
    for (const a of answers) {
      expect(a.result.isError).toBe(true);
      expect(a.result.structuredContent.code).toBe("not_found");
    }
    // Well under the 2 s roots/list timeout.
    expect(ms).toBeLessThan(1_900);
  });

  test("alwaysLoad meta on open_diff list_threads wait_for_review", async () => {
    const { env } = mcpSandbox();
    const mcp = await connectMcp({ env });
    try {
      const { tools } = await mcp.client.listTools();
      const alwaysLoaded = tools
        .filter((t) => t._meta?.["anthropic/alwaysLoad"] === true)
        .map((t) => t.name)
        .sort();
      expect(alwaysLoaded).toEqual([
        "list_threads",
        "open_diff",
        "wait_for_review",
      ]);
    } finally {
      await mcp.close();
    }
  });

  test("stdout carries only JSON-RPC lines", async () => {
    const { env } = mcpSandbox({ RUST_LOG: "trace" });
    const r = await rawMcpSession({
      env,
      lines: [
        initializeRequest(1),
        JSON.stringify({
          jsonrpc: "2.0",
          method: "notifications/initialized",
        }),
        JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list" }),
        JSON.stringify({
          jsonrpc: "2.0",
          id: 3,
          method: "tools/call",
          params: { name: "get_thread", arguments: { thread_id: "nope" } },
        }),
      ],
      waitFor: 3,
    });
    expect(r.exitCode).toBe(0);
    const lines = r.stdout.split("\n").filter((l) => l !== "");
    expect(lines.length).toBe(3);
    for (const line of lines) {
      const msg = JSON.parse(line) as { jsonrpc: string };
      expect(msg.jsonrpc).toBe("2.0");
    }
    const ids = lines.map((l) => (JSON.parse(l) as { id: number }).id);
    expect(ids).toEqual([1, 2, 3]);
    // The call result is a tool result (isError with {code, message}), not a
    // protocol error.
    const call = JSON.parse(lines[2] ?? "{}") as {
      result: {
        isError: boolean;
        structuredContent: { code: string; message: string };
      };
    };
    expect(call.result.isError).toBe(true);
    expect(typeof call.result.structuredContent.code).toBe("string");
    expect(typeof call.result.structuredContent.message).toBe("string");
    // Trace logging went to stderr, without ANSI colors.
    expect(r.stderr.length).toBeGreaterThan(0);
    expect(r.stderr).not.toContain("\u001b[");
  });

  test("startup never launches the app", async () => {
    const { env, launchMarker } = mcpSandbox();
    const mcp = await connectMcp({ env });
    try {
      await mcp.client.listTools();
      await Bun.sleep(200);
      expect(existsSync(launchMarker)).toBe(false);
    } finally {
      await mcp.close();
    }
    expect(existsSync(launchMarker)).toBe(false);
  });

  test("session id and client name recorded", async () => {
    const { env, dataDir } = mcpSandbox({
      CLAUDE_CODE_SESSION_ID: "5b2e1f0c-session-under-test",
    });
    const mcp = await connectMcp({
      env,
      clientName: "claude-code",
      clientVersion: "9.8.7",
    });
    try {
      const db = new Database(join(dataDir, "polygloss.db"), {
        readwrite: true,
      });
      try {
        const rows = db
          .query(
            "SELECT id, client_name, client_version, owner_pid, cwd FROM sessions",
          )
          .all() as {
          id: string;
          client_name: string;
          client_version: string | null;
          owner_pid: number | null;
          cwd: string | null;
        }[];
        expect(rows.length).toBe(1);
        expect(rows[0]).toMatchObject({
          id: "5b2e1f0c-session-under-test",
          client_name: "claude-code",
          client_version: "9.8.7",
          // The agent host that spawned the server: this test process.
          owner_pid: process.pid,
        });
      } finally {
        db.close();
      }
    } finally {
      await mcp.close();
    }

    // Without CLAUDE_CODE_SESSION_ID the server makes up `pg-<uuidv7>`.
    const other = mcpSandbox();
    const mcp2 = await connectMcp({ env: other.env, clientName: "codex" });
    try {
      const db = new Database(join(other.dataDir, "polygloss.db"), {
        readwrite: true,
      });
      try {
        const rows = db.query("SELECT id, client_name FROM sessions").all() as {
          id: string;
          client_name: string;
        }[];
        expect(rows.length).toBe(1);
        expect(rows[0]?.client_name).toBe("codex");
        expect(rows[0]?.id).toMatch(
          /^pg-[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
        );
      } finally {
        db.close();
      }
    } finally {
      await mcp2.close();
    }
  });

  test.skipIf(process.env.POLYGLOSS_PERF !== "1")(
    "initialize-ready median under 100 ms",
    async () => {
      const { env } = mcpSandbox();
      const samples: number[] = [];
      for (let i = 0; i < 10; i++) samples.push(await timeToInitialize(env));
      samples.sort((a, b) => a - b);
      const median = ((samples[4] ?? 0) + (samples[5] ?? 0)) / 2;
      console.log(
        `polygloss mcp initialize-ready: median ${median.toFixed(1)} ms, samples ${samples.map((s) => s.toFixed(1)).join(", ")}`,
      );
      expect(median).toBeLessThan(100);
    },
    60_000,
  );
});
