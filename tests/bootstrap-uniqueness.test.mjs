import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { executeRelease } from "../src/lib/package-release.mjs";
import { testing, validateBootstrapUniqueness } from "../src/lib/bootstrap-uniqueness.mjs";

test("extractKeyedEntitiesFromSchema returns keyed entity attributes", () => {
  const schema = `define

attribute buildKey, value string;

entity OntologyPackageBuild,
  owns buildKey @key,
  owns packageName;
`;

  assert.deepEqual(testing.extractKeyedEntitiesFromSchema(schema), {
    OntologyPackageBuild: ["buildKey"],
  });
});

test("findInsertStatementsForKeyedEntities reports keyed insert violations", () => {
  const tql = `insert

$build isa OntologyPackageBuild,
  has buildKey "vibemachine@0.2.2:8461ef7",
  has packageName "vibemachine";
`;

  assert.deepEqual(
    testing.findInsertStatementsForKeyedEntities(tql, {
      OntologyPackageBuild: ["buildKey"],
    }),
    [
      {
        entityType: "OntologyPackageBuild",
        keyAttr: "buildKey",
        value: "vibemachine@0.2.2:8461ef7",
      },
    ]
  );
});

async function createFixtureRepo(t, { brokenRefresh = false } = {}) {
  const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "ontology-bootstrap-"));
  t.after(async () => {
    await fs.rm(tempRoot, { recursive: true, force: true });
  });

  const repoPath = path.join(tempRoot, "repo");
  await fs.mkdir(path.join(repoPath, "tools", "package_contract"), { recursive: true });
  await fs.mkdir(path.join(repoPath, "schema"), { recursive: true });
  await fs.mkdir(path.join(repoPath, "data"), { recursive: true });
  await fs.mkdir(path.join(repoPath, "manifests"), { recursive: true });

  const packageJson = {
    name: "fixture-package",
    version: "1.0.0",
    schemas: [{ name: "package-provenance", file: "schema/package-provenance.tql" }],
    data: ["data/fixture-provenance.tql"],
    manifests: ["manifests/fixture-package-v1.0.0.package-manifest.json"],
    provenance: {
      manifest: "manifests/fixture-package-v1.0.0.package-manifest.json",
    },
    assembly: {
      loadOrder: ["schema/package-provenance.tql", "data/fixture-provenance.tql"],
      generatedArtifacts: ["manifests/fixture-package-v1.0.0.package-manifest.json"],
    },
    scripts: {
      "refresh:package-contract": "node tools/package_contract/refresh_package_contract.mjs",
      "validate:bootstrap": "node tools/package_contract/validate_bootstrap.mjs",
      "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
    },
  };

  await fs.writeFile(path.join(repoPath, "package.json"), `${JSON.stringify(packageJson, null, 2)}\n`);
  await fs.writeFile(
    path.join(repoPath, "schema", "package-provenance.tql"),
    `define

attribute buildKey, value string;

entity OntologyPackageBuild,
  owns buildKey @key,
  owns packageName;
`
  );
  await fs.writeFile(
    path.join(repoPath, "manifests", "fixture-package-v1.0.0.package-manifest.json"),
    `${JSON.stringify({ package: { name: "fixture-package", version: "1.0.0" } }, null, 2)}\n`
  );
  await fs.writeFile(
    path.join(repoPath, "tools", "package_contract", "refresh_package_contract.mjs"),
    `import fs from "node:fs/promises";
const tql = ${JSON.stringify(
      brokenRefresh
        ? `insert

$build isa OntologyPackageBuild,
  has buildKey "fixture-package@1.0.1:abc123",
  has packageName "fixture-package";
`
        : `put $build isa OntologyPackageBuild,
  has buildKey "fixture-package@1.0.1:abc123",
  has packageName "fixture-package";
`
    )};
await fs.writeFile("data/fixture-provenance.tql", tql);
await fs.writeFile(
  "manifests/fixture-package-v1.0.1.package-manifest.json",
  JSON.stringify({ package: { name: "fixture-package", version: "1.0.1" } }, null, 2) + "\\n"
);
`
  );
  await fs.writeFile(
    path.join(repoPath, "tools", "package_contract", "validate_bootstrap.mjs"),
    `console.log("bootstrap ok");\n`
  );
  await fs.writeFile(
    path.join(repoPath, "tools", "package_contract", "validate_typedb_bootstrap.mjs"),
    `console.log("typedb bootstrap ok");\n`
  );

  execFileSync("git", ["init", "-b", "main"], { cwd: repoPath, stdio: "ignore" });
  execFileSync("git", ["config", "user.name", "Fixture"], { cwd: repoPath, stdio: "ignore" });
  execFileSync("git", ["config", "user.email", "fixture@example.com"], { cwd: repoPath, stdio: "ignore" });
  execFileSync("git", ["add", "."], { cwd: repoPath, stdio: "ignore" });
  execFileSync("git", ["commit", "-m", "Initial fixture"], { cwd: repoPath, stdio: "ignore" });

  return repoPath;
}

test("validateBootstrapUniqueness accepts put for keyed entities", async (t) => {
  const repoPath = await createFixtureRepo(t);
  await execFileSync("node", ["tools/package_contract/refresh_package_contract.mjs"], { cwd: repoPath, stdio: "ignore" });
  await assert.doesNotReject(() => validateBootstrapUniqueness(repoPath));
});

test("executeRelease rejects release output that inserts keyed entities", async (t) => {
  const repoPath = await createFixtureRepo(t, { brokenRefresh: true });

  await assert.rejects(
    executeRelease({ repo: repoPath, bump: "patch", version: null, dryRun: false, push: false }),
    /Bootstrap uniqueness validation failed/
  );
});
