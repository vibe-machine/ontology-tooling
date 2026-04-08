import fs from "node:fs/promises";
import path from "node:path";

import { validateMigrationContract } from "./migration-contract.mjs";

function isNonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function parseSemver(value) {
  if (!isNonEmptyString(value)) {
    throw new Error(`invalid semver value: ${value}`);
  }

  const match = /^v?(\d+)\.(\d+)\.(\d+)$/.exec(value.trim());
  if (!match) {
    throw new Error(`invalid semver value: ${value}`);
  }

  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
  };
}

function packageAssetPaths(packageJson) {
  const schemas = Array.isArray(packageJson.schemas) ? packageJson.schemas.map((entry) => entry.file) : [];
  const data = Array.isArray(packageJson.data) ? packageJson.data : [];
  const manifests = Array.isArray(packageJson.manifests) ? packageJson.manifests : [];

  let provenance = [];
  if (Array.isArray(packageJson.provenance)) {
    provenance = packageJson.provenance;
  } else if (packageJson.provenance && typeof packageJson.provenance === "object") {
    if (Array.isArray(packageJson.provenance.files)) {
      provenance = packageJson.provenance.files;
    } else if (typeof packageJson.provenance.manifest === "string") {
      provenance = [packageJson.provenance.manifest];
    }
  }

  return new Set([...schemas, ...data, ...manifests, ...provenance]);
}

function requiredReleaseScripts(packageJson) {
  const required = ["refresh:package-contract", "validate:bootstrap", "test:typedb-bootstrap"];
  if (packageJson.migration) {
    required.push("test:typedb-migration");
  }
  return required;
}

async function readJson(filePath) {
  return JSON.parse(await fs.readFile(filePath, "utf8"));
}

async function assertPathExists(repoPath, relativePath, context) {
  const absolutePath = path.join(repoPath, relativePath);
  try {
    await fs.access(absolutePath);
  } catch {
    throw new Error(`${context} not found: ${relativePath}`);
  }
}

function assertUnique(values, context) {
  const seen = new Set();
  for (const value of values) {
    if (seen.has(value)) {
      throw new Error(`${context} contains duplicate value: ${value}`);
    }
    seen.add(value);
  }
}

function validateRequiredFields(packageJson) {
  if (!isNonEmptyString(packageJson.name)) {
    throw new Error("package.json 'name' must be a non-empty string");
  }
  if (!isNonEmptyString(packageJson.displayName)) {
    throw new Error("package.json 'displayName' must be a non-empty string");
  }
  parseSemver(packageJson.version);
  if (!Array.isArray(packageJson.schemas) || packageJson.schemas.length === 0) {
    throw new Error("package.json 'schemas' must contain at least one schema entry");
  }
}

async function validateSchemaEntries(repoPath, packageJson) {
  for (const schema of packageJson.schemas) {
    if (!schema || typeof schema !== "object") {
      throw new Error("schema entries must be objects");
    }
    if (!isNonEmptyString(schema.name)) {
      throw new Error("schema entry name must be a non-empty string");
    }
    if (!isNonEmptyString(schema.file)) {
      throw new Error(`schema '${schema.name}' file must be a non-empty string`);
    }
    await assertPathExists(repoPath, schema.file, `schema file`);
  }
}

async function validateFileList(repoPath, paths, context) {
  for (const relativePath of paths ?? []) {
    if (!isNonEmptyString(relativePath)) {
      throw new Error(`${context} contains an empty path`);
    }
    await assertPathExists(repoPath, relativePath, context);
  }
}

async function validateAssembly(repoPath, packageJson) {
  const assembly = packageJson.assembly;
  if (!assembly) return;

  if (!Array.isArray(assembly.loadOrder) || assembly.loadOrder.length === 0) {
    throw new Error("assembly.loadOrder must contain at least one asset");
  }

  assertUnique(assembly.loadOrder, "assembly.loadOrder");

  const declaredAssets = packageAssetPaths(packageJson);
  for (const relativePath of assembly.loadOrder) {
    if (!declaredAssets.has(relativePath)) {
      throw new Error(`assembly.loadOrder references undeclared asset: ${relativePath}`);
    }
    await assertPathExists(repoPath, relativePath, "assembly asset");
  }
}

function validateReleaseScripts(packageJson) {
  const scripts = packageJson.scripts ?? {};
  for (const scriptName of requiredReleaseScripts(packageJson)) {
    if (!isNonEmptyString(scripts[scriptName])) {
      throw new Error(`package.json scripts must define '${scriptName}'`);
    }
  }
}

export async function validatePackageContract(repoPath) {
  const packageJsonPath = path.join(repoPath, "package.json");
  const packageJson = await readJson(packageJsonPath);

  validateRequiredFields(packageJson);
  validateReleaseScripts(packageJson);
  await validateSchemaEntries(repoPath, packageJson);
  await validateFileList(repoPath, packageJson.data ?? [], "data file");
  await validateFileList(repoPath, packageJson.manifests ?? [], "manifest file");

  if (Array.isArray(packageJson.provenance)) {
    await validateFileList(repoPath, packageJson.provenance, "provenance file");
  } else if (packageJson.provenance && typeof packageJson.provenance === "object") {
    if (Array.isArray(packageJson.provenance.files)) {
      await validateFileList(repoPath, packageJson.provenance.files, "provenance file");
    } else if (isNonEmptyString(packageJson.provenance.manifest)) {
      await validateFileList(repoPath, [packageJson.provenance.manifest], "provenance file");
    }
  }

  if (isNonEmptyString(packageJson.domains)) {
    await assertPathExists(repoPath, packageJson.domains, "domains file");
  }
  if (isNonEmptyString(packageJson.categorization)) {
    await assertPathExists(repoPath, packageJson.categorization, "categorization file");
  }

  await validateAssembly(repoPath, packageJson);
  await validateMigrationContract(repoPath);

  return packageJson;
}

export const testing = {
  packageAssetPaths,
  parseSemver,
  requiredReleaseScripts,
};
