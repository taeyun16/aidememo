/* wg.h — C-ABI for Wiki-Graph (wg).
 *
 * All read functions return a heap-allocated, NUL-terminated UTF-8 JSON
 * string. The caller MUST free it with wg_free_string. On error the JSON
 * payload is `{"error": "..."}`.
 *
 * Build: link against libwg_ffi (cdylib or staticlib).
 *
 *   cc example.c -L /path/to/target/debug -lwg_ffi -o example
 *
 * Thread safety: a single wg_store_t* is safe to share across threads.
 */

#ifndef WG_H
#define WG_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque handle. */
typedef struct WgStore wg_store_t;

/* Lifecycle. */
wg_store_t* wg_open(const char* path);
void        wg_close(wg_store_t* store);
void        wg_free_string(char* s);
char*       wg_version(void);

/* Search & query. `current_only` excludes superseded facts. */
char* wg_search(const wg_store_t* store,
                const char* query,
                uint32_t limit,
                bool        current_only);
char* wg_query(const wg_store_t* store,
               const char* topic,
               uint32_t limit,
               uint32_t depth,
               uint32_t recent_limit,
               bool     current_only,
               const char* mode);  /* "naive"|"local"|"hybrid"|"global" or NULL */

/* Graph. */
char* wg_traverse(const wg_store_t* store,
                  const char* entity,
                  uint32_t depth,
                  const char* direction /* "forward" | "reverse" | "both" */);
char* wg_path_find(const wg_store_t* store, const char* from, const char* to);

/* Entity CRUD.
 * tags_json / aliases_json: JSON arrays of strings, e.g. "[\"a\",\"b\"]".
 *                           Pass NULL or "" to omit.
 */
char* wg_entity_add(const wg_store_t* store,
                    const char* name,
                    const char* entity_type,    /* may be NULL */
                    const char* tags_json,      /* may be NULL */
                    const char* aliases_json,   /* may be NULL */
                    const char* source_page);   /* may be NULL */
char* wg_entity_get(const wg_store_t* store, const char* name);
char* wg_entity_list(const wg_store_t* store,
                     uint32_t limit,            /* 0 = no limit */
                     const char* entity_type);  /* may be NULL */
char* wg_entity_delete(const wg_store_t* store, const char* name);
/* Set the compiled-truth summary; pass "" or NULL to clear. */
char* wg_entity_describe(const wg_store_t* store,
                         const char* name,
                         const char* summary);
char* wg_resolve_entity(const wg_store_t* store, const char* name);

/* Fact CRUD. */
char* wg_fact_add(const wg_store_t* store,
                  const char* content,
                  const char* entity_ids_json,  /* JSON array of ULIDs, may be NULL */
                  const char* fact_type,        /* may be NULL */
                  const char* tags_json,        /* may be NULL */
                  const char* source,           /* may be NULL */
                  float       confidence);      /* 0.0 = unset */
/* Insert many facts in one redb write transaction. items_json is a
 * JSON array of objects matching wg_fact_add's args:
 *   content (required), entity_ids, fact_type, tags, source, confidence
 * Returns {"ids":[...]} on success. */
char* wg_fact_add_many(const wg_store_t* store, const char* items_json);
char* wg_fact_get(const wg_store_t* store, const char* fact_id);
char* wg_fact_list(const wg_store_t* store,
                   const char* entity,         /* may be NULL */
                   const char* fact_type,      /* may be NULL */
                   uint32_t    limit,          /* 0 = no limit */
                   bool        current_only);  /* exclude superseded */
char* wg_fact_delete(const wg_store_t* store, const char* fact_id);
/* Mark old_id as superseded by new_id (validity windows). */
char* wg_fact_supersede(const wg_store_t* store,
                        const char* old_id,
                        const char* new_id);

/* Relations. */
char* wg_relation_add(const wg_store_t* store,
                      const char* source,
                      const char* target,
                      const char* rel_type);
char* wg_relation_remove(const wg_store_t* store,
                         const char* source,
                         const char* target,
                         const char* rel_type);
char* wg_relations_get(const wg_store_t* store,
                       const char* entity,
                       const char* direction); /* may be NULL → "both" */

/* Ingest, lint, stats. */
char* wg_ingest(const wg_store_t* store, const char* wiki_root, bool incremental);
char* wg_lint(const wg_store_t* store);
char* wg_stats(const wg_store_t* store);

#ifdef __cplusplus
}
#endif

#endif /* WG_H */
