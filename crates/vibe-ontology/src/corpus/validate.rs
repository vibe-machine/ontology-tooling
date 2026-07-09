use std::collections::HashSet;

use crate::error::{invalid, Result};

use super::model::{CorpusItem, CorpusManifest, ExpectedKind};

pub(crate) fn validate_manifest(manifest: &CorpusManifest, source: &str) -> Result<()> {
    if manifest.version == 0 {
        return Err(invalid(format!(
            "{source}: version must be a positive integer"
        )));
    }
    if manifest.ontology_package.trim().is_empty() {
        return Err(invalid(format!(
            "{source}: ontology_package must be non-empty"
        )));
    }
    let mut seen = HashSet::new();
    for fixture in &manifest.fixtures {
        if fixture.id.trim().is_empty() {
            return Err(invalid(format!("{source}: fixture id must be non-empty")));
        }
        if !seen.insert(&fixture.id) {
            return Err(invalid(format!(
                "{source}: fixture id '{}' is duplicated",
                fixture.id
            )));
        }
        if let Some(value) = &fixture.schema {
            if value.trim().is_empty() {
                return Err(invalid(format!(
                    "{source}: fixture '{}' field 'schema' must be non-empty when present",
                    fixture.id
                )));
            }
        }
        if let Some(value) = &fixture.data {
            if value.trim().is_empty() {
                return Err(invalid(format!(
                    "{source}: fixture '{}' field 'data' must be non-empty when present",
                    fixture.id
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_item(item: &CorpusItem, fixture_ids: &HashSet<String>) -> Result<()> {
    let context = format!("{}#{}", item.source, item.id);
    if item.id.trim().is_empty() {
        return Err(invalid(format!(
            "{}: item.id must be non-empty",
            item.source
        )));
    }
    if item.title.trim().is_empty() {
        return Err(invalid(format!("{context}: item.title must be non-empty")));
    }
    if item.natural_language_intent.trim().is_empty() {
        return Err(invalid(format!(
            "{context}: item.natural_language_intent must be non-empty"
        )));
    }
    if item.typeql.trim().is_empty() {
        return Err(invalid(format!("{context}: item.typeql must be non-empty")));
    }
    if item.ontology_tags.iter().any(|tag| tag.trim().is_empty()) {
        return Err(invalid(format!(
            "{context}: ontology_tags must contain only non-empty strings"
        )));
    }
    if item.fixtures.iter().any(|id| id.trim().is_empty()) {
        return Err(invalid(format!(
            "{context}: fixtures must reference non-empty fixture ids"
        )));
    }
    for fixture_id in &item.fixtures {
        if !fixture_ids.contains(fixture_id) {
            return Err(invalid(format!(
                "{context}: references unknown fixture id '{fixture_id}'"
            )));
        }
    }
    match item.expected.kind {
        ExpectedKind::RowCount => {
            if item.expected.value.is_none() {
                return Err(invalid(format!(
                    "{context}: expected.value is required for row_count"
                )));
            }
        }
        ExpectedKind::JsonShape => {
            if item.expected.shape.is_none() {
                return Err(invalid(format!(
                    "{context}: expected.shape is required for json_shape"
                )));
            }
        }
        ExpectedKind::NonEmpty | ExpectedKind::Empty => {}
    }
    if let Some(provenance) = &item.provenance {
        if provenance.trim().is_empty() {
            return Err(invalid(format!(
                "{context}: provenance must be non-empty when present"
            )));
        }
    }
    Ok(())
}
