# aidememo-napi

Node.js bindings for [AideMemo (`aidememo`)](https://github.com/taeyun16/aidememo) —
a local knowledge-graph wiki indexed with BM25 + semantic vectors.

The package returns JSON strings from read methods; call `JSON.parse()` at the
boundary. This keeps the native ABI small while preserving the full Rust schema.

## Install

From a checkout:

```bash
cd crates/aidememo-napi
npm install
npm run build
npm test
```

After public npm release, the intended install path is:

```bash
npm install aidememo-napi
```

## Quick start

```js
const { AideMemoStore } = require('aidememo-napi');

const g = new AideMemoStore('./_meta/wiki.sqlite');

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

SQLite is the default local backend. To open redb stores, build the native
package with the Cargo `redb` feature and pass `backend: 'redb'` when opening
the store:

```bash
cd crates/aidememo-napi
npm run build -- --features redb
```

```js
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
const thread = JSON.parse(g.factList({ entity: pack.session_id, limit: 20 }));
```

For a multi-agent shared store, pass `sourceId` on writes and reads. The same
field flows through `search`, `query`, `factList`, `factAdd`, `factAddMany`,
and `workflowStart`.

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
| `new AideMemoStore(path, args?)` | store handle; `args.backend` may be `"sqlite"` when built with the Cargo `sqlite` feature |
| `search(query, args?)` | JSON string: `SearchResult[]` |
| `query(topic, args?)` | JSON string: `QueryResult` |
| `workflowStart(title, args?)` | JSON string: workflow context pack |
| `traverse(entity, args?)` | JSON string: graph traversal |
| `pathFind(from, to)` | JSON string: path or `null` |
| `entityAdd/get/list/delete`, `resolveEntity`, `entityDescribe` | entity operations |
| `factAdd`, `factAddMany`, `factGet/list`, `factSupersede`, `factDelete` | fact operations |
| `factPin`, `pinnedFacts` | always-loaded facts |
| `relationAdd/remove`, `relationsGet` | relation operations |
| `ingest(wikiRoot, incremental?)`, `lint()`, `stats()` | maintenance |
