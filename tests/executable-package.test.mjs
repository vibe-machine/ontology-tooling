import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import {
  APPLY_UNITS_ROOT,
  MAX_WRITE_UNIT_CHARS,
  prepareExecutablePackage,
  testing,
  validateExecutablePackage,
} from "../src/lib/executable-package.mjs";

async function sha256(filePath) {
  const crypto = await import("node:crypto");
  return crypto.createHash("sha256").update(await fs.readFile(filePath)).digest("hex");
}

async function createFixtureRepo(t, packageJson, files) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "ontology-executable-package-"));
  t.after(async () => {
    await fs.rm(root, { recursive: true, force: true });
  });

  for (const [relativePath, content] of Object.entries(files)) {
    const absolutePath = path.join(root, relativePath);
    await fs.mkdir(path.dirname(absolutePath), { recursive: true });
    await fs.writeFile(absolutePath, content);
  }

  await fs.writeFile(path.join(root, "package.json"), `${JSON.stringify(packageJson, null, 2)}\n`);
  return root;
}

function repeatedPutFile(count, payloadSize = 256) {
  return Array.from({ length: count }, (_, index) => `put $r${index} isa SchemaResource,
  has docKey "doc-${index}",
  has definition "${"x".repeat(payloadSize)}";`).join("\n\n");
}

test("splitExecutableBlocks groups put statements deterministically", () => {
  const blocks = testing.splitExecutableBlocks(`put $a isa Thing, has name "a";\n\nput $b isa Thing, has name "b";\n`);
  assert.deepEqual(blocks, [
    'put $a isa Thing, has name "a";',
    'put $b isa Thing, has name "b";',
  ]);
});

test("resolvePreambles orders recursive dependencies before dependent groups", () => {
  const moduleGroup = {
    variable: "module",
    statements: ['put $module isa OntologyModule, has moduleName "fixture";'],
  };
  const versionGroup = {
    variable: "version",
    statements: [
      'put $version isa OntologyModuleVersion, has moduleVersion "1.0.0";',
      'put (version: $version, module: $module) isa ontologyModuleVersionOf;',
    ],
  };
  const artifactGroup = {
    variable: "artifact",
    statements: [
      'put $artifact isa SourceArtifactRecord, has sourcePath "a";',
      'put (version: $version, sourceArtifact: $artifact) isa ontologyModuleVersionBasedOnSourceArtifact;',
    ],
  };

  const preambles = testing.resolvePreambles([artifactGroup], [moduleGroup, versionGroup, artifactGroup]);
  assert.deepEqual(
    preambles.map((group) => group.variable),
    ["module", "version"]
  );
});

test("prepareExecutablePackage rewrites oversized write files into generated apply units", async (t) => {
  const largeDocs = repeatedPutFile(180, 384);
  assert.ok(largeDocs.length > MAX_WRITE_UNIT_CHARS);

  const repoPath = await createFixtureRepo(
    t,
    {
      name: "fixture",
      displayName: "fixture",
      version: "1.0.0",
      scripts: {
        "refresh:package-contract": "node refresh.mjs",
        "validate:bootstrap": "node validate-bootstrap.mjs",
        "test:typedb-bootstrap": "node test-bootstrap.mjs",
      },
      schemas: [{ name: "fixture", file: "schema/fixture.tql" }],
      data: ["data/docs.tql"],
      provenance: {
        files: ["data/provenance.tql"],
        manifest: "manifests/fixture.package-manifest.json",
      },
      assembly: {
        loadOrder: ["schema/fixture.tql", "data/docs.tql", "data/provenance.tql"],
        generatedArtifacts: [],
      },
      migration: {
        format: 1,
        plans: [
          {
            id: "fixture-0.9.x-to-1.0.0",
            from: "0.9.x",
            to: "1.0.0",
            mode: "compatible",
            phases: [
              {
                id: "write",
                units: [{ kind: "write", path: "migrations/v0.9.0-to-v1.0.0.tql" }],
              },
            ],
          },
        ],
      },
    },
    {
      "schema/fixture.tql": "define\nattribute docKey, value string;\nentity SchemaResource, owns docKey, owns definition;\n",
      "data/docs.tql": largeDocs,
      "data/provenance.tql": 'put $p isa SchemaResource,\n  has docKey "prov";\n',
      "manifests/fixture.package-manifest.json": `${JSON.stringify({
        upstream: {
          sourceArtifacts: [
            { path: "package.json", sha256: "stale" },
            { path: "schema/fixture.tql", sha256: "stale" },
            { path: "data/docs.tql", sha256: "stale" },
            { path: "data/provenance.tql", sha256: "stale" },
          ],
        },
        artifacts: [],
      }, null, 2)}\n`,
      "migrations/v0.9.0-to-v1.0.0.tql": largeDocs,
    }
  );

  const packageJson = await prepareExecutablePackage(repoPath);
  const manifest = JSON.parse(
    await fs.readFile(path.join(repoPath, "manifests", "fixture.package-manifest.json"), "utf8")
  );

  assert.notDeepEqual(packageJson.data, ["data/docs.tql"]);
  assert.ok(packageJson.data.every((entry) => entry.startsWith(`${APPLY_UNITS_ROOT}/data/docs/`)));
  assert.ok(packageJson.assembly.loadOrder.includes("schema/fixture.tql"));
  assert.ok(packageJson.assembly.loadOrder.some((entry) => entry.startsWith(`${APPLY_UNITS_ROOT}/data/docs/`)));
  assert.ok(
    packageJson.migration.plans[0].phases[0].units.every((unit) =>
      unit.path.startsWith(`${APPLY_UNITS_ROOT}/migrations/v0.9.0-to-v1.0.0/`)
    )
  );

  for (const relativePath of packageJson.data) {
    await assert.doesNotReject(fs.access(path.join(repoPath, relativePath)));
    const text = await fs.readFile(path.join(repoPath, relativePath), "utf8");
    assert.match(text, /# manifest: manifests\/fixture\.package-manifest\.json/);
  }
  for (const unit of packageJson.migration.plans[0].phases[0].units) {
    await assert.doesNotReject(fs.access(path.join(repoPath, unit.path)));
  }
  assert.ok(manifest.artifacts.some((artifact) => artifact.path === packageJson.data[0]));
  const packageJsonHash = await sha256(path.join(repoPath, "package.json"));
  const manifestPackageJsonArtifact = manifest.upstream.sourceArtifacts.find((artifact) => artifact.path === "package.json");
  assert.equal(manifestPackageJsonArtifact.sha256, packageJsonHash);
});

test("validateExecutablePackage rejects oversized one-shot write units", async (t) => {
  const largeDocs = repeatedPutFile(180, 384);
  const repoPath = await createFixtureRepo(
    t,
    {
      name: "fixture",
      displayName: "fixture",
      version: "1.0.0",
      scripts: {
        "refresh:package-contract": "node refresh.mjs",
        "validate:bootstrap": "node validate-bootstrap.mjs",
        "test:typedb-bootstrap": "node test-bootstrap.mjs",
      },
      schemas: [{ name: "fixture", file: "schema/fixture.tql" }],
      data: ["data/docs.tql"],
      assembly: {
        loadOrder: ["schema/fixture.tql", "data/docs.tql"],
      },
    },
    {
      "schema/fixture.tql": "define\nattribute docKey, value string;\nentity SchemaResource, owns docKey, owns definition;\n",
      "data/docs.tql": largeDocs,
    }
  );

  await assert.rejects(
    validateExecutablePackage(repoPath),
    /write unit 'data\/docs\.tql'/
  );
});
