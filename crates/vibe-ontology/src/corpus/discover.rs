use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{invalid, Error, Result};

use super::model::{CorpusItem, CorpusManifest, ItemCategory, RawItem};
use super::validate::{validate_item, validate_manifest};

const CORPUS_SUBDIR: &str = "corpus";
const MANIFEST_NAME: &str = "manifest.json";
const KNOWN_GOOD_DIR: &str = "queries";
const NEGATIVE_DIR: &str = "negative";

/// Result of a successful corpus discovery.
#[derive(Debug, Clone)]
pub struct Corpus {
    pub repo_path: PathBuf,
    pub corpus_dir: PathBuf,
    pub manifest: CorpusManifest,
    pub items: Vec<CorpusItem>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ItemFilePayload {
    Wrapped { items: Vec<RawItem> },
    Many(Vec<RawItem>),
    Single(Box<RawItem>),
}

/// Discover and validate the corpus rooted at `<repo>/corpus/`.
///
/// Performs:
/// - manifest existence and shape checks
/// - fixture file existence
/// - per-item shape validation (enums, fixture refs, expected payloads)
/// - cross-file uniqueness of item ids
pub fn discover_corpus(repo_path: impl AsRef<Path>) -> Result<Corpus> {
    let repo_path = repo_path.as_ref().to_path_buf();
    let corpus_dir = repo_path.join(CORPUS_SUBDIR);
    if !corpus_dir.is_dir() {
        return Err(Error::CorpusMissing(corpus_dir));
    }

    let manifest_path = corpus_dir.join(MANIFEST_NAME);
    if !manifest_path.is_file() {
        return Err(Error::ManifestMissing(manifest_path));
    }
    let manifest: CorpusManifest = read_json(&manifest_path)?;
    validate_manifest(&manifest, "corpus/manifest.json")?;
    validate_fixture_files(&corpus_dir, &manifest)?;

    let fixture_ids: HashSet<String> =
        manifest.fixtures.iter().map(|f| f.id.clone()).collect();

    let mut items = Vec::new();
    items.extend(load_item_dir(
        &corpus_dir,
        KNOWN_GOOD_DIR,
        ItemCategory::KnownGood,
        &fixture_ids,
    )?);
    items.extend(load_item_dir(
        &corpus_dir,
        NEGATIVE_DIR,
        ItemCategory::Negative,
        &fixture_ids,
    )?);
    assert_unique_ids(&items)?;

    Ok(Corpus {
        repo_path,
        corpus_dir,
        manifest,
        items,
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn validate_fixture_files(corpus_dir: &Path, manifest: &CorpusManifest) -> Result<()> {
    for fixture in &manifest.fixtures {
        for (label, value) in [("schema", &fixture.schema), ("data", &fixture.data)] {
            if let Some(rel) = value {
                let abs = corpus_dir.join(rel);
                if !abs.exists() {
                    return Err(invalid(format!(
                        "fixture '{}' references missing {label} file: {rel}",
                        fixture.id
                    )));
                }
            }
        }
    }
    Ok(())
}

fn load_item_dir(
    corpus_dir: &Path,
    subdir: &str,
    category: ItemCategory,
    fixture_ids: &HashSet<String>,
) -> Result<Vec<CorpusItem>> {
    let dir = corpus_dir.join(subdir);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<PathBuf> = match fs::read_dir(&dir) {
        Ok(iter) => iter
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("json"))
            .collect(),
        Err(source) => {
            return Err(Error::Io {
                path: dir.clone(),
                source,
            });
        }
    };
    entries.sort();

    let mut items = Vec::new();
    for path in entries {
        let source_rel = format!(
            "{subdir}/{}",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("")
        );
        let payload: ItemFilePayload = read_json(&path)?;
        let raws: Vec<RawItem> = match payload {
            ItemFilePayload::Wrapped { items } => items,
            ItemFilePayload::Many(items) => items,
            ItemFilePayload::Single(item) => vec![*item],
        };
        for raw in raws {
            let item = CorpusItem::from_raw(raw, source_rel.clone(), category);
            validate_item(&item, fixture_ids)?;
            items.push(item);
        }
    }
    Ok(items)
}

fn assert_unique_ids(items: &[CorpusItem]) -> Result<()> {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for item in items {
        if let Some(prev) = seen.insert(item.id.as_str(), item.source.as_str()) {
            return Err(invalid(format!(
                "duplicate corpus item id '{}' in {} and {}",
                item.id, prev, item.source
            )));
        }
    }
    Ok(())
}
