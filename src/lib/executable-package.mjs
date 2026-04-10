import fs from "node:fs/promises";
import path from "node:path";

export const APPLY_UNITS_ROOT = "generated/apply-units";
export const MAX_WRITE_UNIT_CHARS = 50_000;
export const MAX_WRITE_UNIT_BLOCKS = 25;

function isNonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function toPosix(relativePath) {
  return relativePath.split(path.sep).join(path.posix.sep);
}

function isTypeQLFile(relativePath) {
  return typeof relativePath === "string" && relativePath.endsWith(".tql");
}

function isGeneratedApplyUnit(relativePath) {
  return typeof relativePath === "string"
    && (relativePath === APPLY_UNITS_ROOT || relativePath.startsWith(`${APPLY_UNITS_ROOT}/`));
}

function trimCommentLines(lines) {
  return lines.filter((line) => {
    const trimmed = line.trim();
    return trimmed.length > 0 && !trimmed.startsWith("#");
  });
}

function splitPutStatements(text) {
  const statements = [];
  let current = "";

  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed.startsWith("#") || trimmed === "") continue;

    if (trimmed.startsWith("put ") && current) {
      statements.push(current.trim());
      current = line;
    } else {
      current += `${current ? "\n" : ""}${line}`;
    }
  }

  if (current.trim()) {
    statements.push(current.trim());
  }
  return statements;
}

function groupPutStatements(statements) {
  const groups = [];
  let currentGroup = null;

  for (const statement of statements) {
    const entityMatch = statement.match(/^put\s+\$(\w+)\s+isa\s+(\w+)/);

    if (entityMatch) {
      if (currentGroup) groups.push(currentGroup);
      currentGroup = { variable: entityMatch[1], type: entityMatch[2], statements: [statement] };
    } else if (currentGroup && statement.includes(`$${currentGroup.variable}`)) {
      currentGroup.statements.push(statement);
    } else {
      if (currentGroup) groups.push(currentGroup);
      currentGroup = { variable: null, type: null, statements: [statement] };
    }
  }

  if (currentGroup) groups.push(currentGroup);
  return groups;
}

function referencedVariables(text) {
  return [...text.matchAll(/\$(\w+)/g)].map((match) => match[1]);
}

function resolvePreambles(targetGroups, allGroups) {
  const targetSet = new Set(targetGroups);
  const resolved = [];
  const resolvedVariables = new Set(targetGroups.filter((group) => group.variable).map((group) => group.variable));
  const requiredVariables = new Set();

  for (const group of targetGroups) {
    for (const variable of referencedVariables(group.statements.join("\n"))) {
      if (!resolvedVariables.has(variable)) {
        requiredVariables.add(variable);
      }
    }
  }

  let changed = true;
  while (changed) {
    changed = false;
    for (const group of allGroups) {
      if (!group.variable || targetSet.has(group) || resolved.includes(group)) continue;
      if (!requiredVariables.has(group.variable)) continue;

      resolved.push(group);
      resolvedVariables.add(group.variable);
      changed = true;

      for (const variable of referencedVariables(group.statements.join("\n"))) {
        if (!resolvedVariables.has(variable)) {
          requiredVariables.add(variable);
        }
      }
    }
  }

  return resolved;
}

function splitParagraphQueries(text) {
  const blocks = [];
  let current = [];

  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();

    if (trimmed.startsWith("#")) {
      continue;
    }

    if (trimmed === "") {
      if (current.length > 0) {
        blocks.push(current.join("\n").trim());
        current = [];
      }
      continue;
    }

    current.push(line);
  }

  if (current.length > 0) {
    blocks.push(current.join("\n").trim());
  }

  return blocks.filter(Boolean);
}

export function splitExecutableBlocks(text) {
  const trimmed = text.trim();
  if (!trimmed) return [];

  const significantLines = trimCommentLines(trimmed.split(/\r?\n/)).map((line) => line.trimStart());
  const hasPut = significantLines.some((line) => line.startsWith("put "));
  const hasNonPutWrite = significantLines.some((line) =>
    line.startsWith("match")
    || line.startsWith("insert ")
    || line.startsWith("delete ")
    || line.startsWith("update ")
  );

  if (hasPut && !hasNonPutWrite) {
    return groupPutStatements(splitPutStatements(trimmed)).map((group) => group.statements.join("\n\n"));
  }

  return splitParagraphQueries(trimmed);
}

