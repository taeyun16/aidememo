//! Import/Export functionality for JSONL format.

use crate::error::{AideMemoError, Result};
use crate::store::Store;
use crate::types::*;
use std::io::{Read, Write};

/// Exporter for JSONL format.
pub struct Exporter<'a> {
    store: &'a Store,
}

impl<'a> Exporter<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Export data to JSONL.
    pub fn export_jsonl(&self, writer: &mut dyn Write, scope: ExportScope) -> Result<ExportStats> {
        let mut stats = ExportStats::default();

        // Write header with schema version
        let header = serde_json::json!({
            "schema_version": 1,
            "exported_by": "aidememo 0.1.0",
        });
        writeln!(writer, "{}", header).map_err(|e| AideMemoError::Serialize {
            context: "export header".to_string(),
            source: serde_json::Error::io(e),
        })?;

        // Export entities
        if matches!(scope, ExportScope::All | ExportScope::Entities) {
            let entities = self.store.entity_list(ListOpts {
                limit: Some(100000),
                ..Default::default()
            })?;

            for entity in entities {
                let record = self.store.entity_get_by_id(entity.id)?;
                let json = serde_json::json!({
                    "type": "entity",
                    "data": record,
                });
                writeln!(writer, "{}", json).map_err(|e| AideMemoError::Serialize {
                    context: "export entity".to_string(),
                    source: serde_json::Error::io(e),
                })?;
                stats.entities_exported += 1;
            }
        }

        // Export relations
        if matches!(scope, ExportScope::All | ExportScope::Relations) {
            let entities = self.store.entity_list(ListOpts {
                limit: Some(100000),
                ..Default::default()
            })?;

            for entity in entities {
                let relations = self
                    .store
                    .relations_get(&entity.name, TraverseDirection::Both)?;
                for rel in relations {
                    let json = serde_json::json!({
                        "type": "relation",
                        "data": rel,
                    });
                    writeln!(writer, "{}", json).map_err(|e| AideMemoError::Serialize {
                        context: "export relation".to_string(),
                        source: serde_json::Error::io(e),
                    })?;
                    stats.relations_exported += 1;
                }
            }
        }

        // Export facts
        if matches!(scope, ExportScope::All | ExportScope::Facts) {
            let facts = self.store.fact_list(FactListOpts {
                limit: Some(100000),
                ..Default::default()
            })?;

            for fact in facts {
                let json = serde_json::json!({
                    "type": "fact",
                    "data": fact,
                });
                writeln!(writer, "{}", json).map_err(|e| AideMemoError::Serialize {
                    context: "export fact".to_string(),
                    source: serde_json::Error::io(e),
                })?;
                stats.facts_exported += 1;
            }
        }

        Ok(stats)
    }
}

/// Importer for JSONL format.
pub struct Importer<'a> {
    store: &'a mut Store,
}

impl<'a> Importer<'a> {
    pub fn new(store: &'a mut Store) -> Self {
        Self { store }
    }

    /// Import data from JSONL.
    pub fn import_jsonl(&mut self, reader: &mut dyn Read) -> Result<ImportStats> {
        let mut stats = ImportStats::default();

        let mut content = String::new();
        reader
            .read_to_string(&mut content)
            .map_err(|e| AideMemoError::Deserialize {
                context: "read import data".to_string(),
                source: serde_json::Error::io(e),
            })?;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Skip header lines
            if line.starts_with("{\"schema_version\"") || line.starts_with("{\"exported_by\"") {
                continue;
            }

            let json: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_e) => {
                    stats.errors += 1;
                    continue;
                }
            };

            let record_type = json.get("type").and_then(|v| v.as_str());
            let data = json.get("data");

            match (record_type, data) {
                (Some("entity"), Some(data)) => {
                    if let Ok(record) = serde_json::from_value::<EntityRecord>(data.clone()) {
                        // Check if entity already exists
                        if self.store.entity_get(&record.name).is_err() {
                            let input = EntityInput {
                                name: record.name,
                                entity_type: Some(record.entity_type),
                                aliases: Some(record.aliases),
                                tags: Some(record.tags),
                                source_page: record.source_page,
                            };
                            if self.store.entity_add(input).is_ok() {
                                stats.entities_imported += 1;
                            } else {
                                stats.errors += 1;
                            }
                        }
                    } else {
                        stats.errors += 1;
                    }
                }
                (Some("relation"), Some(data)) => {
                    if let Ok(record) = serde_json::from_value::<RelationRecord>(data.clone()) {
                        let input = RelationInput {
                            source: self
                                .store
                                .entity_get_by_id(record.source_id)
                                .map(|e| e.name)
                                .unwrap_or_default(),
                            target: self
                                .store
                                .entity_get_by_id(record.target_id)
                                .map(|e| e.name)
                                .unwrap_or_default(),
                            relation_type: record.relation_type,
                            weight: Some(record.weight),
                            evidence: Some(record.evidence),
                        };
                        if input.source.is_empty() || input.target.is_empty() {
                            stats.errors += 1;
                        } else if self.store.relation_add(input).is_ok() {
                            stats.relations_imported += 1;
                        } else {
                            stats.errors += 1;
                        }
                    } else {
                        stats.errors += 1;
                    }
                }
                (Some("fact"), Some(data)) => {
                    if let Ok(record) = serde_json::from_value::<FactRecord>(data.clone()) {
                        let input = FactInput {
                            content: record.content,
                            fact_type: Some(record.fact_type),
                            entity_ids: Some(record.entity_ids),
                            tags: Some(record.tags),
                            source: record.source,
                            source_id: record.source_id,
                            source_confidence: Some(record.source_confidence),
                            observed_at: record.observed_at,
                        };
                        if self.store.fact_add(input).is_ok() {
                            stats.facts_imported += 1;
                        } else {
                            stats.errors += 1;
                        }
                    } else {
                        stats.errors += 1;
                    }
                }
                _ => {
                    stats.errors += 1;
                }
            }
        }

        Ok(stats)
    }
}
