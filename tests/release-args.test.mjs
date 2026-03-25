import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { executeRelease, planPackageRelease } from "../src/lib/package-release.mjs";
import { parseReleaseArgs, validateReleaseArgs } from "../src/lib/release-args.mjs";
import { bumpVersion, resolveReleaseVersion } from "../src/lib/versions.mjs";

test("parseReleaseArgs parses dry-run bump flow", () => {
  const options = parseReleaseArgs(["--repo", "../ontology-beads", "--bump", "patch", "--dry-run"]);
  assert.deepEqual(options, {
    repo: "../ontology-beads",
    bump: "patch",
    version: null,
    dryRun: true,
    validateOnly: false,
    push: true,
    help: false,
  });
});

test("validateReleaseArgs rejects missing release target", () => {
  assert.throws(() => validateReleaseArgs(parseReleaseArgs(["--bump", "patch"])), /--repo/);
});

test("validateReleaseArgs rejects bump and version together", () => {
  assert.throws(
    () => validateReleaseArgs(parseReleaseArgs(["--repo", ".", "--bump", "patch", "--version", "1.2.3"])),
    /either --bump or --version, not both/
  );
});

test("validateReleaseArgs accepts validate-only mode", () => {
  assert.doesNotThrow(() => validateReleaseArgs(parseReleaseArgs(["--repo", ".", "--validate-only"])));
});

test("validateReleaseArgs rejects validate-only mixed with release args", () => {
  assert.throws(
    () => validateReleaseArgs(parseReleaseArgs(["--repo", ".", "--validate-only", "--bump", "patch"])),
    /validate-only/
  );
});

test("bumpVersion increments patch, minor, and major semver values", () => {
  assert.equal(bumpVersion("1.2.3", "patch"), "1.2.4");
  assert.equal(bumpVersion("1.2.3", "minor"), "1.3.0");
  assert.equal(bumpVersion("1.2.3", "major"), "2.0.0");
});

test("resolveReleaseVersion accepts an explicit version", () => {
  assert.equal(resolveReleaseVersion("1.2.3", { version: "2.0.0", bump: null }), "2.0.0");
});

test("planPackageRelease rewrites versioned manifest references", () => {
  const plan = planPackageRelease(
    {
      name: "example",
      version: "1.0.0",
      manifests: ["manifests/example-v1.0.0.package-manifest.json"],
      provenance: {
        manifest: "manifests/example-v1.0.0.package-manifest.json",
      },
      assembly: {
        generatedArtifacts: [
          "manifests/example-v1.0.0.package-manifest.json",
          "manifests/example-v1.0.0.report.json",
        ],
      },
      upstream: {
        tag: "v1.0.0",
      },
      scripts: {
        "refresh:package-contract": "node tools/package_contract/refresh_package_contract.mjs",
        "validate:bootstrap": "node tools/package_contract/validate_bootstrap.mjs",
        "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
      },
    },
    { bump: "patch", version: null }
  );

  assert.equal(plan.nextVersion, "1.0.1");
  assert.equal(plan.nextPackageJson.provenance.manifest, "manifests/example-v1.0.1.package-manifest.json");
  assert.deepEqual(plan.renamePlan, [
    {
      from: "manifests/example-v1.0.0.package-manifest.json",
      to: "manifests/example-v1.0.1.package-manifest.json",
    },
    {
      from: "manifests/example-v1.0.0.report.json",
      to: "manifests/example-v1.0.1.report.json",
    },
  ]);
});