function chunkBlocks(blocks, { maxChars = MAX_WRITE_UNIT_CHARS, maxBlocks = MAX_WRITE_UNIT_BLOCKS } = {}) {
  const chunks = [];
  let current = [];
  let currentChars = 0;

  for (const block of blocks) {
    if (block.length > maxChars) {
      throw new Error(
        `write block exceeds safe size limit (${block.length} chars > ${maxChars}) and must be split at the source`
      );
    }

    const separatorChars = current.length > 0 ? 2 : 0;
    const candidateChars = currentChars + separatorChars + block.length;
    if (current.length > 0 && (candidateChars > maxChars || current.length >= maxBlocks)) {
      chunks.push(current);
      current = [block];
      currentChars = block.length;
      continue;
    }

    current.push(block);
    currentChars = candidateChars;
  }

  if (current.length > 0) {
    chunks.push(current);
  }

  return chunks;
}

function buildShardPath(sourcePath, index) {
  const stem = sourcePath.replace(/\.tql$/, "");
  return path.posix.join(APPLY_UNITS_ROOT, stem, `${String(index + 1).padStart(4, "0")}.tql`);
}

function renderShard(sourcePath, blocks) {
  return `# Generated executable apply unit from ${sourcePath}\n\n${blocks.join("\n\n")}\n`;
}

function buildPutChunks(text, { maxChars = MAX_WRITE_UNIT_CHARS, maxBlocks = MAX_WRITE_UNIT_BLOCKS } = {}) {
  const groups = groupPutStatements(splitPutStatements(text));
  if (groups.length === 0) return [];

  const chunks = [];
  let currentGroups = [];

  const renderCandidate = (candidateGroups) => {
    const preambles = resolvePreambles(candidateGroups, groups);
    return [...preambles, ...candidateGroups].map((group) => group.statements.join("\n\n"));
  };

  for (const group of groups) {
    const candidateGroups = [...currentGroups, group];
    const renderedBlocks = renderCandidate(candidateGroups);
    const renderedText = renderShard("generated", renderedBlocks);

    if (currentGroups.length > 0 && (candidateGroups.length > maxBlocks || renderedText.length > maxChars)) {
      chunks.push(renderCandidate(currentGroups));
      currentGroups = [group];
      continue;
    }

    currentGroups = candidateGroups;
  }

  if (currentGroups.length > 0) {
    chunks.push(renderCandidate(currentGroups));
  }

  for (const chunk of chunks) {
    if (renderShard("generated", chunk).length > maxChars) {
      throw new Error(`write chunk exceeds safe size limit (${maxChars} chars) and must be split at the source`);
    }
  }

  return chunks;
}

async function readJson(filePath) {
  return JSON.parse(await fs.readFile(filePath, "utf8"));
}

