//! Fuzzy matching for entity names.
//!
//! Provides strsim + trigram-based fuzzy matching for typo tolerance
//! and entity name suggestions.

use crate::error::Result;
use crate::store::Store;
use crate::types::*;

/// Fuzzy matcher for entity names.
pub struct FuzzyMatcher<'a> {
    store: &'a Store,
}

impl<'a> FuzzyMatcher<'a> {
    /// Create a new fuzzy matcher.
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Find the best matching entity for a query string.
    ///
    /// Returns the best match with similarity score > threshold.
    pub fn best_match(&self, query: &str, threshold: f32) -> Result<Option<(EntitySummary, f32)>> {
        let query_lower = query.to_lowercase();
        let entities = self.store.entity_list(ListOpts {
            limit: Some(10000),
            ..Default::default()
        })?;

        let mut best: Option<(EntitySummary, f32)> = None;

        for entity in entities {
            // Check name
            let name_sim = trigram::similarity(&query_lower, &entity.name.to_lowercase());

            // Check aliases
            let mut alias_sim = 0.0f32;
            let entity_record = self.store.entity_get_by_id(entity.id)?;
            for alias in &entity_record.aliases {
                let sim = trigram::similarity(&query_lower, &alias.to_lowercase());
                alias_sim = alias_sim.max(sim);
            }

            let max_sim = name_sim.max(alias_sim);

            if max_sim >= threshold {
                if let Some(ref current) = best {
                    if max_sim > current.1 {
                        best = Some((entity, max_sim));
                    }
                } else {
                    best = Some((entity, max_sim));
                }
            }
        }

        Ok(best)
    }

    /// Find all entities matching a query string with similarity >= threshold.
    pub fn find_matches(&self, query: &str, threshold: f32) -> Result<Vec<(EntitySummary, f32)>> {
        let query_lower = query.to_lowercase();
        let entities = self.store.entity_list(ListOpts {
            limit: Some(10000),
            ..Default::default()
        })?;

        let mut matches: Vec<(EntitySummary, f32)> = Vec::new();

        for entity in entities {
            let name_sim = trigram::similarity(&query_lower, &entity.name.to_lowercase());

            let entity_record = self.store.entity_get_by_id(entity.id)?;
            let mut max_alias_sim = 0.0f32;
            for alias in &entity_record.aliases {
                let sim = trigram::similarity(&query_lower, &alias.to_lowercase());
                max_alias_sim = max_alias_sim.max(sim);
            }

            let max_sim = name_sim.max(max_alias_sim);

            if max_sim >= threshold {
                matches.push((entity, max_sim));
            }
        }

        // Sort by similarity descending
        matches.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(matches)
    }

    /// Calculate Levenshtein distance between two strings.
    pub fn levenshtein(&self, s1: &str, s2: &str) -> usize {
        strsim::levenshtein(s1, s2)
    }

    /// Calculate Jaro-Winkler similarity between two strings.
    pub fn jaro_winkler(&self, s1: &str, s2: &str) -> f64 {
        strsim::jaro_winkler(s1, s2)
    }

    /// Calculate trigram similarity between two strings.
    pub fn trigram_sim(&self, s1: &str, s2: &str) -> f32 {
        trigram::similarity(s1, s2)
    }

    /// Calculate normalized trigram similarity (0.0 to 1.0).
    /// Note: trigram::similarity returns 0.0-1.0 directly.
    pub fn normalized_trigram(&self, s1: &str, s2: &str) -> f32 {
        trigram::similarity(s1, s2)
    }
}

/// Extension trait for entity fuzzy matching.
pub trait EntityFuzzyExt {
    /// Get entity by name with fuzzy matching fallback.
    fn entity_get_fuzzy(&self, name: &str) -> crate::error::Result<EntityRecord>;
}

impl EntityFuzzyExt for Store {
    fn entity_get_fuzzy(&self, name: &str) -> crate::error::Result<EntityRecord> {
        // Try exact match first
        match self.entity_get(name) {
            Ok(entity) => return Ok(entity),
            Err(_) => {}
        }

        // Try fuzzy match
        let matcher = FuzzyMatcher::new(self);
        if let Some((entity, _score)) = matcher.best_match(name, 0.5)? {
            return self.entity_get_by_id(entity.id);
        }

        // Return original not found error
        let suggestions = self.suggest_similar_entities(name).unwrap_or_default();
        Err(crate::error::WgError::entity_not_found(
            name.to_string(),
            suggestions,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tempfile::tempdir;

    fn create_test_store() -> (Store, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let config = Config::default();
        let store = Store::open(&path, config).unwrap();
        (store, dir)
    }

    #[test]
    fn test_fuzzy_matcher() {
        let (mut store, _dir) = create_test_store();

        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                aliases: Some(vec!["redis-server".to_string()]),
                ..Default::default()
            })
            .unwrap();

        store
            .entity_add(EntityInput {
                name: "PostgreSQL".to_string(),
                entity_type: Some(EntityType::Technology),
                aliases: Some(vec!["postgres".to_string()]),
                ..Default::default()
            })
            .unwrap();

        let matcher = FuzzyMatcher::new(&store);

        // Exact match
        let result = matcher.best_match("Redis", 0.0).unwrap();
        assert!(result.is_some());

        // Fuzzy match (typo)
        let result = matcher.best_match("Redsi", 0.3).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().0.name, "Redis");

        // Alias match
        let result = matcher.best_match("redis-server", 0.5).unwrap();
        assert!(result.is_some());

        // No match
        let result = matcher.best_match("NonExistent", 0.9).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_trigram_similarity() {
        let (store, _dir) = create_test_store();
        let matcher = FuzzyMatcher::new(&store);

        // High similarity
        let sim = matcher.trigram_sim("Redis", "Redis");
        assert!(sim > 0.9);

        // Medium similarity
        let sim = matcher.trigram_sim("Redis", "Redsi");
        assert!(sim > 0.3);

        // Low similarity
        let sim = matcher.trigram_sim("Redis", "PostgreSQL");
        assert!(sim < 0.5);
    }

    #[test]
    fn test_entity_fuzzy_ext() {
        let (mut store, _dir) = create_test_store();

        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                aliases: Some(vec!["redis-cache".to_string()]),
                ..Default::default()
            })
            .unwrap();

        // Exact match
        let result = store.entity_get_fuzzy("Redis").unwrap();
        assert_eq!(result.name, "Redis");

        // Alias match
        let result = store.entity_get_fuzzy("redis-cache").unwrap();
        assert_eq!(result.name, "Redis");

        // Fuzzy match
        let result = store.entity_get_fuzzy("Reddis").unwrap();
        assert_eq!(result.name, "Redis");

        // No match
        let result = store.entity_get_fuzzy("NonExistentXYZ");
        assert!(result.is_err());
    }
}
