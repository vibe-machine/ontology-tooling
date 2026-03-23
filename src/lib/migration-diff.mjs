import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";

/**
 * Split a TypeQL data file into individual `put` statements.
 * Each statement starts with `put` and ends with `;`.
 * Comments and blank lines are stripped.
 */
function splitPutStatements(text) {
  const statements = [];
  let current = "";

  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (trimmed.startsWith("#") || trimmed === "") continue;

    if (trimmed.startsWith("put ") && current) {
      statements.push(current.trim());
      current = line;
    } else {
      current += (current ? "\n" : "") + line;
    }
  }
  if (current.trim()) statements.push(current.trim());
  return statements;
}

/**
 * Group consecutive put statements into logical blocks.
 * An entity put (`put $var isa Type`) followed by relation puts
 * that reference the same variable form one group.
 */
function groupPutStatements(statements) {
  const groups = [];
  let currentGroup = null;

  for (const stmt of statements) {
    const entityMatch = stmt.match(/^put\s+\$(\w+)\s+isa\s+(\w+)/);

    if (entityMatch) {
      if (currentGroup) groups.push(currentGroup);
      currentGroup = { variable: entityMatch[1], type: entityMatch[2], statements: [stmt] };
    } else if (currentGroup && stmt.includes(`$${currentGroup.variable}`)) {
      currentGroup.statements.push(stmt);
    } else {
      if (currentGroup) groups.push(currentGroup);
      currentGroup = { variable: null, type: null, statements: [stmt] };
    }
  }
  if (currentGroup) groups.push(currentGroup);
  return groups;
}

/**
 * Extract a stable key from a put-group for diffing.
 * Uses the entity type + first `has` attribute name + value.
 */
function extractGroupKey(group) {
  const firstStmt = group.statements[0];
  const typeMatch = firstStmt.match(/isa\s+(\w+)/);
  const hasMatch = firstStmt.match(/has\s+(\w+)\s+"([^"]+)"/);

  if (typeMatch && hasMatch) {
    return `${typeMatch[1]}::${hasMatch[1]}::${hasMatch[2]}`;
  }
  return `raw::${firstStmt.substring(0, 120)}`;
}

/**
 * Parse `has` clauses from an entity put statement.
 * Returns an array of { attribute, value } objects.
 */
function parseHasClauses(statement) {
  const clauses = [];
  const re = /has\s+(\w+)\s+("(?:[^"\\]|\\.)*"|\S+?)(?:\s+(@\w+(?:\([^)]*\))?))?(?=[,;]|\s*$)/g;
  for (const match of statement.matchAll(re)) {
    clauses.push({ attribute: match[1], value: match[2], annotation: match[3] || null });
  }
  return clauses;
}

/**
 * Generate a match/delete/insert statement to update changed attributes
 * on an existing entity identified by its key attribute.
 */
function renderEntityUpdate(variable, type, keyClauses, changedAttrs) {
  // One match/delete/insert pipeline per changed attribute.
  // Each pipeline is a separate query separated by blank lines.
  const pipelines = [];

  for (const change of changedAttrs) {
    const oldVar = `$${variable}_old_${change.attribute}`;
    const lines = [];

    lines.push("match");
    lines.push(`  $${variable} isa ${type},`);
    const keyParts = keyClauses.map((c) => `    has ${c.attribute} ${c.value}`);
    lines.push(keyParts.join(",\n") + ",");
    // Bind old value to a variable so delete can reference it
    lines.push(`    has ${change.attribute} ${oldVar};`);

    if (change.oldValues.length > 0) {
      lines.push("delete");
      lines.push(`  has ${oldVar} of $${variable};`);
    }
    if (change.newValues.length > 0) {
      lines.push("insert");
      for (const newVal of change.newValues) {
        lines.push(`  $${variable} has ${change.attribute} ${newVal};`);
      }
    }

    pipelines.push(lines.join("\n"));
  }

  return pipelines.join("\n\n");
}

/**
 * Compare old and new entity put statements and produce an update statement
 * if attributes changed. Returns null if only relation puts changed.
 */
function diffEntityGroup(oldGroup, newGroup) {
  const oldEntity = oldGroup.statements[0];
  const newEntity = newGroup.statements[0];

  const oldClauses = parseHasClauses(oldEntity);
  const newClauses = parseHasClauses(newEntity);

  if (oldClauses.length === 0 || newClauses.length === 0) return null;

  // Key clause = first has attribute (matches extractGroupKey logic)
  const keyAttr = newClauses[0].attribute;
  const keyClauses = newClauses.filter((c) => c.attribute === keyAttr);

  // Build attr → [values] maps for non-key attributes
  const buildAttrMap = (clauses) => {
    const map = new Map();
    for (const c of clauses) {
      if (c.attribute === keyAttr) continue;
      if (!map.has(c.attribute)) map.set(c.attribute, []);
      map.get(c.attribute).push(c.value);
    }
    return map;
  };

  const oldAttrs = buildAttrMap(oldClauses);
  const newAttrs = buildAttrMap(newClauses);

  const changedAttrs = [];
  for (const [attr, newValues] of newAttrs) {
    const oldValues = oldAttrs.get(attr) || [];
    if (JSON.stringify(oldValues) !== JSON.stringify(newValues)) {
      changedAttrs.push({ attribute: attr, oldValues, newValues });
    }
  }
  for (const [attr, oldValues] of oldAttrs) {
    if (!newAttrs.has(attr)) {
      changedAttrs.push({ attribute: attr, oldValues, newValues: [] });
    }
  }

  if (changedAttrs.length === 0) return null;

  return renderEntityUpdate(newGroup.variable, newGroup.type, keyClauses, changedAttrs);
}

