import { executeRelease } from "../lib/package-release.mjs";
import { parseReleaseArgs, validateReleaseArgs } from "../lib/release-args.mjs";

function printHelp() {
  console.log(`ontology-release

Shared release command for ontology package repositories.

Usage:
  ontology-release --repo <path> --bump <patch|minor|major> [--dry-run] [--no-push]
  ontology-release --repo <path> --version <x.y.z> [--dry-run] [--no-push]
  ontology-release --repo <path> --validate-only

Examples:
  ontology-release --repo ../ontology-trace-to-knowledge --bump patch --dry-run
  ontology-release --repo ../ontology-beads --version 0.60.0 --no-push
  ontology-release --repo ../ontology-trace-to-knowledge --validate-only

Current status:
  The command supports dry-run release planning, validate-only gating, and end-to-end release mutation.
`);
}

function main() {
  const options = parseReleaseArgs(process.argv.slice(2));
  if (options.help || process.argv.slice(2).length === 0) {
    printHelp();
    return;
  }

  validateReleaseArgs(options);
  return executeRelease(options).then((summary) => {
    if (options.dryRun) {
      console.log("ontology-release dry run");
    } else {
      console.log("ontology-release completed");
    }
    console.log(JSON.stringify(summary, null, 2));
  });
}

try {
  await main();
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
