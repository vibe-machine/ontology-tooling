export function parseReleaseArgs(argv) {
  const options = {
    repo: null,
    bump: null,
    version: null,
    dryRun: false,
    push: true,
    help: false,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];

    if (arg === "--help" || arg === "-h") {
      options.help = true;
      continue;
    }
    if (arg === "--dry-run") {
      options.dryRun = true;
      continue;
    }
    if (arg === "--no-push") {
      options.push = false;
      continue;
    }
    if (arg === "--repo") {
      options.repo = argv[index + 1] ?? null;
      index += 1;
      continue;
    }
    if (arg === "--bump") {
      options.bump = argv[index + 1] ?? null;
      index += 1;
      continue;
    }
    if (arg === "--version") {
      options.version = argv[index + 1] ?? null;
      index += 1;
      continue;
    }

    throw new Error(`Unknown argument: ${arg}`);
  }

  return options;
}

export function validateReleaseArgs(options) {
  if (options.help) return;

  if (!options.repo) {
    throw new Error("Missing required --repo argument");
  }

  if (options.bump && options.version) {
    throw new Error("Use either --bump or --version, not both");
  }

  if (!options.bump && !options.version) {
    throw new Error("A release requires either --bump <patch|minor|major> or --version <x.y.z>");
  }
}
