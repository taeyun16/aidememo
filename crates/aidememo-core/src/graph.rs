//! Graph traversal operations for AideMemo.
//!
//! Provides BFS/DFS traversal, path finding, and graph health checks.

use crate::backend::StoreBackend;
use crate::error::{AideMemoError, Result};
use crate::types::*;

/// Graph traversal engine.
pub struct Graph<'a, B: StoreBackend + ?Sized> {
    store: &'a B,
}

impl<'a, B: StoreBackend + ?Sized> Graph<'a, B> {
    /// Create a new graph traversal engine.
    pub fn new(store: &'a B) -> Self {
        Self { store }
    }

    /// Traverse the graph from a starting entity.
    ///
    /// Uses BFS to find all reachable entities within the given depth.
    pub fn traverse(&self, start_name: &str, opts: TraverseOpts) -> Result<TraverseResult> {
        self.traverse_scoped(start_name, opts, None)
    }

    /// Traverse the graph while restricting entities and relations to
    /// `source_id`. Scoped edges must be explicitly owned by that namespace;
    /// legacy unscoped edges are hidden.
    pub fn traverse_scoped(
        &self,
        start_name: &str,
        opts: TraverseOpts,
        source_id: Option<&str>,
    ) -> Result<TraverseResult> {
        let start_id = self.store.entity_get_scoped(start_name, source_id)?.id;
        let max_depth = opts.depth;
        let direction = opts.direction;
        let relation_types: Option<Vec<String>> = opts
            .relation_types
            .map(|v| v.into_iter().map(|rt| rt.0).collect());

        let mut visited: std::collections::HashSet<EntityId> = std::collections::HashSet::new();
        let mut entities: Vec<EntitySummary> = Vec::new();
        let mut relations: Vec<RelationRecord> = Vec::new();

        // BFS queue: (entity_id, depth)
        let mut queue: Vec<(EntityId, u32)> = vec![(start_id, 0)];
        visited.insert(start_id);

        while let Some((current_id, depth)) = queue.pop() {
            if depth > max_depth {
                continue;
            }

            // Get current entity
            let entity_record = self.store.entity_get_by_id_scoped(current_id, source_id)?;

            // Count facts for this entity
            let fact_count = self.count_entity_facts_scoped(&current_id, source_id)?;

            entities.push(EntitySummary {
                id: current_id,
                name: entity_record.name.clone(),
                entity_type: entity_record.entity_type.clone(),
                fact_count,
                tags: entity_record.tags.clone(),
            });

            // Get relations based on direction
            let rels = self.get_relations_for_entity(&current_id, direction, source_id)?;

            for rel in rels {
                // Filter by relation type if specified
                if let Some(ref types) = relation_types
                    && !types.contains(&rel.relation_type.0)
                {
                    continue;
                }

                // Directional relation records always retain their persisted
                // source/target orientation, including reverse lookups. Pick
                // the endpoint opposite the current node before expanding.
                let next_id = if rel.source_id == current_id {
                    rel.target_id
                } else {
                    rel.source_id
                };
                if source_id.is_some()
                    && self.store.count_entity_facts_scoped(&next_id, source_id)? == 0
                {
                    continue;
                }

                relations.push(rel.clone());

                // Add the neighbouring endpoint to the queue if not visited.
                if !visited.contains(&next_id) {
                    visited.insert(next_id);
                    queue.push((next_id, depth + 1));
                }
            }
        }

        Ok(TraverseResult {
            entities,
            relations,
            visited_count: visited.len(),
        })
    }

    /// Find a path between two entities.
    pub fn path_find(&self, from_name: &str, to_name: &str) -> Result<Option<Vec<PathStep>>> {
        self.path_find_scoped(from_name, to_name, None)
    }

