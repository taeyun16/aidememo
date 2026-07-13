/* aidememo.h — C-ABI for AideMemo (aidememo).
 *
 * All read functions return a heap-allocated, NUL-terminated UTF-8 JSON
 * string. The caller MUST free it with aidememo_free_string. On error the JSON
 * payload is `{"error": "..."}`.
 *
 * Build: link against libaidememo_ffi (cdylib or staticlib).
 *
 *   cc example.c -L /path/to/target/debug -laidememo_ffi -o example
 *
 * Thread safety: a single aidememo_store_t* is safe to share across threads.
 *
 * Source namespaces: legacy functions expose the store-wide view. Functions
 * suffixed `_scoped` accept a caller-selected source_id and constrain facts,
 * source-owned relations, and fact-backed entity visibility to that exact
 * namespace. Entity identity and type remain a shared ontology. These calls
 * do not authenticate the caller; use them only between trusted components or
 * place the MCP server's token-to-source binding in front of the store.
 */

#ifndef AIDEMEMO_H
#define AIDEMEMO_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque handle. */
typedef struct AideMemoStore aidememo_store_t;

/* Lifecycle. */
aidememo_store_t* aidememo_open(const char* path);
/* Open with an explicit backend: "sqlite" or "libsqlite" in default builds,
 * or "redb" when the library was built with the `redb` Cargo feature. Pass
 * NULL or "" for the compiled default backend. */
aidememo_store_t* aidememo_open_with_backend(const char* path, const char* backend);
void        aidememo_close(aidememo_store_t* store);
void        aidememo_free_string(char* s);
char*       aidememo_version(void);

/* Search & query. `current_only` excludes superseded facts. Pass a non-empty
 * source_id to scoped variants for a source-local view. */
char* aidememo_search(const aidememo_store_t* store,
                const char* query,
                uint32_t limit,
                bool        current_only);
char* aidememo_search_scoped(const aidememo_store_t* store,
                const char* query,
                uint32_t limit,
                bool current_only,
                const char* source_id);
char* aidememo_query(const aidememo_store_t* store,
               const char* topic,
               uint32_t limit,
               uint32_t depth,
               uint32_t recent_limit,
               bool     current_only,
               const char* mode);  /* "naive"|"local"|"hybrid"|"global" or NULL */
char* aidememo_query_scoped(const aidememo_store_t* store,
               const char* topic,
               uint32_t limit,
               uint32_t depth,
               uint32_t recent_limit,
               bool current_only,
               const char* mode,
               const char* source_id);

/* Graph. Scoped variants traverse only relations owned by source_id and
 * entities backed by facts in that source. */
char* aidememo_traverse(const aidememo_store_t* store,
                  const char* entity,
                  uint32_t depth,
                  const char* direction /* "forward" | "reverse" | "both" */);
char* aidememo_path_find(const aidememo_store_t* store, const char* from, const char* to);
char* aidememo_traverse_scoped(const aidememo_store_t* store,
                  const char* entity,
                  uint32_t depth,
                  const char* direction,
                  const char* source_id);
char* aidememo_path_find_scoped(const aidememo_store_t* store,
                  const char* from,
                  const char* to,
                  const char* source_id);

/* Entity CRUD. Scoped reads require fact-backed visibility in source_id and
 * omit globally-authored aliases, tags, source page, summary, and timestamps.
 * tags_json / aliases_json: JSON arrays of strings, e.g. "[\"a\",\"b\"]".
 *                           Pass NULL or "" to omit.
 */
char* aidememo_entity_add(const aidememo_store_t* store,
                    const char* name,
                    const char* entity_type,    /* may be NULL */
                    const char* tags_json,      /* may be NULL */
                    const char* aliases_json,   /* may be NULL */
                    const char* source_page);   /* may be NULL */
char* aidememo_entity_get(const aidememo_store_t* store, const char* name);
char* aidememo_entity_get_scoped(const aidememo_store_t* store,
                     const char* name,
                     const char* source_id);
char* aidememo_entity_list(const aidememo_store_t* store,
                     uint32_t limit,            /* 0 = no limit */
                     const char* entity_type);  /* may be NULL */