test("planPackageRelease preserves upstream metadata and rewrites scripts", () => {
  const plan = planPackageRelease(
    {
      name: "gist",
      version: "14.0.0",
      manifests: ["manifests/gist-v14.0.0.translation-manifest.json"],
      provenance: {
        manifest: "manifests/gist-v14.0.0.translation-manifest.json",
      },
      assembly: {
        generatedArtifacts: [
          "manifests/gist-v14.0.0.translation-manifest.json",
          "manifests/gist-v14.0.0.ir-summary.json",
        ],
      },
      upstream: {
        repo: "https://github.com/semanticarts/gist",
        tag: "v14.0.0",
        commit: "6ab80c158a7fa56a1b5d3d824b125b92107e8f08",
      },
      scripts: {
        "parse:ir": "node tools/gist_to_typeql/parse_gist.mjs --out manifests/gist-v14.0.0.ir-summary.json",
        "refresh:package-contract": "npm run parse:ir && npm run emit:structural",
        "validate:bootstrap": "node tools/gist_to_typeql/validate_bootstrap.mjs",
        "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
      },
    },
    { bump: null, version: "1.0.0" }
  );

  assert.equal(plan.nextVersion, "1.0.0");

  // Upstream metadata is immutable provenance — never rewritten
  assert.equal(plan.nextPackageJson.upstream.tag, "v14.0.0");
  assert.equal(plan.nextPackageJson.upstream.repo, "https://github.com/semanticarts/gist");
  assert.equal(plan.nextPackageJson.upstream.commit, "6ab80c158a7fa56a1b5d3d824b125b92107e8f08");

  // Scripts with versioned paths are rewritten
  assert.equal(
    plan.nextPackageJson.scripts["parse:ir"],
    "node tools/gist_to_typeql/parse_gist.mjs --out manifests/gist-v1.0.0.ir-summary.json"
  );

  // Scripts without versioned paths are unchanged
  assert.equal(
    plan.nextPackageJson.scripts["validate:bootstrap"],
    "node tools/gist_to_typeql/validate_bootstrap.mjs"
  );

  // Manifests are rewritten
  assert.equal(plan.nextPackageJson.provenance.manifest, "manifests/gist-v1.0.0.translation-manifest.json");
});

test("planPackageRelease rewrites migration metadata for the next release", () => {
  const plan = planPackageRelease(
    {
      name: "vibemachine",
      version: "0.6.0",
      migration: {
        format: 1,
        supportsUpgradeFrom: ["0.5.x"],
        plans: [
          {
            id: "vibemachine-0.5.0-to-0.6.0",
            from: "0.5.0",
            to: "0.6.0",
            mode: "replace",
            snapshot: { required: true, label: "pre-vibemachine-0.6.0-migration" },
            phases: [
              {
                id: "preflight",
                units: [{ kind: "assert-data", path: "migrations/preflight/assert-v0.5.0-build.tql" }],
              },
              {
                id: "migrate",
                units: [{ kind: "write", path: "migrations/v0.5.0-to-v0.6.0.tql" }],
              },
              {
                id: "verify",
                units: [{ kind: "assert-data", path: "migrations/verify/assert-v0.6.0-build.tql" }],
              },
            ],
          },
        ],
      },
      scripts: {
        "refresh:package-contract": "node tools/package_contract/refresh_package_contract.mjs",
        "validate:bootstrap": "node tools/package_contract/validate_bootstrap.mjs",
        "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
      },
    },
    { bump: "minor", version: null }
  );

  assert.equal(plan.nextVersion, "0.7.0");
  assert.deepEqual(plan.nextPackageJson.migration.supportsUpgradeFrom, ["0.6.x"]);
  assert.equal(plan.nextPackageJson.migration.plans[0].id, "vibemachine-0.6.0-to-0.7.0");
  assert.equal(plan.nextPackageJson.migration.plans[0].from, "0.6.0");
  assert.equal(plan.nextPackageJson.migration.plans[0].to, "0.7.0");
  assert.equal(
    plan.nextPackageJson.migration.plans[0].snapshot.label,
    "pre-vibemachine-0.7.0-migration"
  );
  assert.deepEqual(plan.nextPackageJson.migration.plans[0].phases, [
    {
      id: "preflight",
      units: [{ kind: "assert-data", path: "migrations/preflight/assert-v0.6.0-build.tql" }],
    },
    {
      id: "migrate",
      units: [{ kind: "write", path: "migrations/v0.6.0-to-v0.7.0.tql" }],
    },
    {
      id: "verify",
      units: [{ kind: "assert-data", path: "migrations/verify/assert-v0.7.0-build.tql" }],
    },
  ]);
});

