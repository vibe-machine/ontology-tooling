import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { validateMigrationContract } from "../src/lib/migration-contract.mjs";

async function createFixtureRepo(t, packageJson, files) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "ontology-migration-contract-"));
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

test("validateMigrationContract accepts replace-mode migration plans", async (t) => {
  const repoPath = await createFixtureRepo(
    t,
    {
      name: "beads",
      version: "0.60.0",
      schemas: [{ name: "beads", file: "schema/beads.tql" }],
      migration: {
        format: 1,
        plans: [
          {
            id: "beads-0.59-to-0.60",
            from: ">=0.59.0 <0.60.0",
            to: "0.60.0",
            mode: "replace",
            snapshot: { required: true, label: "pre-upgrade" },
            phases: [
              {
                id: "preflight",
                units: [{ kind: "assert-schema", path: "migrations/0.60.0/preflight/assert-legacy.tql" }],
              },
              {
                id: "schema",
                units: [{ kind: "schema", path: "migrations/0.60.0/schema/beads.tql" }],
              },
              {
                id: "verify",
                units: [{ kind: "assert-data", path: "migrations/0.60.0/verify/assert-target.tql" }],
              },
            ],
          },
        ],
      },
    },
    {
      "schema/beads.tql": "define\nBead sub entity;",
      "migrations/0.60.0/preflight/assert-legacy.tql": "match $x isa thing; limit 1;",
      "migrations/0.60.0/schema/beads.tql": "undefine\noldType sub entity;\ndefine\nBead sub entity;",
      "migrations/0.60.0/verify/assert-target.tql": "match $x isa thing; limit 1;",
    }
  );

  await assert.doesNotReject(validateMigrationContract(repoPath));
});

test("validateMigrationContract rejects replace mode without required snapshot", async (t) => {
  const repoPath = await createFixtureRepo(
    t,
    {
      name: "beads",
      version: "0.60.0",
      migration: {
        format: 1,
        plans: [
          {
            id: "beads-0.59-to-0.60",
            from: "0.59.x",
            to: "0.60.0",
            mode: "replace",
            phases: [
              {
                id: "verify",
                units: [{ kind: "assert-data", path: "migrations/0.60.0/verify/assert-target.tql" }],
              },
            ],
          },
        ],
      },
    },
    {
      "migrations/0.60.0/verify/assert-target.tql": "match $x isa thing; limit 1;",
    }
  );

  await assert.rejects(validateMigrationContract(repoPath), /must require a snapshot/);
});

test("validateMigrationContract rejects plans targeting a different package version", async (t) => {
  const repoPath = await createFixtureRepo(
    t,
    {
      name: "beads",
      version: "0.60.0",
      migration: {
        format: 1,
        plans: [
          {
            id: "beads-0.58-to-0.59",
            from: "0.58.x",
            to: "0.59.0",
            mode: "compatible",
            phases: [
              {
                id: "verify",
                units: [{ kind: "assert-data", path: "migrations/assert.tql" }],
              },
            ],
          },
        ],
      },
    },
    {
      "migrations/assert.tql": "match $x isa thing; limit 1;",
    }
  );

  await assert.rejects(validateMigrationContract(repoPath), /expected package version 0.60.0/);
});
