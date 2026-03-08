import { parseReleaseArgs, validateReleaseArgs } from "../lib/release-args.mjs";

function printHelp() {
  console.log(`ontology-release

Shared release command for ontology package repositories.

Usage:
  ontology-release --repo <path> --bump <patch|minor|major> [--dry-run] [--no-push]
  ontology-release --repo <path> --version <x.y.z> [--dry-run] [--no-push]

Examples:
  ontology-release --repo ../ontology-trace-to-knowledge --bump patch --dry-run
  ontology-release --repo ../ontology-beads --version 0.60.0 --no-push

Current status:
  This is the foundation scaffold. End-to-end release mutation is not implemented yet.
`);
}

function main() {
  const options = parseReleaseArgs(process.argv.slice(2));
  if (options.help || process.argv.slice(2).length === 0) {
    printHelp();
    return;
  }

  validateReleaseArgs(options);

  if (options.dryRun) {
    console.log("ontology-release dry run");
    console.log(JSON.stringify(options, null, 2));
    return;
  }

  throw new Error(
    "ontology-release is scaffolded but not implemented yet. Use --dry-run or --help until gist-5y8.2 is complete."
  );
}

try {
  main();
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