/**
 * Find groups from allGroups whose variables are referenced by changedGroups
 * but not defined within changedGroups (preamble dependencies).
 */
function resolvePreambles(changedGroups, allGroups) {
  const definedVars = new Set(changedGroups.filter((g) => g.variable).map((g) => g.variable));
  const neededVars = new Set();

  for (const group of changedGroups) {
    const text = group.statements.join("\n");
    for (const match of text.matchAll(/\$(\w+)/g)) {
      if (!definedVars.has(match[1])) neededVars.add(match[1]);
    }
  }

  return allGroups.filter((g) => g.variable && neededVars.has(g.variable) && !definedVars.has(g.variable));
}

function getFileAtTag(repoPath, tag, relativePath) {
  try {
    return execFileSync("git", ["show", `${tag}:${relativePath}`], {
      cwd: repoPath,
      encoding: "utf8",
    });
  } catch {
    return "";
  }
}

/**
 * Generate a migration diff between two versions of a package.
 * Compares data files from the previous git tag against the current working tree.
 *
 * - New entities use `put` statements.
 * - Changed entities use `match`/`delete`/`insert` to update only the changed attributes.
 *
 * Returns the relative path to the migration file, or null if no data changed.
 */
export async function generateMigrationDiff(repoPath, fromVersion, toVersion) {
  const packageJsonPath = path.join(repoPath, "package.json");
  const packageJson = JSON.parse(await fs.readFile(packageJsonPath, "utf8"));
  const dataFiles = packageJson.data ?? [];

  if (dataFiles.length === 0) return null;

  const fromTag = `v${fromVersion}`;
  const allNewGroups = [];
  const newGroups = [];
  const updateStatements = [];

  for (const dataFile of dataFiles) {
    const oldText = getFileAtTag(repoPath, fromTag, dataFile);
    const newText = await fs.readFile(path.join(repoPath, dataFile), "utf8");

    const oldGroups = groupPutStatements(splitPutStatements(oldText));
    const newGroups_ = groupPutStatements(splitPutStatements(newText));
    allNewGroups.push(...newGroups_);

    const oldMap = new Map();
    for (const g of oldGroups) oldMap.set(extractGroupKey(g), g);

    for (const g of newGroups_) {
      const key = extractGroupKey(g);
      const oldGroup = oldMap.get(key);

      if (!oldGroup) {
        // New entity — use put
        newGroups.push(g);
      } else if (oldGroup.statements.join("\n") !== g.statements.join("\n")) {
        // Changed entity — generate match/delete/insert for attribute diffs
        const updateStmt = diffEntityGroup(oldGroup, g);
        if (updateStmt) {
          updateStatements.push(updateStmt);
        } else {
          // Only relation puts changed — use put (safe for relations)
          newGroups.push(g);
        }
      }
    }
  }

  if (newGroups.length === 0 && updateStatements.length === 0) return null;

  // Resolve preambles for new groups (put statements need shared variables)
  const preambles = newGroups.length > 0 ? resolvePreambles(newGroups, allNewGroups) : [];
  const putGroups = [...preambles, ...newGroups];

  // Build migration file
  const migrationRelPath = `migrations/v${fromVersion}-to-v${toVersion}.tql`;
  const migrationAbsPath = path.join(repoPath, migrationRelPath);
  await fs.mkdir(path.dirname(migrationAbsPath), { recursive: true });

  const manifestRelPath = (packageJson.manifests ?? []).find((m) => m.endsWith(".package-manifest.json"));

  const header = [
    `# Migration: ${packageJson.name} v${fromVersion} \u2192 v${toVersion}`,
    `# Generated by ontology-release`,
    `# Apply in a write transaction against an existing database`,
    ...(manifestRelPath ? [`# manifest: ${manifestRelPath}`] : []),
    "",
  ].join("\n");

  const sections = [];
  if (putGroups.length > 0) {
    sections.push(putGroups.map((g) => g.statements.join("\n")).join("\n\n"));
  }
  if (updateStatements.length > 0) {
    sections.push(updateStatements.join("\n\n"));
  }

  const migrationContent = `${header}\n${sections.join("\n\n")}\n`;
  await fs.writeFile(migrationAbsPath, migrationContent);

  // Append migration file hash to the package manifest
  if (manifestRelPath) {
    const manifestPath = path.join(repoPath, manifestRelPath);
    const manifest = JSON.parse(await fs.readFile(manifestPath, "utf8"));
    const sha256 = crypto.createHash("sha256").update(migrationContent).digest("hex");
    if (!manifest.artifacts) manifest.artifacts = [];
    manifest.artifacts.push({ kind: "migration", path: migrationRelPath, sha256 });
    await fs.writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  }

  return migrationRelPath;
}

export const testing = {
  diffEntityGroup,
  extractGroupKey,
  groupPutStatements,
  parseHasClauses,
  resolvePreambles,
  splitPutStatements,
};
