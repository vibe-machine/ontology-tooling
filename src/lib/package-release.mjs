import { execFileSync } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";

import { validateBootstrapUniqueness } from "./bootstrap-uniqueness.mjs";
import { validateMigrationContract } from "./migration-contract.mjs";
import { resolveReleaseVersion } from "./versions.mjs";

const REQUIRED_RELEASE_SCRIPTS = ["refresh:package-contract", "validate:bootstrap", "test:typedb-bootstrap"];
const OPTIONAL_RELEASE_SCRIPTS = ["test:typedb-migration"];

function run(command, args, { cwd, captureOutput = false } = {}) {
  return execFileSync(command, args, {
    cwd,
    encoding: "utf8",
    stdio: captureOutput ? ["ignore", "pipe", "pipe"] : "inherit",
  });
}

async function readJson(filePath) {
  return JSON.parse(await fs.readFile(filePath, "utf8"));
}

async function writeJson(filePath, value) {
  await fs.writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

async function shouldInstallNodeDependencies(repoPath) {
  const packageJson = await readJson(path.join(repoPath, "package.json"));
  const dependencyFields = ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"];
  return dependencyFields.some((field) => {
    const value = packageJson[field];
    return value && !Array.isArray(value) && typeof value === "object" && Object.keys(value).length > 0;
  });
}

function replaceVersionToken(value, currentVersion, nextVersion) {
  return typeof value === "string" ? value.split(currentVersion).join(nextVersion) : value;
}

function dedupeRenames(renamePlan) {
  const deduped = new Map();
  for (const rename of renamePlan) {
    deduped.set(`${rename.from}=>${rename.to}`, rename);
  }
  return [...deduped.values()];
}

export function planPackageRelease(packageJson, { bump, version }) {
  const currentVersion = packageJson.version;
  if (!currentVersion) {
    throw new Error("Target package.json is missing version");
  }

  const nextVersion = resolveReleaseVersion(currentVersion, { bump, version });
  const resumeExistingVersion = version !== null && nextVersion === currentVersion;
  if (nextVersion === currentVersion && !resumeExistingVersion) {
    throw new Error(`Release version is unchanged: ${currentVersion}`);
  }

  if (resumeExistingVersion) {
    return {
      currentVersion,
      nextVersion,
      nextPackageJson: structuredClone(packageJson),
      renamePlan: [],
      resumeExistingVersion: true,
    };
  }

  const nextPackageJson = structuredClone(packageJson);
  nextPackageJson.version = nextVersion;

  const renamePlan = [];

  const rewritePathArray = (container, key) => {
    if (!Array.isArray(container?.[key])) return;
    container[key] = container[key].map((entry) => {
      const rewritten = replaceVersionToken(entry, currentVersion, nextVersion);
      if (rewritten !== entry) {
        renamePlan.push({ from: entry, to: rewritten });
      }
      return rewritten;
    });
  };

  rewritePathArray(nextPackageJson, "manifests");

  if (typeof nextPackageJson.provenance?.manifest === "string") {
    const rewritten = replaceVersionToken(nextPackageJson.provenance.manifest, currentVersion, nextVersion);
    if (rewritten !== nextPackageJson.provenance.manifest) {
      renamePlan.push({ from: nextPackageJson.provenance.manifest, to: rewritten });
      nextPackageJson.provenance.manifest = rewritten;
    }
  }

  if (Array.isArray(nextPackageJson.assembly?.generatedArtifacts)) {
    nextPackageJson.assembly.generatedArtifacts = nextPackageJson.assembly.generatedArtifacts.map((entry) => {
      const rewritten = replaceVersionToken(entry, currentVersion, nextVersion);
      if (rewritten !== entry) {
        renamePlan.push({ from: entry, to: rewritten });
      }
      return rewritten;
    });
  }

  // Rewrite versioned paths inside scripts (e.g. --out manifests/pkg-v1.0.0.json)
  if (nextPackageJson.scripts) {
    for (const key of Object.keys(nextPackageJson.scripts)) {
      nextPackageJson.scripts[key] = replaceVersionToken(nextPackageJson.scripts[key], currentVersion, nextVersion);
    }
  }

  // NOTE: upstream.* is never rewritten — it is immutable provenance metadata
  // referring to the source repository, not the package version.

  return {
    currentVersion,
    nextVersion,
    nextPackageJson,
    renamePlan: dedupeRenames(renamePlan),
    resumeExistingVersion: false,
  };
}

async function pathExists(filePath) {
  try {
    await fs.access(filePath);
    return true;
  } catch {
    return false;
  }
}

async function applyRenamePlan(repoPath, renamePlan) {
  for (const rename of renamePlan) {
    const fromPath = path.join(repoPath, rename.from);
    const toPath = path.join(repoPath, rename.to);
    if (!(await pathExists(fromPath))) continue;
    if (await pathExists(toPath)) continue;
    await fs.mkdir(path.dirname(toPath), { recursive: true });
    await fs.rename(fromPath, toPath);
  }
}

async function linkSiblingOntologyRepos(repoPath, worktreePath) {
  const workspaceRoot = path.dirname(repoPath);
  const worktreeRoot = path.dirname(worktreePath);
  const repoName = path.basename(repoPath);
  const entries = await fs.readdir(workspaceRoot, { withFileTypes: true });

  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    if (!entry.name.startsWith("ontology-")) continue;
    if (entry.name === repoName) continue;

    const sourcePath = path.join(workspaceRoot, entry.name);
    const targetPath = path.join(worktreeRoot, entry.name);
    if (await pathExists(targetPath)) continue;
    await fs.symlink(sourcePath, targetPath, "dir");
  }
}

