import { execFileSync } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";

import { resolveReleaseVersion } from "./versions.mjs";

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
  if (nextVersion === currentVersion) {
    throw new Error(`Release version is unchanged: ${currentVersion}`);
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

  if (typeof nextPackageJson.upstream?.tag === "string") {
    nextPackageJson.upstream.tag = replaceVersionToken(nextPackageJson.upstream.tag, currentVersion, nextVersion);
  }

  return {
    currentVersion,
    nextVersion,
    nextPackageJson,
    renamePlan: dedupeRenames(renamePlan),
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

function assertCleanWorkingTree(repoPath) {
  const output = run("git", ["status", "--porcelain"], { cwd: repoPath, captureOutput: true }).trim();
  if (output) {
    throw new Error(`Target repo has uncommitted changes:\n${output}`);
  }
}

function assertReleaseScripts(packageJson) {
  const scripts = packageJson.scripts ?? {};
  for (const name of ["refresh:package-contract", "validate:bootstrap", "test:typedb-bootstrap"]) {
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

async function removePath(targetPath) {
  await fs.rm(targetPath, { recursive: true, force: true });
}

async function withValidationWorktree(repoPath, callback) {
  const workspaceRoot = path.dirname(repoPath);
  const tempRoot = await fs.mkdtemp(path.join(workspaceRoot, ".ontology-release-check-"));
  const worktreePath = path.join(tempRoot, path.basename(repoPath));

  try {
    run("git", ["worktree", "add", "--detach", worktreePath, "HEAD"], { cwd: repoPath });
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

function buildSummary({ repoPath, packageName, currentVersion, nextVersion, renamePlan, push }) {
  return {
    repoPath,
    packageName,
    currentVersion,
    nextVersion,
    tagName: `v${nextVersion}`,
    push,
    renamedPaths: renamePlan,
    scripts: ["refresh:package-contract", "validate:bootstrap", "test:typedb-bootstrap"],
  };
}

export async function executeRelease(options) {
  const repoPath = path.resolve(options.repo);
  const packageJsonPath = path.join(repoPath, "package.json");
  const packageJson = await readJson(packageJsonPath);
  assertReleaseScripts(packageJson);

  if (options.validateOnly) {
    const summary = {
      repoPath,
      packageName: packageJson.name,
      currentVersion: packageJson.version,
      nextVersion: null,
      tagName: null,
      push: false,
      renamedPaths: [],
      scripts: ["refresh:package-contract", "validate:bootstrap", "test:typedb-bootstrap"],
      mode: "validate-only",
    };

    await withValidationWorktree(repoPath, async (worktreePath) => {
      runPackageScript(worktreePath, "refresh:package-contract");
      runPackageScript(worktreePath, "validate:bootstrap");
      runPackageScript(worktreePath, "test:typedb-bootstrap");
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
  });
  summary.mode = options.dryRun ? "dry-run" : "release";

  if (options.dryRun) {
    return summary;
  }

  assertCleanWorkingTree(repoPath);
  assertTagDoesNotExist(repoPath, summary.tagName);

  await writeJson(packageJsonPath, plan.nextPackageJson);
  await applyRenamePlan(repoPath, plan.renamePlan);

  runPackageScript(repoPath, "refresh:package-contract");
  runPackageScript(repoPath, "validate:bootstrap");
  runPackageScript(repoPath, "test:typedb-bootstrap");

  run("git", ["add", "-A"], { cwd: repoPath });
  run("git", ["commit", "-m", `Release ${packageJson.name} v${plan.nextVersion}`], { cwd: repoPath });
  run("git", ["tag", summary.tagName], { cwd: repoPath });

  if (options.push) {
    run("git", ["push"], { cwd: repoPath });
    run("git", ["push", "origin", summary.tagName], { cwd: repoPath });
  }

  return summary;
}