char* aidememo_entity_list_scoped(const aidememo_store_t* store,
                     uint32_t limit,
                     const char* entity_type,
                     const char* source_id);
char* aidememo_entity_delete(const aidememo_store_t* store, const char* name);
/* Set the compiled-truth summary; pass "" or NULL to clear. */
char* aidememo_entity_describe(const aidememo_store_t* store,
                         const char* name,
                         const char* summary);
char* aidememo_resolve_entity(const aidememo_store_t* store, const char* name);

/* Fact CRUD. Use the same source_id on scoped writes and reads. Exact-content
 * deduplication is independent per source namespace. */
char* aidememo_fact_add(const aidememo_store_t* store,
                  const char* content,
                  const char* entity_ids_json,  /* JSON array of ULIDs, may be NULL */
                  const char* fact_type,        /* may be NULL */
                  const char* tags_json,        /* may be NULL */
                  const char* source,           /* may be NULL */
                  float       confidence);      /* 0.0 = unset */
char* aidememo_fact_add_scoped(const aidememo_store_t* store,
                  const char* content,
                  const char* entity_ids_json,
                  const char* fact_type,
                  const char* tags_json,
                  const char* source,
                  float confidence,
                  const char* source_id,
                  const char* actor_id);
/* Insert many facts in one backend transaction when supported. items_json is a
 * JSON array of objects matching aidememo_fact_add's args:
 *   content (required), entity_ids, fact_type, tags, source, source_id,
 *   actor_id, confidence
 * Returns {"ids":[...]} on success. */
char* aidememo_fact_add_many(const aidememo_store_t* store, const char* items_json);
char* aidememo_fact_get(const aidememo_store_t* store, const char* fact_id);
char* aidememo_fact_get_scoped(const aidememo_store_t* store,
                  const char* fact_id,
                  const char* source_id);
char* aidememo_pinned_facts(const aidememo_store_t* store, uint32_t limit);
char* aidememo_pinned_facts_scoped(const aidememo_store_t* store,
                  uint32_t limit,
                  const char* source_id);
char* aidememo_fact_pin(const aidememo_store_t* store,
                  const char* fact_id,
                  bool pinned);
char* aidememo_fact_pin_scoped(const aidememo_store_t* store,
                  const char* fact_id,
                  bool pinned,
                  const char* source_id);
char* aidememo_fact_list(const aidememo_store_t* store,
                   const char* entity,         /* may be NULL */
                   const char* fact_type,      /* may be NULL */
                   uint32_t    limit,          /* 0 = no limit */
                   bool        current_only);  /* exclude superseded */
char* aidememo_fact_list_scoped(const aidememo_store_t* store,
                   const char* entity,
                   const char* fact_type,
                   uint32_t limit,
                   bool current_only,
                   const char* source_id);
char* aidememo_fact_delete(const aidememo_store_t* store, const char* fact_id);
/* Mark old_id as superseded by new_id (validity windows). */
char* aidememo_fact_supersede(const aidememo_store_t* store,
                        const char* old_id,
                        const char* new_id);

/* Relations. relation_add_scoped owns the edge in source_id;
 * relations_get_scoped returns exact-scope edges and does not inherit legacy
 * unscoped relations. */
char* aidememo_relation_add(const aidememo_store_t* store,
                      const char* source,
                      const char* target,
                      const char* rel_type);
char* aidememo_relation_add_scoped(const aidememo_store_t* store,
                      const char* source,
                      const char* target,
                      const char* rel_type,
                      const char* source_id);
char* aidememo_relation_remove(const aidememo_store_t* store,
                         const char* source,
                         const char* target,
                         const char* rel_type);
char* aidememo_relations_get(const aidememo_store_t* store,
                       const char* entity,
                       const char* direction); /* may be NULL → "both" */
char* aidememo_relations_get_scoped(const aidememo_store_t* store,
                       const char* entity,
                       const char* direction,
                       const char* source_id);

/* Ingest, lint, stats. */
char* aidememo_ingest(const aidememo_store_t* store, const char* wiki_root, bool incremental);
char* aidememo_lint(const aidememo_store_t* store);
char* aidememo_stats(const aidememo_store_t* store);

#ifdef __cplusplus
}
#endif

#endif /* AIDEMEMO_H */
