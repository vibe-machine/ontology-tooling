import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";

import { generateMigrationDiff, testing } from "../src/lib/migration-diff.mjs";

const {
  splitPutStatements,
  groupPutStatements,
  extractGroupKey,
  resolvePreambles,
  parseHasClauses,
  diffEntityGroup,
  nonMigratableAssetPathsFromPackageJson,
} = testing;

test("nonMigratableAssetPathsFromPackageJson excludes manifest but keeps provenance data migratable", () => {
  assert.deepEqual(
    [...nonMigratableAssetPathsFromPackageJson({ provenance: ["data/build.tql"] })],
    []
  );
  assert.deepEqual(
    [...nonMigratableAssetPathsFromPackageJson({
      provenance: { files: ["data/build.tql"], manifest: "manifests/build.package-manifest.json" },
    })].sort(),
    ["manifests/build.package-manifest.json"]
  );
  assert.deepEqual(
    [...nonMigratableAssetPathsFromPackageJson({
      assembly: { generatedArtifacts: ["data/example-provenance.tql", "data/example-schema-docs.tql"] },
    })],
    []
  );
});

test("splitPutStatements parses multi-line put statements", () => {
  const text = `
# comment
put $r1 isa SchemaResource,
  has docKey "key1",
  has typeLabel "Type1";
put (resource: $r1, module: $module) isa inModule;

put $r2 isa SchemaResource,
  has docKey "key2";
`;
  const stmts = splitPutStatements(text);
  assert.equal(stmts.length, 3);
  assert.ok(stmts[0].startsWith("put $r1 isa SchemaResource,"));
  assert.ok(stmts[0].includes('has docKey "key1"'));
  assert.ok(stmts[1].startsWith("put (resource: $r1"));
  assert.ok(stmts[2].startsWith("put $r2 isa SchemaResource,"));
});

test("splitPutStatements handles empty input", () => {
  assert.deepEqual(splitPutStatements(""), []);
  assert.deepEqual(splitPutStatements("# just comments\n# more"), []);
});

test("groupPutStatements groups entity with its relation", () => {
  const stmts = [
    'put $module isa SchemaModule,\n  has moduleKey "vibemachine";',
    'put $r1 isa SchemaResource,\n  has docKey "key1";',
    "put (resource: $r1, module: $module) isa inModule;",
    'put $r2 isa SchemaResource,\n  has docKey "key2";',
    "put (resource: $r2, module: $module) isa inModule;",
  ];
  const groups = groupPutStatements(stmts);
  assert.equal(groups.length, 3);
  assert.equal(groups[0].variable, "module");
  assert.equal(groups[0].statements.length, 1);
  assert.equal(groups[1].variable, "r1");
  assert.equal(groups[1].statements.length, 2);
  assert.equal(groups[2].variable, "r2");
  assert.equal(groups[2].statements.length, 2);
});

test("extractGroupKey uses type and first has attribute", () => {
  const group = {
    variable: "r1",
    type: "SchemaResource",
    statements: ['put $r1 isa SchemaResource,\n  has docKey "https://example.com#Foo",\n  has typeLabel "Foo";'],
  };
  assert.equal(extractGroupKey(group), 'SchemaResource::docKey::https://example.com#Foo');
});

test("extractGroupKey falls back to raw prefix for keyless statements", () => {
  const group = {
    variable: null,
    type: null,
    statements: ["put (resource: $r1, module: $module) isa inModule;"],
  };
  const key = extractGroupKey(group);
  assert.ok(key.startsWith("raw::"));
});

test("resolvePreambles includes referenced but undefined variables", () => {
  const moduleGroup = {
    variable: "module",
    type: "SchemaModule",
    statements: ['put $module isa SchemaModule, has moduleKey "test";'],
  };
  const changedGroup = {
    variable: "r1",
    type: "SchemaResource",
    statements: [
      'put $r1 isa SchemaResource, has docKey "key1";',
      "put (resource: $r1, module: $module) isa inModule;",
    ],
  };

  const preambles = resolvePreambles([changedGroup], [moduleGroup, changedGroup]);
  assert.equal(preambles.length, 1);
  assert.equal(preambles[0].variable, "module");
});

test("resolvePreambles returns empty when all variables are self-contained", () => {
  const group = {
    variable: "draft",
    type: "SpecificationStatus",
    statements: ['put $draft isa SpecificationStatus, has status_label "draft";'],
  };
  const preambles = resolvePreambles([group], [group]);
  assert.equal(preambles.length, 0);
});

test("parseHasClauses extracts attribute-value pairs", () => {
  const stmt = 'put $r1 isa SchemaResource,\n  has docKey "key1",\n  has typeLabel "Type1",\n  has scopeNote "some note";';
  const clauses = parseHasClauses(stmt);
  assert.equal(clauses.length, 3);
  assert.equal(clauses[0].attribute, "docKey");
  assert.equal(clauses[0].value, '"key1"');
  assert.equal(clauses[1].attribute, "typeLabel");
  assert.equal(clauses[2].attribute, "scopeNote");
  assert.equal(clauses[2].value, '"some note"');
});