    /// Find a path whose entities are visible in an optional source
    /// namespace. `None` preserves the unscoped path search.
    pub fn path_find_scoped(
        &self,
        from_name: &str,
        to_name: &str,
        source_id: Option<&str>,
    ) -> Result<Option<Vec<PathStep>>> {
        let from_id = self.store.entity_get_scoped(from_name, source_id)?.id;
        let to_id = self.store.entity_get_scoped(to_name, source_id)?.id;

        if from_id == to_id {
            return Ok(Some(Vec::new()));
        }

        // BFS to find shortest path
        let mut visited: std::collections::HashSet<EntityId> = std::collections::HashSet::new();
        let mut queue: Vec<(EntityId, Vec<PathStep>)> = vec![(from_id, Vec::new())];
        visited.insert(from_id);

        while let Some((current_id, mut path)) = queue.pop() {
            let rels =
                self.get_relations_for_entity(&current_id, TraverseDirection::Forward, source_id)?;

            for rel in rels {
                let next_id = rel.target_id;

                if source_id.is_some()
                    && self.store.count_entity_facts_scoped(&next_id, source_id)? == 0
                {
                    continue;
                }

                if next_id == to_id {
                    path.push(PathStep {
                        from: current_id,
                        relation_type: rel.relation_type,
                        to: next_id,
                    });
                    return Ok(Some(path));
                }

                if !visited.contains(&next_id) {
                    visited.insert(next_id);
                    let mut new_path = path.clone();
                    new_path.push(PathStep {
                        from: current_id,
                        relation_type: rel.relation_type,
                        to: next_id,
                    });
                    queue.push((next_id, new_path));
                }
            }
        }

        Ok(None)
    }

    /// Find all paths between two entities (for cycle detection).
    pub fn find_all_paths(&self, from_name: &str, to_name: &str) -> Result<Vec<Vec<PathStep>>> {
        let from_id = self.store.resolve_entity(from_name)?;
        let to_id = self.store.resolve_entity(to_name)?;

        let mut all_paths: Vec<Vec<PathStep>> = Vec::new();
        let mut visited: std::collections::HashSet<EntityId> = std::collections::HashSet::new();

        self.dfs_find_paths(from_id, to_id, Vec::new(), &mut visited, &mut all_paths)?;

        Ok(all_paths)
    }

    fn dfs_find_paths(
        &self,
        current: EntityId,
        target: EntityId,
        mut path: Vec<PathStep>,
        visited: &mut std::collections::HashSet<EntityId>,
        all_paths: &mut Vec<Vec<PathStep>>,
    ) -> Result<()> {
        if current == target {
            all_paths.push(path);
            return Ok(());
        }

        visited.insert(current);

        let rels = self.get_relations_for_entity(&current, TraverseDirection::Forward, None)?;

        for rel in rels {
            let next_id = rel.target_id;

            if !visited.contains(&next_id) {
                path.push(PathStep {
                    from: current,
                    relation_type: rel.relation_type.clone(),
                    to: next_id,
                });

                self.dfs_find_paths(next_id, target, path.clone(), visited, all_paths)?;
                path.pop();
            }
        }

        visited.remove(&current);

        Ok(())
    }

    /// Detect cycles in the graph.
    pub fn detect_cycles(&self) -> Result<Vec<Vec<EntityId>>> {
        let mut cycles: Vec<Vec<EntityId>> = Vec::new();
        let mut visited: std::collections::HashSet<EntityId> = std::collections::HashSet::new();
        let mut recursion_stack: std::collections::HashSet<EntityId> =
            std::collections::HashSet::new();
        let mut path: Vec<EntityId> = Vec::new();

        // Get all entity IDs
        let entities = self.store.entity_list(ListOpts {
            limit: Some(10000),
            ..Default::default()
        })?;

        for entity in entities {
            if !visited.contains(&entity.id) {
                self.dfs_detect_cycles(
                    entity.id,
                    &mut visited,
                    &mut recursion_stack,
                    &mut path,
                    &mut cycles,
                )?;
            }
        }

        Ok(cycles)
    }

    fn dfs_detect_cycles(
        &self,
        current: EntityId,
        visited: &mut std::collections::HashSet<EntityId>,
        recursion_stack: &mut std::collections::HashSet<EntityId>,
        path: &mut Vec<EntityId>,
        cycles: &mut Vec<Vec<EntityId>>,
    ) -> Result<()> {
        visited.insert(current);
        recursion_stack.insert(current);
        path.push(current);

        let rels = self.get_relations_for_entity(&current, TraverseDirection::Forward, None)?;

        for rel in rels {
            let next_id = rel.target_id;

            if !visited.contains(&next_id) {
                if let Err(e) =
                    self.dfs_detect_cycles(next_id, visited, recursion_stack, path, cycles)
                {
                    // If cycle detected in recursion, propagate
                    if matches!(e, AideMemoError::CycleDetected { .. }) {
                        return Err(e);
                    }
                }
            } else if recursion_stack.contains(&next_id) {
                // Found a cycle
                let cycle_start = path.iter().position(|&id| id == next_id).unwrap();
                let cycle: Vec<EntityId> = path[cycle_start..].to_vec();
                cycles.push(cycle);
            }
        }

        path.pop();
        recursion_stack.remove(&current);

        Ok(())
    }

