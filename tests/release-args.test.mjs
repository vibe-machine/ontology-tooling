import test from "node:test";
import assert from "node:assert/strict";

import { parseReleaseArgs, validateReleaseArgs } from "../src/lib/release-args.mjs";

test("parseReleaseArgs parses dry-run bump flow", () => {
  const options = parseReleaseArgs(["--repo", "../ontology-beads", "--bump", "patch", "--dry-run"]);
  assert.deepEqual(options, {
    repo: "../ontology-beads",
    bump: "patch",
    version: null,
    dryRun: true,
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
