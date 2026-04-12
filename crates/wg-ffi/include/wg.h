/* WikiGraph C-ABI header */

#ifndef WG_H
#define WG_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque handle
typedef struct wg_store wg_store_t;

// Error codes
typedef enum wg_error {
    WG_OK = 0,
    WG_ERROR = 1,
    WG_NOT_FOUND = 2,
    WG_INVALID_INPUT = 3,
} wg_error_t;

// Lifecycle
wg_store_t* wg_open(const char* path, wg_error_t* error);
void wg_close(wg_store_t* store);

// Entity operations
wg_error_t wg_entity_add(wg_store_t* store, const char* name, const char* type);
wg_error_t wg_entity_get(wg_store_t* store, const char* name, char** output);
wg_error_t wg_entity_list(wg_store_t* store, char** output);
wg_error_t wg_entity_delete(wg_store_t* store, const char* name);

// Fact operations
wg_error_t wg_fact_add(wg_store_t* store, const char* content, const char* type, char** fact_id);
wg_error_t wg_fact_get(wg_store_t* store, const char* fact_id, char** output);
wg_error_t wg_fact_list(wg_store_t* store, const char* entity, char** output);
wg_error_t wg_fact_delete(wg_store_t* store, const char* fact_id);

// Search
wg_error_t wg_search(wg_store_t* store, const char* query, char** output);

// Graph traversal
wg_error_t wg_traverse(wg_store_t* store, const char* entity, int depth, char** output);
wg_error_t wg_path_find(wg_store_t* store, const char* from, const char* to, char** output);

// Statistics
wg_error_t wg_stats(wg_store_t* store, char** output);

#ifdef __cplusplus
}
#endif

#endif // WG_H
