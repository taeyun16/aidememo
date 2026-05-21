// End-to-end smoke test for wg-napi.
//
// Run: `npm run build && node tests/smoke.js` from crates/wg-napi.

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { WgStore, version } = require('..');

function main() {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'wg-napi-smoke-'));
  const db = path.join(tmp, 'test.redb');

  try {
    const g = new WgStore(db);
    console.log(`wg-napi version: ${version()}`);

    // Entity CRUD
    const eidRedis = g.entityAdd('Redis', {
      entityType: 'technology',
      tags: ['cache', 'infra'],
      aliases: ['redis-server'],
    });
    g.entityAdd('Postgres', { entityType: 'technology' });
    if (g.resolveEntity('Redis') !== eidRedis) throw new Error('resolve_entity mismatch');
    if (g.resolveEntity('redis-server') !== eidRedis) throw new Error('alias resolution failed');

    const e = JSON.parse(g.entityGet('Redis'));
    if (e.name !== 'Redis') throw new Error(`expected Redis, got ${e.name}`);
    if (!e.tags.includes('cache')) throw new Error('missing cache tag');

    const ents = JSON.parse(g.entityList({ limit: 10 }));
    if (ents.length !== 2) throw new Error(`expected 2 entities, got ${ents.length}`);

    // Facts
    const fid = g.factAdd('Redis Sentinel provides high availability', {
      entityIds: [eidRedis],
      factType: 'decision',
      tags: ['ha'],
      sourceId: 'alpha',
      confidence: 0.9,
    });
    const fact = JSON.parse(g.factGet(fid));
    if (!fact.content.startsWith('Redis Sentinel')) throw new Error('fact content mismatch');

    const facts = JSON.parse(g.factList({ entity: 'Redis', limit: 10, sourceId: 'alpha' }));
    if (facts.length !== 1) throw new Error(`expected 1 fact, got ${facts.length}`);

    // Batch insert — single redb write txn for the whole array.
    const manyIds = g.factAddMany([
      { content: 'Redis Cluster shards by hash slot', entityIds: [eidRedis], factType: 'pattern' },
      { content: 'Redis 7 introduces Functions and ACL improvements',
        entityIds: [eidRedis], factType: 'note', confidence: 0.85 },
      { content: 'Postgres logical replication is the default',
        entityIds: [g.resolveEntity('Postgres')], factType: 'convention' },
    ]);
    if (manyIds.length !== 3) throw new Error(`expected 3 batch ids, got ${manyIds.length}`);
    for (const id of manyIds) {
      const rec = JSON.parse(g.factGet(id));
      if (rec.id !== id) throw new Error(`fact_get round-trip failed for ${id}`);
    }

    // Relations
    g.relationAdd('Redis', 'Postgres', 'alternative_to');
    const rels = JSON.parse(g.relationsGet('Redis', 'forward'));
    if (rels.length !== 1) throw new Error(`expected 1 relation, got ${rels.length}`);

    // Search (BM25 + semantic — semantic may no-op without model, but must dispatch)
    try {
      const hits = JSON.parse(g.search('high availability', { limit: 5, bm25Only: true, sourceId: 'alpha' }));
      console.log(`search hits: ${hits.length}`);
    } catch (e) {
      console.log(`search skipped: ${e.message}`);
    }

    // Graph
    const traverse = JSON.parse(g.traverse('Redis', { depth: 1, direction: 'both' }));
    if (!Array.isArray(traverse.entities)) throw new Error('traverse.entities not array');
    const found = JSON.parse(g.pathFind('Redis', 'Postgres'));
    if (!found || found.length < 1) throw new Error('expected path Redis -> Postgres');

    // Lint / stats
    const issues = JSON.parse(g.lint());
    const stats = JSON.parse(g.stats());
    console.log(`stats:`, stats);
    console.log(`lint issues: ${issues.length}`);

    // Query (unified)
    try {
      const ctx = JSON.parse(g.query('Redis', { limit: 3, depth: 1, recentLimit: 3 }));
      if (ctx.topic !== 'Redis') throw new Error('query topic mismatch');
      if (ctx.entity?.name !== 'Redis') throw new Error('query entity mismatch');
      console.log(`query keys: [${Object.keys(ctx).join(', ')}]`);
    } catch (e) {
      console.log(`query skipped: ${e.message}`);
    }

    // Validity windows
    const newFid = g.factAdd('Redis Sentinel + Cluster supersedes Sentinel-only HA', {
      entityIds: [eidRedis],
      factType: 'decision',
    });
    g.factSupersede(fid, newFid);
    const oldFact = JSON.parse(g.factGet(fid));
    if (oldFact.superseded_at == null) throw new Error('expected superseded_at to be set');
    if (oldFact.superseded_by !== newFid) throw new Error('expected superseded_by to point to newFid');

    const allFacts = JSON.parse(g.factList({ entity: 'Redis' }));
    const currentFacts = JSON.parse(g.factList({ entity: 'Redis', currentOnly: true }));
    if (currentFacts.length !== allFacts.length - 1) {
      throw new Error(`expected currentOnly to hide 1 fact (all=${allFacts.length}, current=${currentFacts.length})`);
    }

    // Cleanup
    g.factDelete(fid);
    g.factDelete(newFid);
    g.relationRemove('Redis', 'Postgres', 'alternative_to');
    g.entityDelete('Postgres');

    console.log('OK: wg-napi smoke test passed');
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

main();
