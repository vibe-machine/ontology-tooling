use serde::Serialize;

use super::model::{CorpusItem, CorpusManifest, ItemCategory, PromptExport};
use super::ItemFilter;

/// Default repo-relative path for the prompt-export artifact written by
/// `ont corpus export` and consumed by Lingo / TypeQL prompt fixtures.
pub const DEFAULT_EXPORT_PATH: &str = "generated/corpus/typeql-prompt-examples.json";

/// On-disk shape of the deterministic prompt-export artifact.
#[derive(Debug, Clone, Serialize)]
pub struct PromptExportArtifact {
    pub schema_version: u32,
    pub ontology_package: String,
    pub source_corpus_version: u32,
    pub item_count: usize,
    pub items: Vec<CorpusItem>,
}

impl PromptExportArtifact {
    pub fn new(manifest: &CorpusManifest, items: Vec<CorpusItem>) -> Self {
        Self {
            schema_version: 1,
            ontology_package: manifest.ontology_package.clone(),
            source_corpus_version: manifest.version,
            item_count: items.len(),
            items,
        }
    }
}

/// Select the items eligible for prompt export. Items must be:
/// - in the `KnownGood` category (i.e. came from `corpus/queries/`)
/// - tagged `prompt_export = include`
/// - match the supplied `tag` filter, when one is provided
///
/// Results are sorted by item id for deterministic output.
pub fn select_exportable_items(items: &[CorpusItem], tag: Option<&str>) -> Vec<CorpusItem> {
    let filter = ItemFilter {
        tag: tag.map(|s| s.to_string()),
        item_id: None,
        category: Some(ItemCategory::KnownGood),
    };
    let mut selected: Vec<CorpusItem> = items
        .iter()
        .filter(|i| filter.matches(i))
        .filter(|i| matches!(i.prompt_export, PromptExport::Include))
        .cloned()
        .collect();
    selected.sort_by(|a, b| a.id.cmp(&b.id));
    selected
}