test("planPackageRelease supports resuming a release when the explicit version matches package.json", () => {
  const packageJson = {
    name: "gist",
    version: "1.0.3",
    manifests: ["manifests/gist-v1.0.3.translation-manifest.json"],
    provenance: {
      manifest: "manifests/gist-v1.0.3.translation-manifest.json",
    },
    assembly: {
      generatedArtifacts: ["manifests/gist-v1.0.3.translation-manifest.json"],
    },
    scripts: {
      "refresh:package-contract": "node tools/package_contract/refresh_package_contract.mjs",
      "validate:bootstrap": "node tools/package_contract/validate_bootstrap.mjs",
      "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
    },
  };

  const plan = planPackageRelease(packageJson, { bump: null, version: "1.0.3" });

  assert.equal(plan.currentVersion, "1.0.3");
  assert.equal(plan.nextVersion, "1.0.3");
  assert.equal(plan.resumeExistingVersion, true);
  assert.deepEqual(plan.renamePlan, []);
  assert.deepEqual(plan.nextPackageJson, packageJson);
});

async function createFixtureRepo(t, { withRemote = false, withMigration = false } = {}) {
  const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "ontology-release-"));
  t.after(async () => {
    await fs.rm(tempRoot, { recursive: true, force: true });
  });

  const repoPath = path.join(tempRoot, "repo");
  await fs.mkdir(path.join(repoPath, "tools", "package_contract"), { recursive: true });
  await fs.mkdir(path.join(repoPath, "manifests"), { recursive: true });
  await fs.mkdir(path.join(repoPath, "data"), { recursive: true });

  const packageJson = {
    name: "fixture-package",
    version: "1.0.0",
    schemas: [{ name: "fixture", file: "schema/fixture.tql" }],
    data: ["data/fixture-provenance.tql"],
    manifests: ["manifests/fixture-package-v1.0.0.package-manifest.json"],
    provenance: {
      manifest: "manifests/fixture-package-v1.0.0.package-manifest.json",
    },
    assembly: {
      generatedArtifacts: [
        "manifests/fixture-package-v1.0.0.package-manifest.json",
        "manifests/fixture-package-v1.0.0.report.json",
      ],
    },
    scripts: {
      "refresh:package-contract": "node tools/package_contract/refresh_package_contract.mjs",
      "validate:bootstrap": "node tools/package_contract/validate_bootstrap.mjs",
      "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
    },
  };

  if (withMigration) {
    packageJson.migration = {
      format: 1,
      supportsUpgradeFrom: ["1.0.x"],
      plans: [
        {
          id: "fixture-package-1.0.0-to-1.0.0",
          from: "1.0.0",
          to: "1.0.0",
          mode: "replace",
          snapshot: { required: true, label: "pre-fixture-package-1.0.0-migration" },
          phases: [
            {
              id: "preflight",
              units: [{ kind: "assert-data", path: "migrations/preflight/assert-v1.0.0-build.tql" }],
            },
            {
              id: "migrate",
              units: [{ kind: "write", path: "migrations/v1.0.0-to-v1.0.0.tql" }],
            },
            {
              id: "verify",
              units: [{ kind: "assert-data", path: "migrations/verify/assert-v1.0.0-build.tql" }],
            },
          ],
        },
      ],
    };
  }

  await fs.writeFile(path.join(repoPath, "package.json"), `${JSON.stringify(packageJson, null, 2)}\n`);
  await fs.mkdir(path.join(repoPath, "schema"), { recursive: true });
  await fs.writeFile(
    path.join(repoPath, "manifests", "fixture-package-v1.0.0.package-manifest.json"),
    `${JSON.stringify({ package: { name: "fixture-package", version: "1.0.0" } }, null, 2)}\n`
  );
  await fs.writeFile(path.join(repoPath, "manifests", "fixture-package-v1.0.0.report.json"), "{\n  \"report\": true\n}\n");
  await fs.writeFile(path.join(repoPath, "schema", "fixture.tql"), "define\nattribute docKey, value string;\nentity SchemaResource, owns docKey;\n");
  await fs.writeFile(
    path.join(repoPath, "data", "fixture-provenance.tql"),
    'put $r1 isa SchemaResource,\n  has docKey "fixture-build@1.0.0";\n'
  );

  await fs.writeFile(
    path.join(repoPath, "tools", "package_contract", "refresh_package_contract.mjs"),
    `import fs from "node:fs/promises";
const packageJson = JSON.parse(await fs.readFile("package.json", "utf8"));
const manifestPath = packageJson.provenance.manifest;
await fs.mkdir("manifests", { recursive: true });
await fs.writeFile(
  manifestPath,
  JSON.stringify({ package: { name: packageJson.name, version: packageJson.version } }, null, 2) + "\\n"
);
`
  );
  await fs.writeFile(
    path.join(repoPath, "tools", "package_contract", "validate_bootstrap.mjs"),
    `import fs from "node:fs/promises";
const packageJson = JSON.parse(await fs.readFile("package.json", "utf8"));
const manifest = JSON.parse(await fs.readFile(packageJson.provenance.manifest, "utf8"));
if (manifest.package.version !== packageJson.version) {
  throw new Error("manifest/package version mismatch");
}
console.log("bootstrap ok");
`
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

  let remotePath = null;
  if (withRemote) {
    remotePath = path.join(tempRoot, "remote.git");
    execFileSync("git", ["init", "--bare", remotePath], { stdio: "ignore" });
    execFileSync("git", ["remote", "add", "origin", remotePath], { cwd: repoPath, stdio: "ignore" });
    execFileSync("git", ["push", "-u", "origin", "main"], { cwd: repoPath, stdio: "ignore" });
  }

  return { repoPath, remotePath };
}

