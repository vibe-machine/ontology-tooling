use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;
use serde::Serialize;
use serde_json::Value;
use vibe_ontology::competency_questions::{
    iso_now, questions_digest, read_declaration, run_competency_questions,
    validate_competency_questions, write_questions_doc, CompetencyDeclaration, LastRun,
    LastRunResult, QuestionOutcome, QuestionsFile, RunOutcome, LAST_RUN_PATH,
};
use vibe_ontology::stored_questions::write_generated;
use vibe_ontology::throwaway_typedb::ThrowawayTypeDb;

use super::{emit_json, Cli, Format};

#[derive(Args, Debug, Clone)]
pub struct CheckQuestionsArgs {
    /// Path to the ontology repository whose competency questions to run
    /// against its sample data (package.json `sampleData`).
    #[arg(long)]
    pub repo: PathBuf,

    /// Only (re)write the files generated from the questions file
    /// (docs/competency-questions.md, schema/stored-queries.tql and
    /// data/<package>-questions.tql); does not start TypeDB or run any query.
    #[arg(long = "write-generated", visible_alias = "write-doc")]
    pub write_generated: bool,

    /// TypeDB executable to start (defaults to TYPEDB_BIN, then `typedb` on PATH).
    #[arg(long)]
    pub typedb_bin: Option<PathBuf>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckQuestionsReport<'a> {
    repo_path: &'a Path,
    package_name: &'a str,
    version: &'a str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    waiver: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    written: Option<String>,
    results: Vec<ResultLine<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultLine<'a> {
    id: &'a str,
    number: u64,
    passed: bool,
    rows: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

pub async fn run(cli: &Cli, args: &CheckQuestionsArgs) -> Result<()> {
    let repo_path = std::fs::canonicalize(&args.repo)
        .with_context(|| format!("resolve repository path {}", args.repo.display()))?;
    let package = read_package(&repo_path)?;
    let package_name = string_field(&package, "name")?;
    let version = string_field(&package, "version")?;

    // --write-generated renders from a questions file that the generated files
    // may not yet match, so it skips those comparisons of the full static check.
    let declaration = if args.write_generated {
        read_declaration(&repo_path, &package)
    } else {
        validate_competency_questions(&repo_path, &package)
    }
    .context("check competency questions")?;

    let (questions_path, file) = match declaration {
        CompetencyDeclaration::Waived { reason } => {
            if !cli.quiet {
                eprintln!("notice: {package_name} waives competency questions: {reason}");
            }
            return emit(
                cli,
                &CheckQuestionsReport {
                    repo_path: &repo_path,
                    package_name,
                    version,
                    status: "waived",
                    waiver: Some(&reason),
                    written: None,
                    results: Vec::new(),
                },
            );
        }
        CompetencyDeclaration::Declared { path, file } => (path, file),
    };

    if args.write_generated {
        let mut written = vec![write_questions_doc(
            &repo_path,
            &package,
            &questions_path,
            &file,
        )?];
        written.extend(write_generated(
            &repo_path,
            &package,
            &questions_path,
            &file,
        )?);
        let written = written
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return emit(
            cli,
            &CheckQuestionsReport {
                repo_path: &repo_path,
                package_name,
                version,
                status: "written",
                waiver: None,
                written: Some(written),
                results: Vec::new(),
            },
        );
    }

    let (run, written) = run_and_record(
        &repo_path,
        &package,
        &questions_path,
        &file,
        args.typedb_bin.as_deref(),
        cli.quiet,
    )
    .await?;
    let outcomes = &run.outcomes;
    let failed = outcomes.iter().filter(|outcome| !outcome.passed()).count();

    emit(
        cli,
        &CheckQuestionsReport {
            repo_path: &repo_path,
            package_name,
            version,
            status: if run.passed() { "ok" } else { "failed" },
            waiver: None,
            written,
            results: outcomes
                .iter()
                .map(|outcome| ResultLine {
                    id: &outcome.id,
                    number: outcome.number,
                    passed: outcome.passed(),
                    rows: outcome.rows,
                    error: outcome.error.as_deref(),
                })
                .collect(),
        },
    )?;

    if failed > 0 {
        bail!(
            "{failed} of {} competency questions failed; {LAST_RUN_PATH} was not written",
            outcomes.len()
        );
    }
    if let Err(problem) = &run.installed {
        bail!("the installed questions are wrong: {problem}; {LAST_RUN_PATH} was not written");
    }
    Ok(())
}

/// Runs the questions against a throwaway TypeDB, prints one line per question
/// to stderr, and, when every question passed, writes `competency/last-run.json`
/// (returning its path). Shared by `ont check-questions` and `ont release`.
pub(crate) async fn run_and_record(
    repo_path: &Path,
    package: &Value,
    questions_path: &str,
    file: &QuestionsFile,
    typedb_bin: Option<&Path>,
    quiet: bool,
) -> Result<(RunOutcome, Option<String>)> {
    let package_name = string_field(package, "name")?;
    let version = string_field(package, "version")?;
    let mut server = ThrowawayTypeDb::start(typedb_bin)?;
    let target = server.target();
    let mut progress = |line: &str| {
        if !quiet {
            eprintln!("{line}");
        }
    };
    let run = run_competency_questions(
        &target,
        "competency_questions",
        repo_path,
        file,
        &mut progress,
    )
    .await;
    server.stop();
    let run = run?;

    for outcome in &run.outcomes {
        eprintln!("{}", result_line(outcome));
    }
    match &run.installed {
        Ok(count) => {
            eprintln!("installed: {count} StoredQuestion records, each with an answer query")
        }
        Err(problem) => eprintln!("installed: FAIL  {problem}"),
    }
    if !run.passed() {
        return Ok((run, None));
    }
    let last_run = LastRun {
        package: package_name.to_string(),
        version: version.to_string(),
        ran_at: iso_now(),
        questions_digest: questions_digest(repo_path, package, questions_path, file)?,
        results: run
            .outcomes
            .iter()
            .map(|outcome| LastRunResult {
                id: outcome.id.clone(),
                number: outcome.number,
                rows: outcome.rows,
                expected_empty: outcome.expected_empty,
            })
            .collect(),
    };
    let path = repo_path.join(LAST_RUN_PATH);
    let mut text = serde_json::to_string_pretty(&last_run)?;
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok((run, Some(path.display().to_string())))
}

fn result_line(outcome: &QuestionOutcome) -> String {
    let verdict = if outcome.passed() { "pass" } else { "FAIL" };
    let mut line = format!(
        "{:>3}  {verdict}  {:>5} rows  {}",
        outcome.number, outcome.rows, outcome.id
    );
    if let Some(failure) = outcome.failure() {
        line.push_str(&format!("  error: {failure}"));
    } else if outcome.expected_empty {
        line.push_str("  (expected empty)");
    }
    line
}

fn emit(cli: &Cli, report: &CheckQuestionsReport<'_>) -> Result<()> {
    match Format::resolve(cli.format) {
        Format::Json => emit_json(report),
        Format::Text => {
            println!(
                "check questions: {} v{}  [{}]",
                report.package_name, report.version, report.status
            );
            if let Some(written) = &report.written {
                println!("wrote:           {written}");
            }
            Ok(())
        }
    }
}

fn read_package(repo_path: &Path) -> Result<Value> {
    let path = repo_path.join("package.json");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn string_field<'a>(package: &'a Value, field: &str) -> Result<&'a str> {
    package
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("package.json is missing string field '{field}'"))
}
