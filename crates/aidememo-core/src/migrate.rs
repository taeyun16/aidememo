//! Import/Export functionality for JSONL format.

use crate::backend::StoreBackend;
use crate::error::{AideMemoError, Result};
use crate::types::*;
use std::io::{Read, Write};

/// Exporter for JSONL format.
pub struct Exporter<'a, B: StoreBackend + ?Sized> {
    store: &'a B,
}

impl<'a, B: StoreBackend + ?Sized> Exporter<'a, B> {
    pub fn new(store: &'a B) -> Self {
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
pub struct Importer<'a, B: StoreBackend + ?Sized> {
    store: &'a mut B,
}

impl<'a, B: StoreBackend + ?Sized> Importer<'a, B> {
    pub fn new(store: &'a mut B) -> Self {
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
                        match self.store.entity_upsert_record(record) {
                            Ok(true) => stats.entities_imported += 1,
                            Ok(false) => {}
                            Err(_) => stats.errors += 1,
                        }
                    } else {
                        stats.errors += 1;
                    }
                }
                (Some("relation"), Some(data)) => {
                    if let Ok(record) = serde_json::from_value::<RelationRecord>(data.clone()) {
                        match self.store.relation_upsert_record(record) {
                            Ok(true) => stats.relations_imported += 1,
                            Ok(false) => {}
                            Err(_) => stats.errors += 1,
                        }
                    } else {
                        stats.errors += 1;
                    }
                }
                (Some("fact"), Some(data)) => {
                    if let Ok(record) = serde_json::from_value::<FactRecord>(data.clone()) {
                        match self.store.fact_upsert_record(record) {
                            Ok(true) => stats.facts_imported += 1,
                            Ok(false) => {}
                            Err(_) => stats.errors += 1,
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