test("executeRelease dry-run reports the planned release without mutating the repo", async (t) => {
  const { repoPath } = await createFixtureRepo(t);
  const summary = await executeRelease({ repo: repoPath, bump: "patch", version: null, dryRun: true, push: false });

  assert.equal(summary.mode, "dry-run");
  assert.equal(summary.nextVersion, "1.0.1");
  const packageJson = JSON.parse(await fs.readFile(path.join(repoPath, "package.json"), "utf8"));
  assert.equal(packageJson.version, "1.0.0");
  const tags = execFileSync("git", ["tag", "--list"], { cwd: repoPath, encoding: "utf8" }).trim();
  assert.equal(tags, "");
});

test("executeRelease validate-only runs gates without creating a release commit or tag", async (t) => {
  const { repoPath } = await createFixtureRepo(t);
  const summary = await executeRelease({
    repo: repoPath,
    bump: null,
    version: null,
    dryRun: false,
    validateOnly: true,
    push: false,
  });

  assert.equal(summary.mode, "validate-only");
  const packageJson = JSON.parse(await fs.readFile(path.join(repoPath, "package.json"), "utf8"));
  assert.equal(packageJson.version, "1.0.0");
  const subject = execFileSync("git", ["log", "-1", "--pretty=%s"], { cwd: repoPath, encoding: "utf8" }).trim();
  assert.equal(subject, "Initial fixture");
  const status = execFileSync("git", ["status", "--porcelain"], { cwd: repoPath, encoding: "utf8" }).trim();
  assert.equal(status, "");
});

test("executeRelease performs version rewrite, refresh, validation, commit, and tag", async (t) => {
  const { repoPath } = await createFixtureRepo(t);
  const summary = await executeRelease({ repo: repoPath, bump: "patch", version: null, dryRun: false, push: false });

  assert.equal(summary.mode, "release");
  assert.equal(summary.nextVersion, "1.0.1");
  const packageJson = JSON.parse(await fs.readFile(path.join(repoPath, "package.json"), "utf8"));
  assert.equal(packageJson.version, "1.0.1");
  await assert.doesNotReject(fs.access(path.join(repoPath, "manifests", "fixture-package-v1.0.1.package-manifest.json")));
  await assert.rejects(fs.access(path.join(repoPath, "manifests", "fixture-package-v1.0.0.package-manifest.json")));
  await assert.doesNotReject(fs.access(path.join(repoPath, "manifests", "fixture-package-v1.0.1.report.json")));

  const tags = execFileSync("git", ["tag", "--list"], { cwd: repoPath, encoding: "utf8" }).trim();
  assert.equal(tags, "v1.0.1");
  const subject = execFileSync("git", ["log", "-1", "--pretty=%s"], { cwd: repoPath, encoding: "utf8" }).trim();
  assert.equal(subject, "Release fixture-package v1.0.1");
});