async function writeJson(filePath, value) {
  await fs.writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

function listProvenanceFiles(packageJson) {
  if (Array.isArray(packageJson.provenance)) {
    return packageJson.provenance;
  }
  if (packageJson.provenance && typeof packageJson.provenance === "object") {
    return Array.isArray(packageJson.provenance.files) ? packageJson.provenance.files : [];
  }
  return [];
}

function setProvenanceFiles(packageJson, files) {
  if (Array.isArray(packageJson.provenance)) {
    packageJson.provenance = files;
    return;
  }
  if (packageJson.provenance && typeof packageJson.provenance === "object") {
    packageJson.provenance.files = files;
  }
}

function schemaPathSet(packageJson) {
  return new Set((packageJson.schemas ?? []).map((schema) => schema.file));
}

function unique(values) {
  return [...new Set(values)];
}

function shouldNormalizeAssemblyPath(packageJson, relativePath) {
  return isTypeQLFile(relativePath) && !schemaPathSet(packageJson).has(relativePath);
}

function shouldNormalizeWriteUnit(relativePath) {
  return isTypeQLFile(relativePath);
}

async function loadAssetText(repoPath, relativePath) {
  return fs.readFile(path.join(repoPath, relativePath), "utf8");
}

export async function prepareExecutablePackage(repoPath, options = {}) {
  const packageJsonPath = path.join(repoPath, "package.json");
  const packageJson = await readJson(packageJsonPath);
  const generatedPaths = [];
  const normalizedPathCache = new Map();
  const sourceTextCache = new Map();

  const trackedPaths = new Set();
  for (const assetPath of packageJson.assembly?.loadOrder ?? []) {
    if (shouldNormalizeAssemblyPath(packageJson, assetPath)) trackedPaths.add(assetPath);
  }
  for (const assetPath of packageJson.data ?? []) {
    if (shouldNormalizeWriteUnit(assetPath)) trackedPaths.add(assetPath);
  }
  for (const assetPath of listProvenanceFiles(packageJson)) {
    if (shouldNormalizeWriteUnit(assetPath)) trackedPaths.add(assetPath);
  }
  for (const plan of packageJson.migration?.plans ?? []) {
    for (const phase of plan.phases ?? []) {
      for (const unit of phase.units ?? []) {
        if (unit.kind === "write" && shouldNormalizeWriteUnit(unit.path)) {
          trackedPaths.add(unit.path);
        }
      }
    }
  }

  for (const relativePath of trackedPaths) {
    sourceTextCache.set(relativePath, await loadAssetText(repoPath, relativePath));
  }

  await fs.rm(path.join(repoPath, APPLY_UNITS_ROOT), { recursive: true, force: true });

  const normalizePath = async (relativePath) => {
    if (!shouldNormalizeWriteUnit(relativePath)) {
      return [relativePath];
    }
    if (normalizedPathCache.has(relativePath)) {
      return normalizedPathCache.get(relativePath);
    }

    const text = sourceTextCache.get(relativePath) ?? await loadAssetText(repoPath, relativePath);
    const trimmed = text.trim();
    if (!trimmed) {
      normalizedPathCache.set(relativePath, [relativePath]);
      return [relativePath];
    }

    const blocks = splitExecutableBlocks(trimmed);
    if (blocks.length === 0) {
      normalizedPathCache.set(relativePath, [relativePath]);
      return [relativePath];
    }

    const significantLines = trimCommentLines(trimmed.split(/\r?\n/)).map((line) => line.trimStart());
    const hasPutOnly = significantLines.some((line) => line.startsWith("put "))
      && !significantLines.some((line) =>
        line.startsWith("match")
        || line.startsWith("insert ")
        || line.startsWith("delete ")
        || line.startsWith("update ")
      );
    const chunks = hasPutOnly ? buildPutChunks(trimmed, options) : chunkBlocks(blocks, options);

    if (chunks.length === 1
        && chunks[0].length <= MAX_WRITE_UNIT_BLOCKS
        && renderShard(relativePath, chunks[0]).length <= MAX_WRITE_UNIT_CHARS) {
      normalizedPathCache.set(relativePath, [relativePath]);
      return [relativePath];
    }

    const emittedPaths = [];
    for (const [index, chunk] of chunks.entries()) {
      const shardPath = buildShardPath(relativePath, index);
      const absolutePath = path.join(repoPath, shardPath);
      await fs.mkdir(path.dirname(absolutePath), { recursive: true });
      await fs.writeFile(absolutePath, renderShard(relativePath, chunk), "utf8");
      emittedPaths.push(toPosix(shardPath));
      generatedPaths.push(toPosix(shardPath));
    }

    normalizedPathCache.set(relativePath, emittedPaths);
    return emittedPaths;
  };

  if (Array.isArray(packageJson.data)) {
    const nextData = [];
    for (const relativePath of packageJson.data) {
      nextData.push(...await normalizePath(relativePath));
    }
    packageJson.data = nextData;
  }

  const provenanceFiles = listProvenanceFiles(packageJson);
  if (provenanceFiles.length > 0) {
    const nextFiles = [];
    for (const relativePath of provenanceFiles) {
      nextFiles.push(...await normalizePath(relativePath));
    }
    setProvenanceFiles(packageJson, nextFiles);
  }

  if (Array.isArray(packageJson.assembly?.loadOrder)) {
    const nextLoadOrder = [];
    for (const relativePath of packageJson.assembly.loadOrder) {
      if (shouldNormalizeAssemblyPath(packageJson, relativePath)) {
        nextLoadOrder.push(...await normalizePath(relativePath));
      } else {
        nextLoadOrder.push(relativePath);
      }
    }
    packageJson.assembly.loadOrder = nextLoadOrder;
  }

  if (packageJson.migration?.plans) {
    for (const plan of packageJson.migration.plans) {
      for (const phase of plan.phases ?? []) {
        const nextUnits = [];
        for (const unit of phase.units ?? []) {
          if (unit.kind === "write" && shouldNormalizeWriteUnit(unit.path)) {
            for (const shardPath of await normalizePath(unit.path)) {
              nextUnits.push({ ...unit, path: shardPath });
            }
          } else {
            nextUnits.push(unit);
          }
        }
        phase.units = nextUnits;
      }
    }
  }

  if (packageJson.assembly) {
    const existingGenerated = (packageJson.assembly.generatedArtifacts ?? []).filter(
      (relativePath) => !isGeneratedApplyUnit(relativePath)
    );
    packageJson.assembly.generatedArtifacts = unique([...existingGenerated, ...generatedPaths]);
  }

  await writeJson(packageJsonPath, packageJson);
  return packageJson;
}

function assertSafeWriteUnit(relativePath, text) {
  const trimmed = text.trim();
  if (!trimmed) {
    throw new Error(`write unit '${relativePath}' is empty`);
  }

  const blocks = splitExecutableBlocks(trimmed);
  if (blocks.length === 0) {
    throw new Error(`write unit '${relativePath}' does not contain executable blocks`);
  }

  if (blocks.length > MAX_WRITE_UNIT_BLOCKS) {
    throw new Error(
      `write unit '${relativePath}' contains ${blocks.length} executable blocks; max is ${MAX_WRITE_UNIT_BLOCKS}`
    );
  }

  if (trimmed.length > MAX_WRITE_UNIT_CHARS) {
    throw new Error(
      `write unit '${relativePath}' is ${trimmed.length} chars; max is ${MAX_WRITE_UNIT_CHARS}. Publish sharded apply units instead of a single large query.`
    );
  }
}

export async function validateExecutablePackage(repoPath) {
  const packageJson = await readJson(path.join(repoPath, "package.json"));
  const checkedPaths = new Set();

  const maybeCheck = async (relativePath) => {
    if (!shouldNormalizeWriteUnit(relativePath) || checkedPaths.has(relativePath)) return;
    checkedPaths.add(relativePath);
    const text = await loadAssetText(repoPath, relativePath);
    assertSafeWriteUnit(relativePath, text);
  };

  for (const relativePath of packageJson.data ?? []) {
    await maybeCheck(relativePath);
  }

  for (const relativePath of listProvenanceFiles(packageJson)) {
    await maybeCheck(relativePath);
  }

  for (const relativePath of packageJson.assembly?.loadOrder ?? []) {
    if (shouldNormalizeAssemblyPath(packageJson, relativePath)) {
      await maybeCheck(relativePath);
    }
  }

  for (const plan of packageJson.migration?.plans ?? []) {
    for (const phase of plan.phases ?? []) {
      for (const unit of phase.units ?? []) {
        if (unit.kind === "write") {
          await maybeCheck(unit.path);
        }
      }
    }
  }

  return packageJson;
}

export const testing = {
  buildShardPath,
  buildPutChunks,
  chunkBlocks,
  groupPutStatements,
  isGeneratedApplyUnit,
  resolvePreambles,
  renderShard,
  splitExecutableBlocks,
  splitParagraphQueries,
  splitPutStatements,
};
