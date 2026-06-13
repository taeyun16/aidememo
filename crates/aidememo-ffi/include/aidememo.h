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
/* Open with an explicit backend: "sqlite" (default) or "redb" when the
 * library was built with the `redb` Cargo feature. Pass NULL or "" for the
 * default backend. */
aidememo_store_t* aidememo_open_with_backend(const char* path, const char* backend);
void        aidememo_close(aidememo_store_t* store);
void        aidememo_free_string(char* s);
char*       aidememo_version(void);

/* Search & query. `current_only` excludes superseded facts. */
char* aidememo_search(const aidememo_store_t* store,
                const char* query,
                uint32_t limit,
                bool        current_only);
char* aidememo_query(const aidememo_store_t* store,
               const char* topic,
               uint32_t limit,
               uint32_t depth,
               uint32_t recent_limit,
               bool     current_only,
               const char* mode);  /* "naive"|"local"|"hybrid"|"global" or NULL */

/* Graph. */
char* aidememo_traverse(const aidememo_store_t* store,
                  const char* entity,
                  uint32_t depth,
                  const char* direction /* "forward" | "reverse" | "both" */);
char* aidememo_path_find(const aidememo_store_t* store, const char* from, const char* to);

/* Entity CRUD.
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
char* aidememo_entity_list(const aidememo_store_t* store,
                     uint32_t limit,            /* 0 = no limit */
                     const char* entity_type);  /* may be NULL */
char* aidememo_entity_delete(const aidememo_store_t* store, const char* name);
/* Set the compiled-truth summary; pass "" or NULL to clear. */
char* aidememo_entity_describe(const aidememo_store_t* store,
                         const char* name,
                         const char* summary);
char* aidememo_resolve_entity(const aidememo_store_t* store, const char* name);

/* Fact CRUD. */
char* aidememo_fact_add(const aidememo_store_t* store,
                  const char* content,
                  const char* entity_ids_json,  /* JSON array of ULIDs, may be NULL */
                  const char* fact_type,        /* may be NULL */
                  const char* tags_json,        /* may be NULL */
                  const char* source,           /* may be NULL */
                  float       confidence);      /* 0.0 = unset */
/* Insert many facts in one backend transaction when supported. items_json is a
 * JSON array of objects matching aidememo_fact_add's args:
 *   content (required), entity_ids, fact_type, tags, source, confidence
 * Returns {"ids":[...]} on success. */
char* aidememo_fact_add_many(const aidememo_store_t* store, const char* items_json);
char* aidememo_fact_get(const aidememo_store_t* store, const char* fact_id);
char* aidememo_fact_list(const aidememo_store_t* store,
                   const char* entity,         /* may be NULL */
                   const char* fact_type,      /* may be NULL */
                   uint32_t    limit,          /* 0 = no limit */
                   bool        current_only);  /* exclude superseded */
char* aidememo_fact_delete(const aidememo_store_t* store, const char* fact_id);
/* Mark old_id as superseded by new_id (validity windows). */
char* aidememo_fact_supersede(const aidememo_store_t* store,
                        const char* old_id,
                        const char* new_id);

/* Relations. */
char* aidememo_relation_add(const aidememo_store_t* store,
                      const char* source,
                      const char* target,
                      const char* rel_type);
char* aidememo_relation_remove(const aidememo_store_t* store,
                         const char* source,
                         const char* target,
                         const char* rel_type);
char* aidememo_relations_get(const aidememo_store_t* store,
                       const char* entity,
                       const char* direction); /* may be NULL → "both" */

/* Ingest, lint, stats. */
char* aidememo_ingest(const aidememo_store_t* store, const char* wiki_root, bool incremental);
char* aidememo_lint(const aidememo_store_t* store);
char* aidememo_stats(const aidememo_store_t* store);

#ifdef __cplusplus
}
#endif

#endif /* AIDEMEMO_H */
