# Repo Wrapper Pattern

Ontology repos should keep only thin local wrappers around shared tooling.

## Shared Tooling Owns

- release version planning
- package version rewrites
- manifest path rewrites
- refresh and validation orchestration
- release commit and git tag flow

## Repo-Local Tooling Owns

- package-specific schema generation
- package-specific provenance/doc generation
- package-specific validation internals
- any ontology-specific refresh logic

## Required Repo Scripts

Each ontology repo should expose these stable scripts:

```json
{
  "scripts": {
    "refresh:package-contract": "node tools/package_contract/refresh_package_contract.mjs",
    "validate:bootstrap": "node tools/package_contract/validate_bootstrap.mjs",
    "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
    "release": "../ontology-tooling/bin/ontology-release --repo .",
    "release:check": "../ontology-tooling/bin/ontology-release --repo . --validate-only"
  }
}
```

If a repo cannot support `refresh:package-contract` yet, it is not ready for the shared `ontology-release` flow.

## Boundary Rule

Do not copy release orchestration into ontology repos.

Ontology repos may keep local `tools/package_contract/*` generators and validators, but they should invoke the shared release command for version/tag automation rather than adding another bespoke release script.
