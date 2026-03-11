import fs from "node:fs/promises";
import path from "node:path";

function parseSemver(value) {
  if (typeof value !== "string") {
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

function compareSemver(lhs, rhs) {
  if (lhs.major !== rhs.major) return lhs.major - rhs.major;
  if (lhs.minor !== rhs.minor) return lhs.minor - rhs.minor;
  return lhs.patch - rhs.patch;
}

function validateSemverRange(range) {
  if (typeof range !== "string" || range.trim().length === 0) {
    throw new Error("migration range must be a non-empty string");
  }

  const clauses = range.trim().split(/\s+/);
  for (const clause of clauses) {
    if (clause.endsWith(".x")) {
      parseSemver(clause.replace(/\.x$/, ".0"));
      continue;
    }

    if (/^(>=|<=|>|<)/.test(clause)) {
      parseSemver(clause.replace(/^(>=|<=|>|<)/, ""));
      continue;
    }

    parseSemver(clause);
  }
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

async function assertPathExists(repoPath, relativePath, context) {
  const absolutePath = path.join(repoPath, relativePath);
  try {
    await fs.access(absolutePath);
  } catch {
    throw new Error(`${context} references missing file: ${relativePath}`);
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

function validatePlanShape(packageJson, plan) {
  if (!plan || typeof plan !== "object") {
    throw new Error("migration plan must be an object");
  }

  if (typeof plan.id !== "string" || plan.id.trim().length === 0) {
    throw new Error("migration plan id must be a non-empty string");
  }

  validateSemverRange(plan.from);

  const target = parseSemver(plan.to);
  const packageVersion = parseSemver(packageJson.version);
  if (compareSemver(target, packageVersion) !== 0) {
    throw new Error(`migration plan '${plan.id}' targets ${plan.to}, expected package version ${packageJson.version}`);
  }

  if (!["replace", "compatible"].includes(plan.mode)) {
    throw new Error(`migration plan '${plan.id}' has unsupported mode '${plan.mode}'`);
  }

  if (plan.mode === "replace" && plan.snapshot?.required !== true) {
    throw new Error(`migration plan '${plan.id}' uses replace mode and must require a snapshot`);
  }

  if (!Array.isArray(plan.phases) || plan.phases.length === 0) {
    throw new Error(`migration plan '${plan.id}' must define at least one phase`);
  }

  assertUnique(plan.phases.map((phase) => phase.id), `migration plan '${plan.id}' phases`);
}

function validateUnitShape(plan, phase, unit) {
  if (!unit || typeof unit !== "object") {
    throw new Error(`migration plan '${plan.id}' phase '${phase.id}' has a non-object unit`);
  }

  const validKinds = new Set(["schema", "write", "assert-schema", "assert-data"]);
  if (!validKinds.has(unit.kind)) {
    throw new Error(`migration plan '${plan.id}' phase '${phase.id}' has unsupported unit kind '${unit.kind}'`);
  }

  if (typeof unit.path !== "string" || unit.path.trim().length === 0) {
    throw new Error(`migration plan '${plan.id}' phase '${phase.id}' has a unit with empty path`);
  }
}

export async function validateMigrationContract(repoPath) {
  const packageJson = JSON.parse(await fs.readFile(path.join(repoPath, "package.json"), "utf8"));
  const migration = packageJson.migration;

  if (!migration) {
    return;
  }

  if (migration.format !== 1) {
    throw new Error(`migration.format must be 1, got ${migration.format}`);
  }

  if (!Array.isArray(migration.plans) || migration.plans.length === 0) {
    throw new Error("migration.plans must contain at least one plan");
  }

  assertUnique(migration.plans.map((plan) => plan.id), "migration plan ids");
  const declaredBootstrapAssets = packageAssetPaths(packageJson);

  for (const plan of migration.plans) {
    validatePlanShape(packageJson, plan);

    let sawVerifyPhase = false;
    for (const phase of plan.phases) {
      if (typeof phase.id !== "string" || phase.id.trim().length === 0) {
        throw new Error(`migration plan '${plan.id}' contains a phase with an empty id`);
      }
      if (!Array.isArray(phase.units) || phase.units.length === 0) {
        throw new Error(`migration plan '${plan.id}' phase '${phase.id}' must contain at least one unit`);
      }

      const phaseUnitPaths = [];
      for (const unit of phase.units) {
        validateUnitShape(plan, phase, unit);
        phaseUnitPaths.push(`${unit.kind}:${unit.path}`);
        await assertPathExists(repoPath, unit.path, `migration plan '${plan.id}' phase '${phase.id}'`);
      }
      assertUnique(phaseUnitPaths, `migration plan '${plan.id}' phase '${phase.id}' units`);

      if (phase.id === "verify" || phase.units.some((unit) => unit.kind.startsWith("assert-"))) {
        sawVerifyPhase = true;
      }
    }

    if (plan.mode === "replace" && !sawVerifyPhase) {
      throw new Error(`migration plan '${plan.id}' uses replace mode and must include a verify/assert phase`);
    }

    for (const bootstrapAsset of declaredBootstrapAssets) {
      if (bootstrapAsset.startsWith("migrations/")) {
        throw new Error(`bootstrap assets must not include migration-scoped files: ${bootstrapAsset}`);
      }
    }
  }
}

export const testing = {
  compareSemver,
  packageAssetPaths,
  parseSemver,
  validatePlanShape,
  validateSemverRange,
};
