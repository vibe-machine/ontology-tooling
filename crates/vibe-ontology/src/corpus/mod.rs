//! Corpus model, discovery, validation, and prompt-export selection.

mod discover;
mod export;
mod model;
mod validate;

pub use discover::{discover_corpus, Corpus};
pub use export::{select_exportable_items, PromptExportArtifact, DEFAULT_EXPORT_PATH};
pub use model::{
    CorpusItem, CorpusManifest, ExpectedKind, ExpectedResult, Fixture, ItemCategory, PromptExport,
    QueryKind,
};

/// Filter criteria applied across the corpus item set.
#[derive(Debug, Default, Clone)]
pub struct ItemFilter {
    pub tag: Option<String>,
    pub item_id: Option<String>,
    pub category: Option<ItemCategory>,
}

impl ItemFilter {
    pub fn matches(&self, item: &CorpusItem) -> bool {
        if let Some(category) = self.category {
            if item.category != category {
                return false;
            }
        }
        if let Some(tag) = &self.tag {
            if !item.ontology_tags.iter().any(|t| t == tag) {
                return false;
            }
        }
        if let Some(id) = &self.item_id {
            if item.id != *id {
                return false;
            }
        }
        true
    }
}

/// Apply an `ItemFilter` to an iterator of items, preserving order.
pub fn filter_items<'a, I>(items: I, filter: &'a ItemFilter) -> impl Iterator<Item = &'a CorpusItem>
where
    I: IntoIterator<Item = &'a CorpusItem>,
{
    items.into_iter().filter(move |item| filter.matches(item))
}