function assertCleanWorkingTree(repoPath) {
  const output = run("git", ["status", "--porcelain"], { cwd: repoPath, captureOutput: true }).trim();
  if (output) {
    throw new Error(`Target repo has uncommitted changes:\n${output}`);
  }
}

function assertReleaseScripts(packageJson) {
  const scripts = packageJson.scripts ?? {};
  for (const name of REQUIRED_RELEASE_SCRIPTS) {
    if (typeof scripts[name] !== "string" || scripts[name].trim().length === 0) {
      throw new Error(`Target repo is missing required script: ${name}`);
    }
  }
}

function assertTagDoesNotExist(repoPath, tagName) {
  const tags = run("git", ["tag", "--list", tagName], { cwd: repoPath, captureOutput: true }).trim();
  if (tags === tagName) {
    throw new Error(`Git tag already exists: ${tagName}`);
  }
}

function runPackageScript(repoPath, scriptName) {
  run("npm", ["run", scriptName], { cwd: repoPath });
}

function releaseScriptsToRun(packageJson) {
  const scripts = packageJson.scripts ?? {};
  return [
    ...REQUIRED_RELEASE_SCRIPTS,
    ...OPTIONAL_RELEASE_SCRIPTS.filter((name) => typeof scripts[name] === "string" && scripts[name].trim().length > 0),
  ];
}

function expectedReleaseCommitMessage(packageName, version) {
  return `Release ${packageName} v${version}`;
}

function assertHeadMatchesReleaseCommit(repoPath, packageName, version) {
  const subject = run("git", ["log", "-1", "--pretty=%s"], { cwd: repoPath, captureOutput: true }).trim();
  const expected = expectedReleaseCommitMessage(packageName, version);
  if (subject !== expected) {
    throw new Error(
      `Cannot resume release for v${version}: HEAD commit is "${subject}", expected "${expected}"`
    );
  }
}

async function runReleaseValidation(repoPath) {
  const packageJson = await readJson(path.join(repoPath, "package.json"));
  runPackageScript(repoPath, "refresh:package-contract");
  await validateBootstrapUniqueness(repoPath);
  await validateMigrationContract(repoPath);

  for (const scriptName of releaseScriptsToRun(packageJson)) {
    if (scriptName === "refresh:package-contract") continue;
    runPackageScript(repoPath, scriptName);
  }
}

