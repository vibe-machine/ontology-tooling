//! Semantic versioning: parsing, bumping, release resolution, and range
//! satisfaction.
//!
//! This is the Rust port of the scattered JS semver logic (`src/lib/versions.mjs`
//! plus the range/constraint matching in `src/lib/migration-contract.mjs`), so
//! the CLI, CI, and the app share one implementation (one-2xg.16 / one-2xg.46).
//! Behavior is intentionally faithful to the JS:
//! - versions are strict `MAJOR.MINOR.PATCH` (`parse`); range clauses also
//!   accept a leading `v` (matching the contract's `v?` regex),
//! - a range is space-separated clauses combined with AND, each an exact
//!   version, a `>=`/`<=`/`>`/`<` comparator, or a `MAJOR.MINOR.x` wildcard.

use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

/// A strict `MAJOR.MINOR.PATCH` semantic version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemanticVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl SemanticVersion {
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses a strict `MAJOR.MINOR.PATCH` string (no `v` prefix), matching
    /// `versions.mjs` `parseSemver`.
    pub fn parse(version: &str) -> Result<Self> {
        parse_triplet(version, false)
            .ok_or_else(|| Error::Version(format!("Unsupported version format: {version}")))
    }

    /// Applies a `major` / `minor` / `patch` bump, matching `bumpVersion`.
    pub fn bump(&self, kind: BumpKind) -> Self {
        match kind {
            BumpKind::Major => Self::new(self.major + 1, 0, 0),
            BumpKind::Minor => Self::new(self.major, self.minor + 1, 0),
            BumpKind::Patch => Self::new(self.major, self.minor, self.patch + 1),
        }
    }
}

impl fmt::Display for SemanticVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for SemanticVersion {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

/// Which component a release bump advances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BumpKind {
    Major,
    Minor,
    Patch,
}

impl FromStr for BumpKind {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "major" => Ok(BumpKind::Major),
            "minor" => Ok(BumpKind::Minor),
            "patch" => Ok(BumpKind::Patch),
            other => Err(Error::Version(format!("Unsupported bump kind: {other}"))),
        }
    }
}

/// Resolves the next release version: an explicit version (validated) wins,
/// otherwise `current` is bumped. Mirrors `resolveReleaseVersion`.
pub fn resolve_release_version(
    current: &str,
    explicit: Option<&str>,
    bump: Option<BumpKind>,
) -> Result<String> {
    if let Some(version) = explicit {
        // Validate the explicit version, then return it verbatim.
        SemanticVersion::parse(version)?;
        return Ok(version.to_string());
    }
    let bump = bump.ok_or_else(|| Error::Version("Unsupported bump kind: none".to_string()))?;
    Ok(SemanticVersion::parse(current)?.bump(bump).to_string())
}

/// A dependency/migration version range: space-separated clauses combined with
/// AND. Mirrors the contract's `validateSemverRange` / `semverSatisfiesRange`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRange {
    clauses: Vec<Clause>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Clause {
    /// `MAJOR.MINOR.x` — matches any patch of that major.minor.
    Wildcard {
        major: u64,
        minor: u64,
    },
    GreaterOrEqual(SemanticVersion),
    LessOrEqual(SemanticVersion),
    Greater(SemanticVersion),
    Less(SemanticVersion),
    Exact(SemanticVersion),
}

impl VersionRange {
    /// Parses (and validates) a range. Errors on any malformed clause, matching
    /// `validateSemverRange`.
    pub fn parse(range: &str) -> Result<Self> {
        let trimmed = range.trim();
        if trimmed.is_empty() {
            return Err(Error::Version(
                "migration range must be a non-empty string".to_string(),
            ));
        }

        let mut clauses = Vec::new();
        for token in trimmed.split_whitespace() {
            clauses.push(parse_clause(token)?);
        }
        Ok(Self { clauses })
    }

    /// True when `version` satisfies EVERY clause.
    pub fn satisfies(&self, version: &SemanticVersion) -> bool {
        self.clauses.iter().all(|clause| clause.matches(version))
    }
}

impl Clause {
    fn matches(&self, version: &SemanticVersion) -> bool {
        match self {
            Clause::Wildcard { major, minor } => version.major == *major && version.minor == *minor,
            Clause::GreaterOrEqual(v) => version >= v,
            Clause::LessOrEqual(v) => version <= v,
            Clause::Greater(v) => version > v,
            Clause::Less(v) => version < v,
            Clause::Exact(v) => version == v,
        }
    }
}

fn parse_clause(token: &str) -> Result<Clause> {
    if let Some(prefix) = token.strip_suffix(".x") {
        // `2.0.x` -> validate as `2.0.0`, keep major.minor.
        let base = parse_range_semver(&format!("{prefix}.0"))?;
        return Ok(Clause::Wildcard {
            major: base.major,
            minor: base.minor,
        });
    }
    if let Some(rest) = token.strip_prefix(">=") {
        return Ok(Clause::GreaterOrEqual(parse_range_semver(rest)?));
    }
    if let Some(rest) = token.strip_prefix("<=") {
        return Ok(Clause::LessOrEqual(parse_range_semver(rest)?));
    }
    if let Some(rest) = token.strip_prefix('>') {
        return Ok(Clause::Greater(parse_range_semver(rest)?));
    }
    if let Some(rest) = token.strip_prefix('<') {
        return Ok(Clause::Less(parse_range_semver(rest)?));
    }
    Ok(Clause::Exact(parse_range_semver(token)?))
}

