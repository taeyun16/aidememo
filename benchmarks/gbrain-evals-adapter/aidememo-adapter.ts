// Adapter for garrytan/gbrain-evals.
//
// Copy this file to:
//   gbrain-evals/eval/runner/adapters/aidememo.ts
//
// Then register `new AideMemoAdapter()` in eval/runner/multi-adapter.ts.

import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { Adapter, AdapterConfig, BrainState, Page, Query, RankedDoc } from "../types.ts";

declare const require: ((id: string) => any) | undefined;

type AideMemoBackend = "cli" | "napi";

type AideMemoNativeStore = {
  ingest(wikiRoot: string, incremental?: boolean): string;
  search(query: string, args?: {
    limit?: number;
    currentOnly?: boolean;
    bm25Only?: boolean;
    asOf?: number;
  }): string;
};

type AideMemoState = {
  root: string;
  wikiRoot: string;
  store: string;
  wgBin: string;
  backend: AideMemoBackend;
  mode: "bm25" | "hybrid";
  sourceToSlug: Map<string, string>;
  nativeStore?: AideMemoNativeStore;
  daemonUrl?: string;
  daemonProc?: ReturnType<typeof Bun.spawn>;
};

type AideMemoSearchRow = {
  content?: string;
  source?: string;
  score?: number;
  rank?: number;
  fact_id?: string;
  entity_names?: string[];
};

function run(args: string[], opts: { cwd?: string } = {}): string {
  const proc = Bun.spawnSync(args, {
    cwd: opts.cwd,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (proc.exitCode !== 0) {
    const stdout = new TextDecoder().decode(proc.stdout);
    const stderr = new TextDecoder().decode(proc.stderr);
    throw new Error(`${args.join(" ")} failed (${proc.exitCode})\nstdout:\n${stdout}\nstderr:\n${stderr}`);
  }
  return new TextDecoder().decode(proc.stdout);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function pickPort(): Promise<number> {
  if (process.env.AIDEMEMO_ADAPTER_DAEMON_PORT) {
    return Promise.resolve(Number(process.env.AIDEMEMO_ADAPTER_DAEMON_PORT));
  }
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      if (!addr || typeof addr === "string") {
        server.close(() => reject(new Error("failed to allocate daemon port")));
        return;
      }
      const port = addr.port;
      server.close(() => resolve(port));
    });
  });
}

async function startDaemon(wgBin: string, store: string): Promise<{
  url: string;
  proc: ReturnType<typeof Bun.spawn>;
}> {
  const port = await pickPort();
  const url = `http://127.0.0.1:${port}`;
  const proc = Bun.spawn([wgBin, "mcp-serve", "--port", String(port), store], {
    stdout: "pipe",
    stderr: "pipe",
  });

  const deadline = Date.now() + Number(process.env.AIDEMEMO_ADAPTER_DAEMON_TIMEOUT_MS ?? "10000");
  while (Date.now() < deadline) {
    if ((proc as any).exitCode !== null) {
      throw new Error(`aidememo mcp-serve exited before /health became ready (code ${(proc as any).exitCode})`);
    }
    try {
      const res = await fetch(`${url}/health`);
      if (res.ok) return { url, proc };
    } catch {
      // Server is not listening yet.
    }
    await sleep(100);
  }
  proc.kill();
  throw new Error(`aidememo mcp-serve did not answer ${url}/health before timeout`);
}

function pageMarkdown(page: Page): string {
  const type = String(page.type ?? "note");
  const title = page.title || page.slug;
  return [
    "---",
    `type: ${type}`,
    "---",
    `# ${title}`,
    "",
    "## Compiled Truth",
    page.compiled_truth || "",
    "",
    "## Timeline",
    page.timeline || "",
    "",
  ].join("\n");
}

function writePages(root: string, pages: Page[]): Map<string, string> {
  const sourceToSlug = new Map<string, string>();
  for (const page of pages) {
    const rel = `${page.slug}.md`;
    const file = join(root, rel);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, pageMarkdown(page));
    sourceToSlug.set(rel, page.slug);
  }
  return sourceToSlug;
}

function pageIdForSource(source: string | undefined, state: AideMemoState): string | undefined {
  if (!source) return undefined;
  const file = source.split("#", 1)[0] ?? source;
  return state.sourceToSlug.get(file);
}

function parseRows(raw: string): AideMemoSearchRow[] {
  const parsed = JSON.parse(raw);
  if (Array.isArray(parsed)) return parsed;
  if (Array.isArray(parsed.results)) return parsed.results;
  return [];
}

