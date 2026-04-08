import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { validatePackageContract } from "../src/lib/package-validator.mjs";

async function createFixtureRepo(t, packageJson, files) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "ontology-package-validator-"));
  t.after(async () => {
    await fs.rm(root, { recursive: true, force: true });
  });

  for (const [relativePath, content] of Object.entries(files)) {
    const absolutePath = path.join(root, relativePath);
    await fs.mkdir(path.dirname(absolutePath), { recursive: true });
    await fs.writeFile(absolutePath, content);
  }

  await fs.writeFile(path.join(root, "package.json"), `${JSON.stringify(packageJson, null, 2)}\n`);
  await fs.writeFile(path.join(root, ".gitignore"), "node_modules/\n");
  execFileSync("git", ["init", "-b", "main"], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["config", "user.name", "Fixture"], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["config", "user.email", "fixture@example.com"], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["add", "."], { cwd: root, stdio: "ignore" });
  execFileSync("git", ["commit", "-m", "Initial fixture"], { cwd: root, stdio: "ignore" });
  return root;
}

function basePackageJson(overrides = {}) {
  const { scripts: overrideScripts, ...restOverrides } = overrides;
  return {
    name: "custom",
    displayName: "custom",
    version: "1.0.0",
    scripts: {
      "refresh:package-contract": "node refresh.mjs",
      "validate:bootstrap": "node validate-bootstrap.mjs",
      "test:typedb-bootstrap": "node test-bootstrap.mjs",
      ...(overrideScripts ?? {}),
    },
    schemas: [{ name: "custom", file: "schema/custom.tql" }],
    data: ["data/seed.tql"],
    manifests: ["manifests/resources.json"],
    provenance: {
      manifest: "manifests/resources.json",
      status: "bootstrap",
    },
    assembly: {
      loadOrder: ["schema/custom.tql", "data/seed.tql", "manifests/resources.json"],
    },
    ...restOverrides,
  };
}

const baseFiles = {
  "schema/custom.tql": "define\ncustom sub entity;",
  "data/seed.tql": "insert $x isa thing;",
  "manifests/resources.json": "{}",
};

test("validatePackageContract accepts a valid self-describing package", async (t) => {
  const repoPath = await createFixtureRepo(t, basePackageJson(), baseFiles);
  await assert.doesNotReject(validatePackageContract(repoPath));
});

test("validatePackageContract rejects undeclared assembly assets", async (t) => {
  const repoPath = await createFixtureRepo(
    t,
    basePackageJson({
      assembly: {
        loadOrder: ["schema/custom.tql", "data/not-declared.tql"],
      },
    }),
    {
      ...baseFiles,
      "data/not-declared.tql": "insert $x isa thing;",
    }
  );

  await assert.rejects(
    validatePackageContract(repoPath),
    /assembly\.loadOrder references undeclared asset: data\/not-declared\.tql/
  );
});

test("validatePackageContract requires live migration testing when migrations are declared", async (t) => {
  const repoPath = await createFixtureRepo(
    t,
    basePackageJson({
      migration: {
        format: 1,
        plans: [
          {
            id: "custom-0.9.x-to-1.0.0",
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
    }),
    {
      ...baseFiles,
      "migrations/v0.9.0-to-v1.0.0.tql": "match $x isa thing; insert $y isa thing;",
    }
  );

  await assert.rejects(
    validatePackageContract(repoPath),
    /scripts must define 'test:typedb-migration'/
  );
});

test("validatePackageContract rejects migration plans that do not cover the previous release tag", async (t) => {
  const repoPath = await createFixtureRepo(
    t,
    basePackageJson({
      version: "1.0.1",
      scripts: {
        "test:typedb-migration": "node test-migration.mjs",
      },
      migration: {
        format: 1,
        plans: [
          {
            id: "custom-0.8.x-to-1.0.1",
            from: "0.8.x",
            to: "1.0.1",
            mode: "compatible",
            phases: [
              {
                id: "write",
                units: [{ kind: "write", path: "migrations/v0.8.0-to-v1.0.1.tql" }],
              },
            ],
          },
        ],
      },
    }),
    {
      ...baseFiles,
      "migrations/v0.8.0-to-v1.0.1.tql": "match $x isa thing; insert $y isa thing;",
    }
  );

  execFileSync("git", ["tag", "v1.0.0"], { cwd: repoPath, stdio: "ignore" });

  await assert.rejects(
    validatePackageContract(repoPath),
    /migration plans do not cover previous release 1\.0\.0 -> 1\.0\.1/
  );
});

test("validatePackageContract accepts migration plans that cover the previous release tag", async (t) => {
  const repoPath = await createFixtureRepo(
    t,
    basePackageJson({
      version: "1.0.1",
      scripts: {
        "test:typedb-migration": "node test-migration.mjs",
      },
      migration: {
        format: 1,
        plans: [
          {
            id: "custom-1.0.x-to-1.0.1",
            from: ">=1.0.0 <1.0.1",
            to: "1.0.1",
            mode: "compatible",
            phases: [
              {
                id: "write",
                units: [{ kind: "write", path: "migrations/v1.0.0-to-v1.0.1.tql" }],
              },
            ],
          },
        ],
      },
    }),
    {
      ...baseFiles,
      "migrations/v1.0.0-to-v1.0.1.tql": "match $x isa thing; insert $y isa thing;",
    }
  );

  execFileSync("git", ["tag", "v1.0.0"], { cwd: repoPath, stdio: "ignore" });

  await assert.doesNotReject(validatePackageContract(repoPath));
});
