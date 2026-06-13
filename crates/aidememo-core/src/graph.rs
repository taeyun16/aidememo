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
        let start_id = self.store.resolve_entity(start_name)?;
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
            let entity_record = self.store.entity_get_by_id(current_id)?;

            // Count facts for this entity
            let fact_count = self.count_entity_facts(&current_id)?;

            entities.push(EntitySummary {
                id: current_id,
                name: entity_record.name.clone(),
                entity_type: entity_record.entity_type.clone(),
                fact_count,
                tags: entity_record.tags.clone(),
            });

            // Get relations based on direction
            let rels = self.get_relations_for_entity(&current_id, direction)?;

            for rel in rels {
                // Filter by relation type if specified
                if let Some(ref types) = relation_types {
                    if !types.contains(&rel.relation_type.0) {
                        continue;
                    }
                }

                relations.push(rel.clone());

                // Add target to queue if not visited
                let next_id = rel.target_id;
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
        let from_id = self.store.resolve_entity(from_name)?;
        let to_id = self.store.resolve_entity(to_name)?;

        if from_id == to_id {
            return Ok(Some(Vec::new()));
        }

        // BFS to find shortest path
        let mut visited: std::collections::HashSet<EntityId> = std::collections::HashSet::new();
        let mut queue: Vec<(EntityId, Vec<PathStep>)> = vec![(from_id, Vec::new())];
        visited.insert(from_id);

        while let Some((current_id, mut path)) = queue.pop() {
            let rels = self.get_relations_for_entity(&current_id, TraverseDirection::Forward)?;

            for rel in rels {
                let next_id = rel.target_id;

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

        let rels = self.get_relations_for_entity(&current, TraverseDirection::Forward)?;

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

        let rels = self.get_relations_for_entity(&current, TraverseDirection::Forward)?;

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
    ) -> Result<Vec<RelationRecord>> {
        self.store.relations_get_by_id(entity_id, direction)
    }

    /// Count facts for an entity.
    ///
    /// Uses the `fact_by_entity` secondary index (range scan) instead
    /// of a full `fact_list` deserialize — important for traversal,
    /// where this is called once per visited entity. The previous
    /// implementation made traverse_d3 cost O(visited × total facts).
    fn count_entity_facts(&self, entity_id: &EntityId) -> Result<u32> {
        self.store.count_entity_facts(entity_id)
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
                relation_type: RelationType::new("monitors"),
                weight: Some(1.0),
                evidence: None,
            })
            .unwrap();

        store
            .relation_add(RelationInput {
                source: "Redis".to_string(),
                target: "PostgreSQL".to_string(),
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
                relation_type: RelationType::new("uses"),
                weight: Some(1.0),
                evidence: None,
            })
            .unwrap();

        store
            .relation_add(RelationInput {
                source: "B".to_string(),
                target: "C".to_string(),
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
}
