export function parseSemver(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!match) {
    throw new Error(`Unsupported version format: ${version}`);
  }

  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
  };
}

export function bumpVersion(version, bump) {
  const parsed = parseSemver(version);

  if (bump === "major") {
    return `${parsed.major + 1}.0.0`;
  }
  if (bump === "minor") {
    return `${parsed.major}.${parsed.minor + 1}.0`;
  }
  if (bump === "patch") {
    return `${parsed.major}.${parsed.minor}.${parsed.patch + 1}`;
  }

  throw new Error(`Unsupported bump kind: ${bump}`);
}

export function resolveReleaseVersion(currentVersion, options) {
  if (options.version) {
    parseSemver(options.version);
    return options.version;
  }

  return bumpVersion(currentVersion, options.bump);
}