    /// Get direct relations for an entity.
    fn get_relations_for_entity(
        &self,
        entity_id: &EntityId,
        direction: TraverseDirection,
        scope_source_id: Option<&str>,
    ) -> Result<Vec<RelationRecord>> {
        self.store
            .relations_get_by_id_scoped(entity_id, direction, scope_source_id)
    }

    fn count_entity_facts_scoped(
        &self,
        entity_id: &EntityId,
        source_id: Option<&str>,
    ) -> Result<u32> {
        self.store.count_entity_facts_scoped(entity_id, source_id)
    }
}

#[cfg(all(test, any(feature = "sqlite", feature = "redb")))]
mod tests {
    use super::*;
    use crate::backend::{StoreBackend, StoreKind};
    use crate::config::Config;
    use tempfile::tempdir;

    fn create_test_store() -> (StoreKind, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let mut config = Config::default();
        if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
            config.store.backend = "redb".to_string();
        }
        let suffix = if config.store.backend == "redb" {
            "redb"
        } else {
            "sqlite"
        };
        let path = dir.path().join(format!("test.{suffix}"));
        config.store.path = path.to_string_lossy().into_owned();
        let store = StoreKind::open(&path, config).unwrap();
        (store, dir)
    }

    #[test]
    fn test_traverse() {
        let (mut store, _dir) = create_test_store();

        // Create entities
        store
            .entity_add(EntityInput {
                name: "Redis".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        store
            .entity_add(EntityInput {
                name: "Sentinel".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        store
            .entity_add(EntityInput {
                name: "PostgreSQL".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        // Create relations
        store
            .relation_add(RelationInput {
                source: "Sentinel".to_string(),
                target: "Redis".to_string(),
                scope_source_id: None,
                relation_type: RelationType::new("monitors"),
                weight: Some(1.0),
                evidence: None,
            })
            .unwrap();

        store
            .relation_add(RelationInput {
                source: "Redis".to_string(),
                target: "PostgreSQL".to_string(),
                scope_source_id: None,
                relation_type: RelationType::new("uses"),
                weight: Some(1.0),
                evidence: None,
            })
            .unwrap();

        let graph = Graph::new(&store);

        // Traverse from Sentinel
        let result = graph
            .traverse(
                "Sentinel",
                TraverseOpts {
                    depth: 2,
                    relation_types: None,
                    direction: TraverseDirection::Forward,
                },
            )
            .unwrap();

        assert!(result.entities.len() >= 2);
        assert!(!result.relations.is_empty());
    }

    #[test]
    fn test_path_find() {
        let (mut store, _dir) = create_test_store();

        // Create entities
        store
            .entity_add(EntityInput {
                name: "A".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        store
            .entity_add(EntityInput {
                name: "B".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        store
            .entity_add(EntityInput {
                name: "C".to_string(),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .unwrap();

        // Create path: A -> B -> C
        store
            .relation_add(RelationInput {
                source: "A".to_string(),
                target: "B".to_string(),
                scope_source_id: None,
                relation_type: RelationType::new("uses"),
                weight: Some(1.0),
                evidence: None,
            })
            .unwrap();

        store
            .relation_add(RelationInput {
                source: "B".to_string(),
                target: "C".to_string(),
                scope_source_id: None,
                relation_type: RelationType::new("uses"),
                weight: Some(1.0),
                evidence: None,
            })
            .unwrap();

        let graph = Graph::new(&store);

        // Find path A -> C
        let path = graph.path_find("A", "C").unwrap();
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.len(), 2); // A->B, B->C

        // No path exists
        let path = graph.path_find("C", "A").unwrap();
        assert!(path.is_none());
    }

    #[test]
    fn scoped_traverse_filters_entities_and_edges_before_limiting() {
        let (mut store, _dir) = create_test_store();

        let shared = store
            .entity_add(EntityInput {
                name: "Shared".to_string(),
                ..Default::default()
            })
            .unwrap();
        let alpha = store
            .entity_add(EntityInput {
                name: "AlphaOnly".to_string(),
                ..Default::default()
            })
            .unwrap();
        let beta = store
            .entity_add(EntityInput {
                name: "BetaOnly".to_string(),
                ..Default::default()
            })
            .unwrap();

        store
            .relation_add(RelationInput {
                source: "Shared".to_string(),
                target: "AlphaOnly".to_string(),
                scope_source_id: Some("alpha".to_string()),
                relation_type: RelationType::new("links"),
                weight: None,
                evidence: None,
            })
            .unwrap();
        store
            .relation_add(RelationInput {
                source: "Shared".to_string(),
                target: "BetaOnly".to_string(),
                scope_source_id: Some("beta".to_string()),
                relation_type: RelationType::new("links"),
                weight: None,
                evidence: None,
            })
            .unwrap();

        for (content, entity_id, source_id) in [
            ("shared alpha fact", shared, "alpha"),
            ("alpha-only fact", alpha, "alpha"),
            ("beta-only fact", beta, "beta"),
        ] {
            store
                .fact_add(FactInput {
                    content: content.to_string(),
                    entity_ids: Some(vec![entity_id]),
                    source_id: Some(source_id.to_string()),
                    ..Default::default()
                })
                .unwrap();
        }

        let result = Graph::new(&store)
            .traverse_scoped(
                "Shared",
                TraverseOpts {
                    depth: 1,
                    relation_types: None,
                    direction: TraverseDirection::Forward,
                },
                Some("alpha"),
            )
            .unwrap();
        let names: std::collections::HashSet<_> = result
            .entities
            .iter()
            .map(|entity| entity.name.as_str())
            .collect();
        assert_eq!(
            names,
            std::collections::HashSet::from(["Shared", "AlphaOnly"])
        );
        assert_eq!(result.relations.len(), 1);
        assert!(
            Graph::new(&store)
                .traverse_scoped("BetaOnly", TraverseOpts::default(), Some("alpha"))
                .is_err()
        );
    }

    #[test]
    fn scoped_traverse_keeps_same_edge_provenance_isolated() {
        let (mut store, _dir) = create_test_store();
        let source = store
            .entity_add(EntityInput {
                name: "SharedSource".to_string(),
                ..Default::default()
            })
            .unwrap();
        let target = store
            .entity_add(EntityInput {
                name: "SharedTarget".to_string(),
                ..Default::default()
            })
            .unwrap();
        for namespace in ["alpha", "beta"] {
            for (entity_id, suffix) in [(source, "source"), (target, "target")] {
                store
                    .fact_add(FactInput {
                        content: format!("{namespace} {suffix} visibility"),
                        entity_ids: Some(vec![entity_id]),
                        source_id: Some(namespace.to_string()),
                        ..Default::default()
                    })
                    .unwrap();
            }
        }
        for (namespace, relation_type, evidence) in [
            (Some("alpha"), "links", "alpha evidence"),
            (Some("beta"), "links", "beta evidence"),
            (None, "links", "legacy evidence"),
        ] {
            store
                .relation_add(RelationInput {
                    source: "SharedSource".to_string(),
                    target: "SharedTarget".to_string(),
                    scope_source_id: namespace.map(str::to_string),
                    relation_type: RelationType::new(relation_type),
                    weight: Some(1.0),
                    evidence: Some(vec![evidence.to_string()]),
                })
                .unwrap();
        }

        let graph = Graph::new(&store);
        let alpha = graph
            .traverse_scoped(
                "SharedSource",
                TraverseOpts {
                    depth: 1,
                    direction: TraverseDirection::Forward,
                    relation_types: None,
                },
                Some("alpha"),
            )
            .unwrap();
        assert_eq!(alpha.relations.len(), 1);
        assert_eq!(alpha.relations[0].relation_type.0, "links");
        assert_eq!(alpha.relations[0].evidence, ["alpha evidence"]);

        let beta = graph
            .traverse_scoped(
                "SharedSource",
                TraverseOpts {
                    depth: 1,
                    direction: TraverseDirection::Forward,
                    relation_types: None,
                },
                Some("beta"),
            )
            .unwrap();
        assert_eq!(beta.relations.len(), 1);
        assert_eq!(beta.relations[0].relation_type.0, "links");
        assert_eq!(beta.relations[0].evidence, ["beta evidence"]);

        let all = graph
            .traverse(
                "SharedSource",
                TraverseOpts {
                    depth: 1,
                    direction: TraverseDirection::Forward,
                    relation_types: None,
                },
            )
            .unwrap();
        assert_eq!(all.relations.len(), 3);
    }
}
