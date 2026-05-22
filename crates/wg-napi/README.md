# wg-napi

Node.js bindings for [Wiki-Graph (`wg`)](https://github.com/taeyun16/wg) —
a local knowledge-graph wiki indexed with BM25 + semantic vectors.

The package returns JSON strings from read methods; call `JSON.parse()` at the
boundary. This keeps the native ABI small while preserving the full Rust schema.

## Install

From a checkout:

```bash
cd crates/wg-napi
npm install
npm run build
npm test
```

After public npm release, the intended install path is:

```bash
npm install wg-napi
```

## Quick start

```js
const { WgStore } = require('wg-napi');

const g = new WgStore('./_meta/wiki.redb');

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

## Workflow start

Use `workflowStart` when an automation trigger only gives the agent a sparse
issue or ticket. It creates a tracked session, stores the trigger as a
`question` fact, and returns scoped decisions, lessons, errors, and search
context in one call.

```js
const { WgStore } = require('wg-napi');

const g = new WgStore('./team.redb');

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
```

For a multi-agent shared store, pass `sourceId` on writes and reads. The same
field flows through `search`, `query`, `factList`, and `workflowStart`.

## API

| Method | Returns |
|---|---|
| `new WgStore(path)` | store handle |
| `search(query, args?)` | JSON string: `SearchResult[]` |
| `query(topic, args?)` | JSON string: `QueryResult` |
| `workflowStart(title, args?)` | JSON string: workflow context pack |
| `traverse(entity, args?)` | JSON string: graph traversal |
| `pathFind(from, to)` | JSON string: path or `null` |
| `entityAdd/get/list/delete`, `resolveEntity`, `entityDescribe` | entity operations |
| `factAdd`, `factAddMany`, `factGet/list`, `factSupersede`, `factDelete` | fact operations |
| `relationAdd/remove`, `relationsGet` | relation operations |
| `ingest(wikiRoot, incremental?)`, `lint()`, `stats()` | maintenance |