function pushRelease(repoPath, tagName) {
  run("git", ["push", "--atomic", "origin", "HEAD", `refs/tags/${tagName}`], { cwd: repoPath });
}

async function removePath(targetPath) {
  await fs.rm(targetPath, { recursive: true, force: true });
}

async function withValidationWorktree(repoPath, callback) {
  const workspaceRoot = path.dirname(repoPath);
  const tempRoot = await fs.mkdtemp(path.join(workspaceRoot, ".ontology-release-check-"));
  const worktreePath = path.join(tempRoot, path.basename(repoPath));

  try {
    run("git", ["worktree", "add", "--detach", worktreePath, "HEAD"], { cwd: repoPath });
    await linkSiblingOntologyRepos(repoPath, worktreePath);
    if (await shouldInstallNodeDependencies(worktreePath)) {
      run("npm", ["install", "--no-audit", "--no-fund"], { cwd: worktreePath });
    }
    return await callback(worktreePath);
  } finally {
    try {
      run("git", ["worktree", "remove", "--force", worktreePath], { cwd: repoPath });
    } catch {
      await removePath(worktreePath);
    }
    await removePath(tempRoot);
  }
}

function buildSummary({ repoPath, packageName, currentVersion, nextVersion, renamePlan, push, scripts }) {
  return {
    repoPath,
    packageName,
    currentVersion,
    nextVersion,
    tagName: `v${nextVersion}`,
    push,
    resumeExistingVersion: false,
    renamedPaths: renamePlan,
    scripts,
  };
}

export async function executeRelease(options) {
  const repoPath = path.resolve(options.repo);
  const packageJsonPath = path.join(repoPath, "package.json");
  const packageJson = await readJson(packageJsonPath);
  assertReleaseScripts(packageJson);
  const scriptsToRun = releaseScriptsToRun(packageJson);

  if (options.validateOnly) {
    const summary = {
      repoPath,
      packageName: packageJson.name,
      currentVersion: packageJson.version,
      nextVersion: null,
      tagName: null,
      push: false,
      resumeExistingVersion: false,
      renamedPaths: [],
      scripts: scriptsToRun,
      mode: "validate-only",
    };

    await withValidationWorktree(repoPath, async (worktreePath) => {
      await runReleaseValidation(worktreePath);
    });
    return summary;
  }

  const plan = planPackageRelease(packageJson, options);
  const summary = buildSummary({
    repoPath,
    packageName: packageJson.name,
    currentVersion: plan.currentVersion,
    nextVersion: plan.nextVersion,
    renamePlan: plan.renamePlan,
    push: options.push,
    scripts: scriptsToRun,
  });
  summary.mode = options.dryRun ? "dry-run" : "release";
  summary.resumeExistingVersion = plan.resumeExistingVersion;

  if (options.dryRun) {
    return summary;
  }

  assertCleanWorkingTree(repoPath);
  assertTagDoesNotExist(repoPath, summary.tagName);

  if (plan.resumeExistingVersion) {
    assertHeadMatchesReleaseCommit(repoPath, packageJson.name, plan.nextVersion);
    await withValidationWorktree(repoPath, async (worktreePath) => {
      await runReleaseValidation(worktreePath);
    });
  } else {
    await writeJson(packageJsonPath, plan.nextPackageJson);
    await applyRenamePlan(repoPath, plan.renamePlan);

    await runReleaseValidation(repoPath);

    run("git", ["add", "-A"], { cwd: repoPath });
    run("git", ["commit", "-m", expectedReleaseCommitMessage(packageJson.name, plan.nextVersion)], { cwd: repoPath });
  }

  run("git", ["tag", summary.tagName], { cwd: repoPath });

  if (options.push) {
    pushRelease(repoPath, summary.tagName);
  }

  return summary;
}
