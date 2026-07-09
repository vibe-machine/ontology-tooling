//! Release CLI argument parsing and validation.
//!
//! Rust port of `src/lib/release-args.mjs`, so the CLI, CI (one-2xg.18), and
//! the app share one implementation (one-2xg.16 / one-2xg.46). Parsing,
//! first-error ordering, and error messages intentionally match JavaScript.
//!
//! This layer deliberately retains bump and version values as strings. The
//! JavaScript source validates only option combinations here; semantic version
//! parsing, [`crate::version::BumpKind`], and
//! [`crate::version::resolve_release_version`] belong to the later release
//! resolution step.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

/// Parsed options returned by `parseReleaseArgs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseArgs {
    pub repo: Option<String>,
    pub bump: Option<String>,
    pub version: Option<String>,
    pub dry_run: bool,
    pub validate_only: bool,
    pub push: bool,
    pub help: bool,
}

impl Default for ReleaseArgs {
    fn default() -> Self {
        Self {
            repo: None,
            bump: None,
            version: None,
            dry_run: false,
            validate_only: false,
            push: true,
            help: false,
        }
    }
}

/// Parses release CLI arguments, matching `parseReleaseArgs`.
pub fn parse_release_args<S: AsRef<str>>(argv: &[S]) -> Result<ReleaseArgs> {
    let mut options = ReleaseArgs::default();
    let mut index = 0;

    while index < argv.len() {
        let arg = argv[index].as_ref();

        match arg {
            "--help" | "-h" => options.help = true,
            "--dry-run" => options.dry_run = true,
            "--validate-only" => options.validate_only = true,
            "--no-push" => options.push = false,
            "--repo" => {
                options.repo = argv.get(index + 1).map(|value| value.as_ref().to_string());
                index += 1;
            }
            "--bump" => {
                options.bump = argv.get(index + 1).map(|value| value.as_ref().to_string());
                index += 1;
            }
            "--version" => {
                options.version = argv.get(index + 1).map(|value| value.as_ref().to_string());
                index += 1;
            }
            _ => return Err(invalid(format!("Unknown argument: {arg}"))),
        }

        index += 1;
    }

    Ok(options)
}

/// Validates parsed release arguments, matching `validateReleaseArgs`.
pub fn validate_release_args(options: &ReleaseArgs) -> Result<()> {
    if options.help {
        return Ok(());
    }

    if !js_truthy(&options.repo) {
        return Err(invalid("Missing required --repo argument"));
    }

    let has_bump = js_truthy(&options.bump);
    let has_version = js_truthy(&options.version);

    if has_bump && has_version {
        return Err(invalid("Use either --bump or --version, not both"));
    }

    if options.validate_only && (has_bump || has_version || options.dry_run) {
        return Err(invalid(
            "Use --validate-only by itself with --repo and optional --no-push",
        ));
    }

    if !options.validate_only && !has_bump && !has_version {
        return Err(invalid(
            "A release requires either --bump <patch|minor|major>, --version <x.y.z>, or --validate-only",
        ));
    }

    Ok(())
}

fn js_truthy(value: &Option<String>) -> bool {
    value.as_ref().is_some_and(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn error_message(error: Error) -> String {
        match error {
            Error::Version(message) => message,
            other => panic!("expected version error, got {other}"),
        }
    }

    #[test]
    fn parse_release_args_parses_dry_run_bump_flow() {
        let options = parse_release_args(&[
            "--repo",
            "../ontology-beads",
            "--bump",
            "patch",
            "--dry-run",
        ])
        .unwrap();

        assert_eq!(
            options,
            ReleaseArgs {
                repo: Some("../ontology-beads".to_string()),
                bump: Some("patch".to_string()),
                version: None,
                dry_run: true,
                validate_only: false,
                push: true,
                help: false,
            }
        );
        assert_eq!(
            serde_json::to_value(&options).unwrap(),
            json!({
                "repo": "../ontology-beads",
                "bump": "patch",
                "version": null,
                "dryRun": true,
                "validateOnly": false,
                "push": true,
                "help": false,
            })
        );
    }

    #[test]
    fn validate_release_args_rejects_missing_release_target() {
        let options = parse_release_args(&["--bump", "patch"]).unwrap();
        assert_eq!(
            error_message(validate_release_args(&options).unwrap_err()),
            "Missing required --repo argument"
        );
    }

    #[test]
    fn validate_release_args_rejects_bump_and_version_together() {
        let options =
            parse_release_args(&["--repo", ".", "--bump", "patch", "--version", "1.2.3"]).unwrap();
        assert_eq!(
            error_message(validate_release_args(&options).unwrap_err()),
            "Use either --bump or --version, not both"
        );
    }

    #[test]
    fn validate_release_args_accepts_validate_only_mode() {
        let options = parse_release_args(&["--repo", ".", "--validate-only"]).unwrap();
        validate_release_args(&options).unwrap();
    }

    #[test]
    fn validate_release_args_rejects_validate_only_mixed_with_release_args() {
        let options =
            parse_release_args(&["--repo", ".", "--validate-only", "--bump", "patch"]).unwrap();
        assert_eq!(
            error_message(validate_release_args(&options).unwrap_err()),
            "Use --validate-only by itself with --repo and optional --no-push"
        );
    }
}
