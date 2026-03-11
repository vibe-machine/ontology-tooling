import fs from "node:fs/promises";
import path from "node:path";

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

async function readJson(filePath) {
  return JSON.parse(await fs.readFile(filePath, "utf8"));
}

function extractKeyedEntitiesFromSchema(tql) {
  const keyedEntities = {};
  const entityPattern = /^entity\s+(\w+)(?:\s+sub\s+\w+)?\s*,?\s*\n([\s\S]*?);/gm;
  const keyAttrPattern = /owns\s+(\w+)\s+@key/g;

  for (const match of tql.matchAll(entityPattern)) {
    const entityType = match[1];
    const body = match[2];
    const keyAttrs = [...body.matchAll(keyAttrPattern)].map((keyMatch) => keyMatch[1]);
    if (keyAttrs.length > 0) {
      keyedEntities[entityType] = keyAttrs;
    }
  }

  return keyedEntities;
}

function extractInsertKeyValues(tql, entityType, keyAttr) {
  const values = [];
  const insertPattern = new RegExp(
    String.raw`insert\s+\$\w+\s+isa\s+${escapeRegExp(entityType)}\s*,([\s\S]*?);`,
    "g"
  );
  const valuePattern = new RegExp(String.raw`has\s+${escapeRegExp(keyAttr)}\s+"([^"]*)"`);

  for (const match of tql.matchAll(insertPattern)) {
    const body = match[1];
    const valueMatch = valuePattern.exec(body);
    if (valueMatch) {
      values.push(valueMatch[1]);
    }
  }

  return values;
}

function findInsertStatementsForKeyedEntities(tql, keyedEntities) {
  const violations = [];

  for (const [entityType, keyAttrs] of Object.entries(keyedEntities)) {
    for (const keyAttr of keyAttrs) {
      const values = extractInsertKeyValues(tql, entityType, keyAttr);
      for (const value of values) {
        violations.push({ entityType, keyAttr, value });
      }
    }
  }

  return violations;
}

function formatViolations(violations) {
  const rendered = violations.map(
    ({ path: filePath, entityType, keyAttr, value }) =>
      `- ${filePath}: uses insert for keyed entity ${entityType} via ${keyAttr}="${value}"; use put instead`
  );
  return `Bootstrap uniqueness validation failed:\n${rendered.join("\n")}`;
}

export async function validateBootstrapUniqueness(repoPath) {
  const packageJson = await readJson(path.join(repoPath, "package.json"));
  const loadOrder = packageJson.assembly?.loadOrder ?? [];
  const schemaFiles = new Set((packageJson.schemas ?? []).map((schema) => schema.file));
  const keyedEntities = {};

  for (const relativePath of loadOrder) {
    if (!schemaFiles.has(relativePath)) {
      continue;
    }
    const tql = await fs.readFile(path.join(repoPath, relativePath), "utf8");
    Object.assign(keyedEntities, extractKeyedEntitiesFromSchema(tql));
  }

  if (Object.keys(keyedEntities).length === 0) {
    return;
  }

  const violations = [];
  for (const relativePath of loadOrder) {
    if (schemaFiles.has(relativePath)) {
      continue;
    }
    const tql = await fs.readFile(path.join(repoPath, relativePath), "utf8");
    for (const violation of findInsertStatementsForKeyedEntities(tql, keyedEntities)) {
      violations.push({ path: relativePath, ...violation });
    }
  }

  if (violations.length > 0) {
    throw new Error(formatViolations(violations));
  }
}

export const testing = {
  extractKeyedEntitiesFromSchema,
  extractInsertKeyValues,
  findInsertStatementsForKeyedEntities,
  formatViolations,
};