function loadNativeStore(store: string): AideMemoNativeStore | undefined {
  if (typeof require !== "function") return undefined;
  const moduleName = process.env.AIDEMEMO_NAPI_MODULE ?? "aidememo-napi";
  try {
    const mod = require(moduleName);
    if (typeof mod?.AideMemoStore !== "function") return undefined;
    return new mod.AideMemoStore(store) as AideMemoNativeStore;
  } catch {
    return undefined;
  }
}

function requestedBackend(): "auto" | AideMemoBackend {
  const raw = process.env.AIDEMEMO_ADAPTER_BACKEND ?? "auto";
  if (raw === "napi" || raw === "cli" || raw === "auto") return raw;
  throw new Error(`unsupported AIDEMEMO_ADAPTER_BACKEND=${raw}; expected auto, cli, or napi`);
}

function asOfMs(value: string | undefined): number | undefined {
  if (!value || value === "corpus-end" || value === "per-source") return undefined;
  const ms = Date.parse(value);
  if (Number.isNaN(ms)) throw new Error(`invalid as_of_date: ${value}`);
  return ms;
}

export class AideMemoAdapter implements Adapter {
  readonly name = "aidememo";

  async init(rawPages: Page[], _config: AdapterConfig): Promise<BrainState> {
    const root = mkdtempSync(join(tmpdir(), "aidememo-gbrain-evals-"));
    const wikiRoot = join(root, "wiki");
    const store = join(root, "wiki.redb");
    mkdirSync(wikiRoot, { recursive: true });
    const sourceToSlug = writePages(wikiRoot, rawPages);

    const wgBin = process.env.AIDEMEMO_BIN ?? "aidememo";
    const backendRequest = requestedBackend();
    const nativeStore = backendRequest === "cli" ? undefined : loadNativeStore(store);
    if (backendRequest === "napi" && !nativeStore) {
      throw new Error(
        "AIDEMEMO_ADAPTER_BACKEND=napi requested but aidememo-napi could not be loaded. Set AIDEMEMO_NAPI_MODULE to the built package path.",
      );
    }

    if (nativeStore) {
      nativeStore.ingest(wikiRoot, false);
    } else {
      run([wgBin, "--store", store, "ingest", wikiRoot]);
    }

    const state: AideMemoState = {
      root,
      wikiRoot,
      store,
      wgBin,
      backend: nativeStore ? "napi" : "cli",
      mode: process.env.AIDEMEMO_ADAPTER_MODE === "bm25" ? "bm25" : "hybrid",
      sourceToSlug,
      nativeStore,
    };

    if (!nativeStore && process.env.AIDEMEMO_ADAPTER_DAEMON === "1") {
      const daemon = await startDaemon(wgBin, store);
      state.daemonUrl = daemon.url;
      state.daemonProc = daemon.proc;
    }

    return state;
  }

  async query(q: Query, state: BrainState): Promise<RankedDoc[]> {
    const s = state as AideMemoState;
    const limit = Number(process.env.AIDEMEMO_ADAPTER_LIMIT ?? "10");
    let rows: AideMemoSearchRow[];

    if (s.nativeStore) {
      rows = parseRows(
        s.nativeStore.search(q.text, {
          limit,
          currentOnly: true,
          bm25Only: s.mode === "bm25",
          asOf: asOfMs(q.as_of_date),
        }),
      );
    } else {
      const args = [s.wgBin, "--store", s.store, "--json", "search", q.text, "-l", String(limit)];
      if (q.as_of_date && q.as_of_date !== "corpus-end" && q.as_of_date !== "per-source") {
        args.push("--as-of", q.as_of_date);
      }
      if (s.mode === "hybrid") {
        args.push("--hybrid");
      }
      if (s.daemonUrl) {
        args.push("--via", s.daemonUrl);
      }
      rows = parseRows(run(args));
    }

    const out: RankedDoc[] = [];
    const seen = new Set<string>();
    for (const row of rows) {
      const page_id = pageIdForSource(row.source, s);
      if (!page_id || seen.has(page_id)) continue;
      seen.add(page_id);
      out.push({
        page_id,
        score: Number(row.score ?? 0),
        rank: out.length + 1,
        snippet: row.content,
      });
    }
    return out;
  }

  async snapshot(state: BrainState): Promise<string> {
    return (state as AideMemoState).root;
  }

  async teardown(state: BrainState): Promise<void> {
    const s = state as AideMemoState;
    if (s.daemonProc) {
      s.daemonProc.kill();
      try {
        await s.daemonProc.exited;
      } catch {
        // Ignore teardown races; the benchmark result is already complete.
      }
    }
    if (!process.env.AIDEMEMO_ADAPTER_KEEP) {
      rmSync(s.root, { recursive: true, force: true });
    }
  }
}

export function createAideMemo(): AideMemoAdapter {
  return new AideMemoAdapter();
}

export default AideMemoAdapter;
