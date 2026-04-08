import path from "node:path";

import { validatePackageContract } from "../lib/package-validator.mjs";

function printHelp() {
  console.log(`ontology-validate-package

Validate an ontology package contract from the ontology-tooling boundary.

Usage:
  ontology-validate-package --repo <path>

Examples:
  ontology-validate-package --repo ../ontology-gist
  ontology-validate-package --repo ../ontology-beads
`);
}

function parseArgs(argv) {
  const options = { repo: null, help: false };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") {
      options.help = true;
      continue;
    }
    if (arg === "--repo") {
      options.repo = argv[index + 1] ?? null;
      index += 1;
      continue;
    }

    throw new Error(`Unknown argument: ${arg}`);
  }

  return options;
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help || process.argv.length <= 2) {
    printHelp();
    return;
  }

  if (!options.repo) {
    throw new Error("Missing required argument: --repo <path>");
  }

  const repoPath = path.resolve(options.repo);
  const packageJson = await validatePackageContract(repoPath);
  console.log(JSON.stringify({
    repoPath,
    packageName: packageJson.name,
    version: packageJson.version,
    status: "ok",
  }, null, 2));
}

try {
  await main();
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
