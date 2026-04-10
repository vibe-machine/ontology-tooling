import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { prepareExecutablePackage } from "../src/lib/executable-package.mjs";

async function createFixtureRepo(t, packageJson, files) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "ontology-release-normalization-"));
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

test("prepareExecutablePackage de-duplicates existing generated apply units in data and loadOrder", async (t) => {
  const repoPath = await createFixtureRepo(
    t,
    {
      name: "fixture",
      displayName: "fixture",
      version: "1.0.0",
      data: [
        "generated/apply-units/data/docs/0001.tql",
        "generated/apply-units/data/docs/0001.tql",
      ],
      manifests: ["manifests/fixture.package-manifest.json"],
      provenance: {
        manifest: "manifests/fixture.package-manifest.json",
      },
      assembly: {
        loadOrder: [
          "schema/fixture.tql",
          "generated/apply-units/data/docs/0001.tql",
          "generated/apply-units/data/docs/0001.tql",
        ],
        generatedArtifacts: ["generated/apply-units/data/docs/0001.tql"],
      },
      schemas: [{ name: "fixture", file: "schema/fixture.tql" }],
    },
    {
      "schema/fixture.tql": "define\nattribute docKey, value string;\nentity SchemaResource, owns docKey;\n",
      "generated/apply-units/data/docs/0001.tql": '# Generated executable apply unit from data/docs.tql\n\nput $r isa SchemaResource,\n  has docKey "doc-1";\n',
      "manifests/fixture.package-manifest.json": `${JSON.stringify({ upstream: { sourceArtifacts: [] }, artifacts: [] }, null, 2)}\n`,
    }
  );

  const packageJson = await prepareExecutablePackage(repoPath);

  assert.deepEqual(packageJson.data, ["generated/apply-units/data/docs/0001.tql"]);
  assert.deepEqual(packageJson.assembly.loadOrder, [
    "schema/fixture.tql",
    "generated/apply-units/data/docs/0001.tql",
  ]);
});
