use serde::{Deserialize, Serialize};

/// Top-level corpus manifest as it appears on disk in `corpus/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusManifest {
    pub version: u32,
    pub ontology_package: String,
    #[serde(default)]
    pub fixtures: Vec<Fixture>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Schema,
    Read,
    Write,
    Fetch,
    Reduce,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptExport {
    Include,
    Exclude,
    BadToGood,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedKind {
    RowCount,
    NonEmpty,
    Empty,
    JsonShape,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedResult {
    pub kind: ExpectedKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<serde_json::Value>,
}

/// Source category for a corpus item; populated by the discoverer based on
/// which directory the item was loaded from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ItemCategory {
    KnownGood,
    Negative,
}

/// On-disk shape of a single corpus item. The discoverer wraps each `RawItem`
/// into a `CorpusItem` annotated with provenance (`source`, `category`).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RawItem {
    pub id: String,
    pub title: String,
    pub natural_language_intent: String,
    pub query_kind: QueryKind,
    #[serde(default)]
    pub ontology_tags: Vec<String>,
    #[serde(default)]
    pub fixtures: Vec<String>,
    pub typeql: String,
    pub expected: ExpectedResult,
    pub prompt_export: PromptExport,
    #[serde(default)]
    pub provenance: Option<String>,
}

/// A single corpus item enriched with discovery-time provenance.
#[derive(Debug, Clone, Serialize)]
pub struct CorpusItem {
    pub id: String,
    pub title: String,
    pub natural_language_intent: String,
    pub query_kind: QueryKind,
    pub ontology_tags: Vec<String>,
    pub fixtures: Vec<String>,
    pub typeql: String,
    pub expected: ExpectedResult,
    pub prompt_export: PromptExport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    /// Repo-relative path of the JSON file the item was loaded from.
    pub source: String,
    pub category: ItemCategory,
}

impl CorpusItem {
    pub(crate) fn from_raw(raw: RawItem, source: String, category: ItemCategory) -> Self {
        Self {
            id: raw.id,
            title: raw.title,
            natural_language_intent: raw.natural_language_intent,
            query_kind: raw.query_kind,
            ontology_tags: raw.ontology_tags,
            fixtures: raw.fixtures,
            typeql: raw.typeql,
            expected: raw.expected,
            prompt_export: raw.prompt_export,
            provenance: raw.provenance,
            source,
            category,
        }
    }
}
