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

async function createFixtureRepo(t, { withRemote = false } = {}) {
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

  await fs.writeFile(path.join(repoPath, "package.json"), `${JSON.stringify(packageJson, null, 2)}\n`);
  await fs.writeFile(
    path.join(repoPath, "manifests", "fixture-package-v1.0.0.package-manifest.json"),
    `${JSON.stringify({ package: { name: "fixture-package", version: "1.0.0" } }, null, 2)}\n`
  );
  await fs.writeFile(path.join(repoPath, "manifests", "fixture-package-v1.0.0.report.json"), "{\n  \"report\": true\n}\n");
  await fs.writeFile(path.join(repoPath, "data", "fixture-provenance.tql"), "# fixture\n");

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
