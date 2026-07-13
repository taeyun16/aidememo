# aidememo-napi

[![npm version](https://img.shields.io/npm/v/aidememo-napi.svg)](https://www.npmjs.com/package/aidememo-napi)
[![Node.js](https://img.shields.io/node/v/aidememo-napi.svg)](https://www.npmjs.com/package/aidememo-napi)
[![license](https://img.shields.io/npm/l/aidememo-napi.svg)](https://github.com/taeyun16/aidememo)

Local-first agent memory and knowledge graph for Node.js, backed by the native
[AideMemo](https://aidememo.taeyun.me) Rust engine. Store structured facts and
relationships, then retrieve them with BM25, semantic search, and graph traversal.

The package returns JSON strings from read methods; call `JSON.parse()` at the
boundary. This keeps the native ABI small while preserving the full Rust schema.

## Highlights

- Local SQLite storage by default; no hosted database or external LLM required.
- BM25, semantic retrieval, graph traversal, validity windows, and scoped memory.
- Workflow/session APIs for coding agents, issue automation, and multi-agent tools.
- Prebuilt native binaries for macOS, Linux, and Windows.
- The same data model as the AideMemo CLI, MCP server, and other language bindings.

## Install

```bash
npm install aidememo-napi
```

The root package automatically selects the matching optional native package:

| Platform | Architecture | Native package |
|---|---|---|
| macOS | Apple Silicon (`arm64`) | `aidememo-napi-darwin-arm64` |
| macOS | Intel (`x64`) | `aidememo-napi-darwin-x64` |
| Linux glibc | `arm64` | `aidememo-napi-linux-arm64-gnu` |
| Linux glibc | `x64` | `aidememo-napi-linux-x64-gnu` |
| Windows | `x64` MSVC | `aidememo-napi-win32-x64-msvc` |

Install `aidememo-napi`, not a platform package directly. Alpine Linux/musl and
Windows arm64 are not included in the current prebuilt matrix.

## Quick start

```js
const { AideMemoStore, version } = require('aidememo-napi');

const g = new AideMemoStore('./_meta/wiki.sqlite');
console.log(`AideMemo ${version()}`);

const redis = g.entityAdd('Redis', {
  entityType: 'technology',
  tags: ['cache', 'infra'],
});

const factId = g.factAdd('Redis Sentinel provides high availability', {
  entityIds: [redis],
  factType: 'decision',
  sourceId: 'team-a',
});

const hits = JSON.parse(g.search('high availability', {
  limit: 5,
  sourceId: 'team-a',
  bm25Only: true,
}));

console.log(factId, hits.map((hit) => hit.content));
```

## Build from source

From a checkout:

```bash
cd crates/aidememo-napi
npm install
npm run build
npm test
```

SQLite is the default local backend. Omit `backend` or pass an empty string to
use the compiled default. Pass `backend: 'sqlite'` or `backend: 'libsqlite'` to
select SQLite explicitly. To open redb stores, build the native package with
the Cargo `redb` feature and pass `backend: 'redb'` when opening the store:

```bash
cd crates/aidememo-napi
npm run build -- --features redb
```

```js
const sqlite = new AideMemoStore('./_meta/wiki.sqlite', { backend: 'libsqlite' });
const g = new AideMemoStore('./_meta/wiki.redb', { backend: 'redb' });
```

## Workflow start

Use `workflowStart` when an automation trigger only gives the agent a sparse
issue or ticket. It creates a tracked session, stores the trigger as a
`question` fact, and returns scoped decisions, lessons, errors, and search
context in one call.

```js
const { AideMemoStore } = require('aidememo-napi');

const g = new AideMemoStore('./team.sqlite');

const redis = g.entityAdd('Redis', { entityType: 'technology' });
g.factAdd('Decision: Redis worker jobs must wrap DNS timeouts with retries', {
  entityIds: [redis],
  factType: 'decision',
  sourceId: 'team-a',
});
g.factAdd('Lesson: Redis timeout incidents were hard to debug without DNS metrics', {
  entityIds: [redis],
  factType: 'lesson',
  sourceId: 'team-a',
});

const pack = JSON.parse(g.workflowStart('Fix Redis timeout in worker', {
  body: 'Worker jobs intermittently time out. The issue has no more detail.',
  source: 'github:org/app#123',
  sourceId: 'team-a',
  limit: 8,
  depth: 2,
  recentLimit: 5,
  bm25Only: true, // keep cold-start deterministic in hooks/tests
}));

console.log(pack.session_id);
console.log(pack.ticket_fact_id);
console.log(pack.relevant_decisions.map((hit) => hit.content));

g.factAdd('Lesson: follow-up facts can attach to this workflow session', {
  entityIds: [redis],
  factType: 'lesson',
  sourceId: 'team-a',
  sessionId: pack.session_id,
});
const thread = JSON.parse(g.factList({
  entity: pack.session_id,
  sourceId: 'team-a',
  limit: 20,
}));
```

## Shared source namespaces

For a multi-agent shared store, pass the same `sourceId` on writes and reads.
It scopes `search`, `query`, `workflowStart`, `traverse`, `pathFind`,
`entityGet` / `entityList`, `factGet` / `factList`, `factPin` / `pinnedFacts`,
and `relationAdd` / `relationsGet`. `workflowStart` carries the namespace into
its ticket fact, optional parent-session relation, and all returned retrieval
context. Omitting `sourceId` preserves the legacy store-wide view.

```js
const worker = g.entityAdd('Worker', { entityType: 'service' });
g.factAdd('Worker uses Redis', { entityIds: [worker], sourceId: 'team-a' });
g.relationAdd('Redis', 'Worker', 'used_by', 'team-a');

const entity = JSON.parse(g.entityGet('Redis', 'team-a'));
const facts = JSON.parse(g.factList({ entity: 'Redis', sourceId: 'team-a' }));
const edges = JSON.parse(g.relationsGet('Redis', 'forward', 'team-a'));
const path = JSON.parse(g.pathFind('Redis', 'Worker', 'team-a'));
g.factPin(facts[0].id, true, 'team-a');
const alwaysOn = JSON.parse(g.pinnedFacts(10, 'team-a'));
```

Exact-content deduplication is local to a source namespace, so two sources can
store the same text as independent facts with distinct provenance.
Entities are a shared ontology: names, IDs, and types can be reused by several
sources. A scoped entity read only exposes entities backed by facts in that
source and omits globally-authored descriptive metadata; scoped relation reads
return only edges added with that exact `sourceId`. The native binding does not
authenticate callers or prevent them from choosing another `sourceId`, and
global mutation methods remain available. Treat this as a trusted-team
boundary. Use separate stores/processes for untrusted tenants, or expose the
store through the MCP server's token-to-source bindings described in
[`docs/MCP.md`](../../docs/MCP.md).

## Branch logs

Use `branchPush` / `branchMerge` when a Node agent or plugin forks a memory
store for speculative work and wants to merge only the winning branch.

```js
const candidate = new AideMemoStore('./candidate-b.sqlite', { backend: 'libsqlite' });
const pushed = JSON.parse(candidate.branchPush('candidate-b', './shared', {
  base: './shared/backup-01...',
}));

const main = new AideMemoStore('./main.sqlite', { backend: 'libsqlite' });
const merged = JSON.parse(main.branchMerge('./shared', {
  branch: 'candidate-b',
}));

console.log(pushed.records_exported, merged.facts_inserted);
```

Local branch paths use the already-open native store handle, so SDK/plugin code
does not reopen the same database file. S3 branch URIs should use the CLI
`aidememo branch ...` commands from a build compiled with `--features s3`.

## Errors

Native failures throw JavaScript `Error` objects. The N-API `error.code` is
`InvalidArg` for caller-actionable failures such as missing entities or invalid
input, and `GenericFailure` for store/search/internal failures. The message
starts with a stable aidememo code such as `[entity_not_found]`.

```js
try {
  g.entityGet('Rdis');
} catch (error) {
  if (error.message.includes('[entity_not_found]')) {
    console.log(error.code); // InvalidArg
  }
}
```

## API

| Method | Returns |
|---|---|
| `new AideMemoStore(path, args?)` | store handle; defaults to SQLite. `args.backend` may be `"sqlite"` or `"libsqlite"` in default builds, or `"redb"` when built with the Cargo `redb` feature |
| `search(query, { sourceId?, ... }?)` | JSON string: `SearchResult[]` |
| `query(topic, { sourceId?, ... }?)` | JSON string: `QueryResult` |
| `workflowStart(title, { sourceId?, actorId?, parentSessionId?, ... }?)` | JSON string: workflow context pack |
| `traverse(entity, { sourceId?, ... }?)` | JSON string: graph traversal |
| `pathFind(from, to, sourceId?)` | JSON string: path or `null` |
| `entityGet(name, sourceId?)`, `entityList({ sourceId?, ... }?)` | JSON string: scoped entity operations |
| `entityAdd/delete`, `resolveEntity`, `entityDescribe` | global shared-ontology operations |
| `factAdd`, `factAddMany`, `factGet(id, sourceId?)`, `factList({ sourceId?, ... }?)` | fact operations |
| `factPin(id, pinned, sourceId?)`, `pinnedFacts(limit?, sourceId?)` | always-loaded facts |
| `relationAdd(source, target, type, sourceId?)`, `relationsGet(entity, direction?, sourceId?)` | scoped relation operations |
| `relationRemove`, `factSupersede`, `factDelete` | global mutation operations |
| `ingest(wikiRoot, incremental?)`, `lint()`, `stats()` | maintenance |
| `branchPush(branch, destination, args?)`, `branchMerge(source, args?)` | JSON string branch-log reports |