test("diffEntityGroup generates match/delete/insert for changed scopeNote", () => {
  const oldGroup = {
    variable: "r1",
    type: "SchemaResource",
    statements: [
      'put $r1 isa SchemaResource,\n  has docKey "key1",\n  has typeLabel "Type1",\n  has scopeNote "old note";',
      "put (resource: $r1, module: $module) isa inModule;",
    ],
  };
  const newGroup = {
    variable: "r1",
    type: "SchemaResource",
    statements: [
      'put $r1 isa SchemaResource,\n  has docKey "key1",\n  has typeLabel "Type1",\n  has scopeNote "new note";',
      "put (resource: $r1, module: $module) isa inModule;",
    ],
  };

  const result = diffEntityGroup(oldGroup, newGroup);
  assert.ok(result, "should produce an update statement");
  assert.ok(result.includes("match"));
  assert.ok(result.includes('has docKey "key1"'));
  assert.ok(result.includes("has scopeNote $r1_old_scopeNote"));
  assert.ok(result.includes("delete"));
  assert.ok(result.includes("has $r1_old_scopeNote of $r1"));
  assert.ok(result.includes("insert"));
  assert.ok(result.includes('"new note"'));
  // Should NOT include typeLabel in the update
  assert.ok(!result.includes("typeLabel"));
});

test("diffEntityGroup returns null when only relation puts changed", () => {
  const oldGroup = {
    variable: "r1",
    type: "SchemaResource",
    statements: [
      'put $r1 isa SchemaResource,\n  has docKey "key1",\n  has scopeNote "same";',
      "put (resource: $r1, module: $old_module) isa inModule;",
    ],
  };
  const newGroup = {
    variable: "r1",
    type: "SchemaResource",
    statements: [
      'put $r1 isa SchemaResource,\n  has docKey "key1",\n  has scopeNote "same";',
      "put (resource: $r1, module: $new_module) isa inModule;",
    ],
  };

  const result = diffEntityGroup(oldGroup, newGroup);
  assert.equal(result, null);
});

test("full diff flow: changed entity produces update, new entity produces put", () => {
  const oldText = `
put $module isa SchemaModule,
  has moduleKey "test",
  has moduleName "test";

put $r1 isa SchemaResource,
  has docKey "key1",
  has scopeNote "old note";
put (resource: $r1, module: $module) isa inModule;
`;

  const newText = `
put $module isa SchemaModule,
  has moduleKey "test",
  has moduleName "test";

put $r1 isa SchemaResource,
  has docKey "key1",
  has scopeNote "new note";
put (resource: $r1, module: $module) isa inModule;

put $r3 isa SchemaResource,
  has docKey "key3",
  has scopeNote "brand new";
put (resource: $r3, module: $module) isa inModule;
`;

  const oldGroups = groupPutStatements(splitPutStatements(oldText));
  const newGroups = groupPutStatements(splitPutStatements(newText));

  const oldMap = new Map();
  for (const g of oldGroups) oldMap.set(extractGroupKey(g), g);

  const newPuts = [];
  const updates = [];
  for (const g of newGroups) {
    const key = extractGroupKey(g);
    const oldGroup = oldMap.get(key);
    if (!oldGroup) {
      newPuts.push(g);
    } else if (oldGroup.statements.join("\n") !== g.statements.join("\n")) {
      const update = diffEntityGroup(oldGroup, g);
      if (update) updates.push(update);
    }
  }

  // $r1 changed → update statement
  assert.equal(updates.length, 1);
  assert.ok(updates[0].includes('has docKey "key1"'));
  assert.ok(updates[0].includes('"new note"'));

  // $r3 is new → put statement
  assert.equal(newPuts.length, 1);
  assert.equal(newPuts[0].variable, "r3");
});

test("generateMigrationDiff includes target provenance updates in the migration file", async (t) => {
  const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "ontology-migration-diff-"));
  t.after(async () => {
    await fs.rm(tempRoot, { recursive: true, force: true });
  });

  const repoPath = path.join(tempRoot, "repo");
  await fs.mkdir(path.join(repoPath, "data"), { recursive: true });

  const packageJson = {
    name: "fixture-package",
    version: "1.0.1",
    data: ["data/seed.tql", "data/provenance.tql"],
    provenance: ["data/provenance.tql"],
  };

  await fs.writeFile(path.join(repoPath, "package.json"), `${JSON.stringify(packageJson, null, 2)}\n`);
  await fs.writeFile(path.join(repoPath, "data", "seed.tql"), 'put $seed isa SeedThing,\n  has seedKey "seed-1";\n');
  await fs.writeFile(
    path.join(repoPath, "data", "provenance.tql"),
    'put $version isa OntologyModuleVersion,\n  has moduleVersionKey "https://example.com/fixture-package@1.0.0";\n'
  );

  execFileSync("git", ["init", "-b", "main"], { cwd: repoPath, stdio: "ignore" });
  execFileSync("git", ["config", "user.name", "Fixture"], { cwd: repoPath, stdio: "ignore" });
  execFileSync("git", ["config", "user.email", "fixture@example.com"], { cwd: repoPath, stdio: "ignore" });
  execFileSync("git", ["add", "."], { cwd: repoPath, stdio: "ignore" });
  execFileSync("git", ["commit", "-m", "Initial fixture"], { cwd: repoPath, stdio: "ignore" });
  execFileSync("git", ["tag", "v1.0.0"], { cwd: repoPath, stdio: "ignore" });

  await fs.writeFile(path.join(repoPath, "data", "seed.tql"), 'put $seed isa SeedThing,\n  has seedKey "seed-2";\n');
  await fs.writeFile(
    path.join(repoPath, "data", "provenance.tql"),
    'put $version isa OntologyModuleVersion,\n  has moduleVersionKey "https://example.com/fixture-package@1.0.1";\n'
  );

  const migrationRelPath = await generateMigrationDiff(repoPath, "1.0.0", "1.0.1");
  assert.equal(migrationRelPath, "migrations/v1.0.0-to-v1.0.1.tql");

  const migrationText = await fs.readFile(path.join(repoPath, migrationRelPath), "utf8");
  assert.match(migrationText, /seed-2/);
  assert.match(migrationText, /OntologyModuleVersion/);
  assert.match(migrationText, /fixture-package@1\.0\.1/);
});