test("executeRelease can resume a missing tag for an existing release commit", async (t) => {
  const { repoPath } = await createFixtureRepo(t);
  await executeRelease({ repo: repoPath, bump: "patch", version: null, dryRun: false, push: false });
  execFileSync("git", ["tag", "-d", "v1.0.1"], { cwd: repoPath, stdio: "ignore" });

  const summary = await executeRelease({ repo: repoPath, bump: null, version: "1.0.1", dryRun: false, push: false });

  assert.equal(summary.mode, "release");
  assert.equal(summary.nextVersion, "1.0.1");
  assert.equal(summary.resumeExistingVersion, true);
  const tags = execFileSync("git", ["tag", "--list"], { cwd: repoPath, encoding: "utf8" }).trim();
  assert.equal(tags, "v1.0.1");
  const subject = execFileSync("git", ["log", "-1", "--pretty=%s"], { cwd: repoPath, encoding: "utf8" }).trim();
  assert.equal(subject, "Release fixture-package v1.0.1");
});

test("executeRelease refuses to resume a missing tag when HEAD is not the release commit", async (t) => {
  const { repoPath } = await createFixtureRepo(t);
  const packageJsonPath = path.join(repoPath, "package.json");
  const packageJson = JSON.parse(await fs.readFile(packageJsonPath, "utf8"));
  packageJson.version = "1.0.1";
  await fs.writeFile(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`);
  execFileSync("git", ["add", "package.json"], { cwd: repoPath, stdio: "ignore" });
  execFileSync("git", ["commit", "-m", "Bump version manually"], { cwd: repoPath, stdio: "ignore" });

  await assert.rejects(
    executeRelease({ repo: repoPath, bump: null, version: "1.0.1", dryRun: false, push: false }),
    /Cannot resume release/
  );
});

test("executeRelease pushes the release commit and tag when push is enabled", async (t) => {
  const { repoPath, remotePath } = await createFixtureRepo(t, { withRemote: true });
  const summary = await executeRelease({ repo: repoPath, bump: "patch", version: null, dryRun: false, push: true });

  assert.equal(summary.nextVersion, "1.0.1");
  const remoteHeads = execFileSync("git", ["--git-dir", remotePath, "show-ref", "--heads", "--tags"], {
    encoding: "utf8",
  });
  assert.match(remoteHeads, /refs\/heads\/main/);
  assert.match(remoteHeads, /refs\/tags\/v1\.0\.1/);
});

test("executeRelease updates migration metadata and emits migration assertion files", async (t) => {
  const { repoPath } = await createFixtureRepo(t, { withMigration: true });
  const summary = await executeRelease({ repo: repoPath, bump: "patch", version: null, dryRun: false, push: false });

  assert.equal(summary.nextVersion, "1.0.1");
  assert.equal(summary.migrationDiff, "migrations/v1.0.0-to-v1.0.1.tql");

  const packageJson = JSON.parse(await fs.readFile(path.join(repoPath, "package.json"), "utf8"));
  assert.deepEqual(packageJson.migration.supportsUpgradeFrom, ["1.0.x"]);
  assert.equal(packageJson.migration.plans[0].id, "fixture-package-1.0.0-to-1.0.1");
  assert.equal(packageJson.migration.plans[0].to, "1.0.1");
  assert.equal(
    packageJson.migration.plans[0].phases[0].units[0].path,
    "migrations/preflight/assert-v1.0.0-build.tql"
  );
  assert.equal(
    packageJson.migration.plans[0].phases[1].units[0].path,
    "migrations/v1.0.0-to-v1.0.1.tql"
  );
  assert.equal(
    packageJson.migration.plans[0].phases[2].units[0].path,
    "migrations/verify/assert-v1.0.1-build.tql"
  );

  await assert.doesNotReject(fs.access(path.join(repoPath, "migrations", "preflight", "assert-v1.0.0-build.tql")));
  await assert.doesNotReject(fs.access(path.join(repoPath, "migrations", "v1.0.0-to-v1.0.1.tql")));
  await assert.doesNotReject(fs.access(path.join(repoPath, "migrations", "verify", "assert-v1.0.1-build.tql")));
});