/// Range clauses accept a leading `v` (the contract's `v?` regex).
fn parse_range_semver(value: &str) -> Result<SemanticVersion> {
    parse_triplet(value, true)
        .ok_or_else(|| Error::Version(format!("invalid semver value: {value}")))
}

fn parse_triplet(value: &str, allow_v_prefix: bool) -> Option<SemanticVersion> {
    let trimmed = value.trim();
    let core = if allow_v_prefix {
        trimmed.strip_prefix('v').unwrap_or(trimmed)
    } else {
        trimmed
    };
    let mut parts = core.split('.');
    let major = parse_component(parts.next()?)?;
    let minor = parse_component(parts.next()?)?;
    let patch = parse_component(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some(SemanticVersion::new(major, minor, patch))
}

fn parse_component(component: &str) -> Option<u64> {
    // Match the JS `\d+` — digits only (no sign, no leading `+`), non-empty.
    if component.is_empty() || !component.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    component.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_triplet_and_rejects_garbage() {
        assert_eq!(
            SemanticVersion::parse("2.0.4").unwrap(),
            SemanticVersion::new(2, 0, 4)
        );
        assert!(SemanticVersion::parse("v2.0.4").is_err()); // strict: no v prefix
        assert!(SemanticVersion::parse("2.0").is_err());
        assert!(SemanticVersion::parse("2.0.4.1").is_err());
        assert!(SemanticVersion::parse("2.0.x").is_err());
        assert!(SemanticVersion::parse("2.0.-1").is_err());
    }

    #[test]
    fn bumps_match_js() {
        let v = SemanticVersion::parse("2.3.4").unwrap();
        assert_eq!(v.bump(BumpKind::Major).to_string(), "3.0.0");
        assert_eq!(v.bump(BumpKind::Minor).to_string(), "2.4.0");
        assert_eq!(v.bump(BumpKind::Patch).to_string(), "2.3.5");
    }

    #[test]
    fn resolve_prefers_explicit_then_bumps() {
        assert_eq!(
            resolve_release_version("2.0.4", Some("5.1.0"), None).unwrap(),
            "5.1.0"
        );
        assert_eq!(
            resolve_release_version("2.0.4", None, Some(BumpKind::Major)).unwrap(),
            "3.0.0"
        );
        assert!(resolve_release_version("2.0.4", Some("nope"), None).is_err());
    }

    #[test]
    fn ordering_is_component_wise() {
        assert!(
            SemanticVersion::parse("2.0.4").unwrap() < SemanticVersion::parse("3.0.0").unwrap()
        );
        assert!(
            SemanticVersion::parse("2.1.0").unwrap() > SemanticVersion::parse("2.0.9").unwrap()
        );
    }

    #[test]
    fn range_wildcard_matches_major_minor() {
        let range = VersionRange::parse("2.0.x").unwrap();
        assert!(range.satisfies(&SemanticVersion::new(2, 0, 4)));
        assert!(range.satisfies(&SemanticVersion::new(2, 0, 0)));
        assert!(!range.satisfies(&SemanticVersion::new(2, 1, 0)));
        assert!(!range.satisfies(&SemanticVersion::new(3, 0, 0)));
    }

    #[test]
    fn range_comparators_and_v_prefix() {
        let range = VersionRange::parse(">=v2.0.0 <3.0.0").unwrap();
        assert!(range.satisfies(&SemanticVersion::new(2, 5, 1)));
        assert!(range.satisfies(&SemanticVersion::new(2, 0, 0)));
        assert!(!range.satisfies(&SemanticVersion::new(3, 0, 0)));
        assert!(!range.satisfies(&SemanticVersion::new(1, 9, 9)));
    }

    #[test]
    fn range_exact_and_conjunction() {
        assert!(VersionRange::parse("2.0.4")
            .unwrap()
            .satisfies(&SemanticVersion::new(2, 0, 4)));
        assert!(!VersionRange::parse("2.0.4")
            .unwrap()
            .satisfies(&SemanticVersion::new(2, 0, 5)));
        // AND of clauses: >2.0.0 AND <=2.5.0
        let range = VersionRange::parse(">2.0.0 <=2.5.0").unwrap();
        assert!(range.satisfies(&SemanticVersion::new(2, 5, 0)));
        assert!(!range.satisfies(&SemanticVersion::new(2, 0, 0)));
    }

    #[test]
    fn empty_range_is_rejected() {
        assert!(VersionRange::parse("   ").is_err());
        assert!(VersionRange::parse("2.0.x garbage").is_err());
    }
}
