"""
Static and integration tests for ontology package bootstrap uniqueness.

Catches the class of bug where applying a package's bootstrap plan to a fresh
TypeDB database fails with @unique / @key constraint violations because:
- A seed data file uses `insert` instead of `put` for entities with @key attrs
- Two data files in the same bootstrap plan create entities with the same @key value
- A single data file contains duplicate @key values

Static tests (no TypeDB required):
- Parse package manifests and TQL data files
- Extract @key attribute values from `put` and `insert` statements
- Check for duplicates within and across files in the same bootstrap plan
- Verify that entities with @key constraints always use `put` (not `insert`)

Integration tests (require TypeDB):
- Apply full bootstrap plans and verify no constraint violations
- Verify idempotency: re-applying the same plan succeeds
"""

import json
import os
import re
import uuid
from collections import defaultdict
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

# Root of the collection-vibe-machine sibling repos
REPOS_ROOT = Path(__file__).resolve().parent.parent.parent

# All known ontology package repos (in dependency order)
PACKAGE_REPOS = [
    REPOS_ROOT / "ontology-gist",
    REPOS_ROOT / "ontology-beads",
    REPOS_ROOT / "ontology-gastown",
]


# ---------------------------------------------------------------------------
# TQL Parsing Helpers
# ---------------------------------------------------------------------------


def _load_package_manifest(repo_root: Path) -> dict:
    """Load and return the package.json manifest from a repo root."""
    manifest_path = repo_root / "package.json"
    assert manifest_path.exists(), f"package.json not found: {manifest_path}"
    return json.loads(manifest_path.read_text(encoding="utf-8"))


def _get_assembly_load_order(manifest: dict) -> list[str]:
    """Return the assembly loadOrder from a package manifest."""
    return manifest.get("assembly", {}).get("loadOrder", [])


def _get_schema_files(manifest: dict) -> set[str]:
    """Return the set of schema file paths from a package manifest."""
    return {s["file"] for s in manifest.get("schemas", [])}


def _classify_load_order(manifest: dict) -> list[dict]:
    """Classify each entry in loadOrder as 'schema' or 'data'."""
    schema_files = _get_schema_files(manifest)
    result = []
    for path in _get_assembly_load_order(manifest):
        kind = "schema" if path in schema_files else "data"
        result.append({"path": path, "kind": kind})
    return result


def _read_tql(repo_root: Path, relative_path: str) -> str:
    """Read a TQL file from a repo."""
    path = repo_root / relative_path
    assert path.exists(), f"TQL file not found: {path}"
    return path.read_text(encoding="utf-8")


def _extract_key_attrs_from_schema(tql: str) -> dict[str, list[str]]:
    """Extract entity types and their @key attribute names from a schema TQL.

    Returns a dict mapping entity type name to list of attribute names
    declared with @key.

    Example schema fragment:
        entity OntologyPackageBuild,
            owns buildKey @key,
            owns packageName;

    Returns: {"OntologyPackageBuild": ["buildKey"]}
    """
    result: dict[str, list[str]] = {}

    # Match entity declarations. The body of an entity declaration ends at
    # the next top-level keyword (entity, relation, attribute, or a type
    # name at column 0 that plays a role) or at a semicolon followed by
    # a blank line / next declaration.
    # Strategy: split on "entity " at the start of a line, then parse each block.
    # We use a regex that captures from "entity Name" through the next semicolon.
    entity_pattern = re.compile(
        r"^entity\s+(\w+)(?:\s+sub\s+\w+)?\s*,?\s*\n(.*?);",
        re.MULTILINE | re.DOTALL,
    )
    key_attr_pattern = re.compile(r"owns\s+(\w+)\s+@key")

    for match in entity_pattern.finditer(tql):
        entity_name = match.group(1)
        body = match.group(2)
        key_attrs = key_attr_pattern.findall(body)
        if key_attrs:
            result[entity_name] = key_attrs

    return result


def _extract_put_key_values(tql: str, entity_type: str, key_attr: str) -> list[str]:
    """Extract values of a @key attribute from put statements for a given entity type.

    Parses statements like:
        put $build isa OntologyPackageBuild,
          has buildKey "gist@14.0.0:abc123",
          ...;

    Returns list of the quoted string values for the key attribute.
    """
    values = []
    # Pattern: put ... isa <entity_type>, ... has <key_attr> "<value>" ...;
    # We need to match across multiple lines until the semicolon
    put_pattern = re.compile(
        rf'put\s+\$\w+\s+isa\s+{re.escape(entity_type)}\s*,'
        rf'(.*?);',
        re.DOTALL,
    )
    value_pattern = re.compile(
        rf'has\s+{re.escape(key_attr)}\s+"([^"]*)"'
    )

    for put_match in put_pattern.finditer(tql):
        body = put_match.group(1)
        val_match = value_pattern.search(body)
        if val_match:
            values.append(val_match.group(1))

    return values


def _extract_insert_key_values(tql: str, entity_type: str, key_attr: str) -> list[str]:
    """Extract values of a @key attribute from insert statements for a given entity type.

    Same as _extract_put_key_values but for `insert` statements.
    """
    values = []
    insert_pattern = re.compile(
        rf'insert\s+\$\w+\s+isa\s+{re.escape(entity_type)}\s*,'
        rf'(.*?);',
        re.DOTALL,
    )
    value_pattern = re.compile(
        rf'has\s+{re.escape(key_attr)}\s+"([^"]*)"'
    )

    for insert_match in insert_pattern.finditer(tql):
        body = insert_match.group(1)
        val_match = value_pattern.search(body)
        if val_match:
            values.append(val_match.group(1))

    return values


def _find_insert_statements_for_keyed_entities(
    tql: str, keyed_entities: dict[str, list[str]]
) -> list[dict]:
    """Find insert statements that create entities which have @key constraints.

    These are bugs: entities with @key attrs should use `put` (idempotent)
    not `insert` (which fails on re-application).

    Returns a list of dicts with entity_type, key_attr, and the matched value.
    """
    violations = []
    for entity_type, key_attrs in keyed_entities.items():
        for key_attr in key_attrs:
            values = _extract_insert_key_values(tql, entity_type, key_attr)
            for value in values:
                violations.append({
                    "entity_type": entity_type,
                    "key_attr": key_attr,
                    "value": value,
                })
    return violations


# ---------------------------------------------------------------------------
# Collect keyed entity definitions across all schema files for a package
# ---------------------------------------------------------------------------


def _collect_keyed_entities_for_package(
    repo_root: Path, manifest: dict
) -> dict[str, list[str]]:
    """Collect all entity types with @key attributes from a package's schemas."""
    keyed: dict[str, list[str]] = {}
    for entry in _classify_load_order(manifest):
        if entry["kind"] != "schema":
            continue
        tql = _read_tql(repo_root, entry["path"])
        keyed.update(_extract_key_attrs_from_schema(tql))
    return keyed


# ---------------------------------------------------------------------------
# Resolve full multi-package dependency chain
# ---------------------------------------------------------------------------


def _resolve_package_chain() -> list[tuple[Path, dict]]:
    """Resolve package dependency order and return (repo_root, manifest) pairs.

    Uses the known PACKAGE_REPOS list which is already in dependency order.
    """
    chain = []
    for repo_root in PACKAGE_REPOS:
        if not (repo_root / "package.json").exists():
            continue
        manifest = _load_package_manifest(repo_root)
        chain.append((repo_root, manifest))
    return chain


# ---------------------------------------------------------------------------
# Static Tests (no TypeDB required)
# ---------------------------------------------------------------------------


class TestBootstrapUniquenessStatic:
    """Static analysis of TQL data files for @key uniqueness violations."""

    @pytest.fixture(autouse=True)
    def _setup(self):
        """Ensure at least one package repo exists."""
        available = [r for r in PACKAGE_REPOS if (r / "package.json").exists()]
        if not available:
            pytest.skip("No ontology package repos found")

    def test_no_duplicate_key_values_within_single_data_file(self):
        """Each data file must not contain duplicate @key values for the same entity type.

        A file that puts two OntologyPackageBuild entities with the same
        buildKey value would fail on the second put (even though put is
        idempotent for the same data, if the same key appears with different
        attribute values it creates confusion).
        """
        for repo_root, manifest in _resolve_package_chain():
            keyed_entities = _collect_keyed_entities_for_package(repo_root, manifest)
            if not keyed_entities:
                continue

            for entry in _classify_load_order(manifest):
                if entry["kind"] != "data":
                    continue

                tql = _read_tql(repo_root, entry["path"])

                for entity_type, key_attrs in keyed_entities.items():
                    for key_attr in key_attrs:
                        values = _extract_put_key_values(tql, entity_type, key_attr)
                        seen: dict[str, int] = {}
                        duplicates = []
                        for v in values:
                            seen[v] = seen.get(v, 0) + 1
                            if seen[v] == 2:
                                duplicates.append(v)

                        assert not duplicates, (
                            f"Duplicate @key values in {entry['path']} "
                            f"(package: {manifest['name']}): "
                            f"{entity_type}.{key_attr} has duplicates: {duplicates}"
                        )

    def test_no_duplicate_key_values_across_data_files_in_same_package(self):
        """No two data files in the same package's bootstrap plan should create
        entities with the same @key attribute value.

        For example, if gistCore-domain-seeds.tql and gistCore-provenance.tql
        both try to create an OntologyPackageBuild with the same buildKey,
        the second file would fail with a @unique constraint violation.
        """
        for repo_root, manifest in _resolve_package_chain():
            keyed_entities = _collect_keyed_entities_for_package(repo_root, manifest)
            if not keyed_entities:
                continue

            # Collect all key values across all data files, tracking source
            for entity_type, key_attrs in keyed_entities.items():
                for key_attr in key_attrs:
                    # Map: value -> list of files that create it
                    value_sources: dict[str, list[str]] = defaultdict(list)

                    for entry in _classify_load_order(manifest):
                        if entry["kind"] != "data":
                            continue
                        tql = _read_tql(repo_root, entry["path"])
                        values = _extract_put_key_values(tql, entity_type, key_attr)
                        for v in values:
                            value_sources[v].append(entry["path"])

                    conflicts = {
                        v: sources
                        for v, sources in value_sources.items()
                        if len(sources) > 1
                    }
                    assert not conflicts, (
                        f"Cross-file @key conflicts in package '{manifest['name']}': "
                        f"{entity_type}.{key_attr} has values created by multiple files: "
                        + "; ".join(
                            f'"{v}" in {sources}' for v, sources in conflicts.items()
                        )
                    )

    def test_no_duplicate_key_values_across_packages_in_dependency_chain(self):
        """When packages are loaded together (gist -> beads -> gastown),
        no two packages should create entities with the same @key value.

        Each package's OntologyPackageBuild should have a unique buildKey
        (e.g., "gist@14.0.0:..." vs "beads@0.59.3:...").
        """
        chain = _resolve_package_chain()
        if len(chain) < 2:
            pytest.skip("Need at least 2 packages to check cross-package conflicts")

        # Collect all keyed entities across the entire chain
        all_keyed: dict[str, list[str]] = {}
        for repo_root, manifest in chain:
            all_keyed.update(
                _collect_keyed_entities_for_package(repo_root, manifest)
            )

        for entity_type, key_attrs in all_keyed.items():
            for key_attr in key_attrs:
                # Map: value -> list of (package_name, file_path)
                value_sources: dict[str, list[tuple[str, str]]] = defaultdict(list)

                for repo_root, manifest in chain:
                    for entry in _classify_load_order(manifest):
                        if entry["kind"] != "data":
                            continue
                        tql = _read_tql(repo_root, entry["path"])
                        values = _extract_put_key_values(tql, entity_type, key_attr)
                        for v in values:
                            value_sources[v].append(
                                (manifest["name"], entry["path"])
                            )

                conflicts = {
                    v: sources
                    for v, sources in value_sources.items()
                    if len(set(pkg for pkg, _ in sources)) > 1
                }
                assert not conflicts, (
                    f"Cross-package @key conflicts: "
                    f"{entity_type}.{key_attr} has values created by multiple packages: "
                    + "; ".join(
                        f'"{v}" in {sources}' for v, sources in conflicts.items()
                    )
                )

    def test_keyed_entities_use_put_not_insert(self):
        """Data files must use `put` (not `insert`) for entities with @key constraints.

        In TypeQL 3.x, `put` is idempotent (find-or-create) while `insert`
        fails if the entity already exists. Using `insert` for keyed entities
        means the bootstrap plan cannot be re-applied to an existing database
        and will fail with @unique constraint violations if another unit in
        the same plan already created that entity.
        """
        for repo_root, manifest in _resolve_package_chain():
            keyed_entities = _collect_keyed_entities_for_package(repo_root, manifest)
            if not keyed_entities:
                continue

            for entry in _classify_load_order(manifest):
                if entry["kind"] != "data":
                    continue

                tql = _read_tql(repo_root, entry["path"])
                violations = _find_insert_statements_for_keyed_entities(
                    tql, keyed_entities
                )

                assert not violations, (
                    f"Data file {entry['path']} (package: {manifest['name']}) "
                    f"uses `insert` instead of `put` for entities with @key constraints. "
                    f"This will cause @unique violations on re-application. "
                    f"Violations: {violations}"
                )

    def test_every_package_has_assembly_load_order(self):
        """Every package must declare an assembly.loadOrder in package.json.

        Without loadOrder, the bootstrap plan execution order is undefined
        and schemas may be applied after data, causing failures.
        """
        for repo_root, manifest in _resolve_package_chain():
            load_order = _get_assembly_load_order(manifest)
            assert load_order, (
                f"Package '{manifest['name']}' at {repo_root} "
                f"has no assembly.loadOrder in package.json"
            )

    def test_schemas_precede_data_in_load_order(self):
        """In each package's loadOrder, all schema files must come before data files.

        Applying data before the schema that defines its types will fail.
        """
        for repo_root, manifest in _resolve_package_chain():
            classified = _classify_load_order(manifest)
            if not classified:
                continue

            seen_data = False
            for entry in classified:
                if entry["kind"] == "data":
                    seen_data = True
                elif entry["kind"] == "schema" and seen_data:
                    assert False, (
                        f"Package '{manifest['name']}': schema file "
                        f"'{entry['path']}' appears after data file in loadOrder. "
                        f"All schemas must be applied before data."
                    )

    def test_all_load_order_files_exist(self):
        """Every file referenced in assembly.loadOrder must exist on disk."""
        for repo_root, manifest in _resolve_package_chain():
            for entry in _classify_load_order(manifest):
                file_path = repo_root / entry["path"]
                assert file_path.exists(), (
                    f"Package '{manifest['name']}': file '{entry['path']}' "
                    f"referenced in loadOrder does not exist at {file_path}"
                )

    def test_provenance_build_keys_are_package_scoped(self):
        """Each package's OntologyPackageBuild buildKey should contain the package name.

        Convention: buildKey format is "<packageName>@<version>:<commit>".
        This ensures no accidental collisions across packages.
        """
        for repo_root, manifest in _resolve_package_chain():
            keyed_entities = _collect_keyed_entities_for_package(repo_root, manifest)
            if "OntologyPackageBuild" not in keyed_entities:
                continue

            package_name = manifest["name"]
            for entry in _classify_load_order(manifest):
                if entry["kind"] != "data":
                    continue

                tql = _read_tql(repo_root, entry["path"])
                build_keys = _extract_put_key_values(
                    tql, "OntologyPackageBuild", "buildKey"
                )
                for bk in build_keys:
                    assert bk.startswith(f"{package_name}@"), (
                        f"Package '{package_name}': OntologyPackageBuild buildKey "
                        f'"{bk}" does not start with "{package_name}@". '
                        f"Build keys must be scoped to the package name to avoid "
                        f"cross-package collisions."
                    )


# ---------------------------------------------------------------------------
# Integration Tests (require TypeDB)
# ---------------------------------------------------------------------------

TYPEDB_ADDRESS = os.environ.get("TYPEDB_ADDRESS", "192.168.66.2:1729")
TYPEDB_USER = os.environ.get("TYPEDB_USER", "admin")
TYPEDB_PASS = os.environ.get("TYPEDB_PASS", "password")


def _ephemeral_db_name(prefix: str = "pytest_uniq") -> str:
    short = uuid.uuid4().hex[:8]
    return f"{prefix}_{short}"


def _split_tql_statements(tql: str) -> list[str]:
    """Split a TQL file with multiple statements into individual queries.

    Handles put, match-insert, and standalone insert blocks.
    Each statement is delimited by a keyword at the start of a line
    (put, match, insert) after stripping comments.
    """
    queries: list[str] = []
    current = ""
    for line in tql.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if (
            stripped.startswith("put ")
            or stripped.startswith("match")
            or stripped.startswith("insert ")
        ) and current:
            finished = current.strip()
            if finished:
                queries.append(finished)
            current = line
        else:
            current += ("" if not current else "\n") + line
    remaining = current.strip()
    if remaining:
        queries.append(remaining)
    return queries


@pytest.fixture(scope="module")
def typedb_driver():
    """Create a TypeDB driver connection (module-scoped)."""
    try:
        from typedb.driver import Credentials, DriverOptions, TransactionType, TypeDB
    except ImportError:
        pytest.skip("typedb-driver not installed")

    creds = Credentials(TYPEDB_USER, TYPEDB_PASS)
    opts = DriverOptions(is_tls_enabled=False)
    try:
        d = TypeDB.driver(TYPEDB_ADDRESS, creds, opts)
    except Exception as exc:
        pytest.skip(f"Cannot connect to TypeDB at {TYPEDB_ADDRESS}: {exc}")
    yield d
    d.close()


@pytest.fixture()
def ephemeral_db(typedb_driver):
    """Create an ephemeral database, yield its name, delete on teardown."""
    db = _ephemeral_db_name()
    typedb_driver.databases.create(db)
    yield db
    if typedb_driver.databases.contains(db):
        typedb_driver.databases.get(db).delete()


def _apply_package_bootstrap(
    driver, db: str, repo_root: Path, manifest: dict
) -> None:
    """Apply a single package's full bootstrap plan (schemas then data)."""
    from typedb.driver import TransactionType

    classified = _classify_load_order(manifest)

    for entry in classified:
        tql = _read_tql(repo_root, entry["path"])
        if entry["kind"] == "schema":
            with driver.transaction(db, TransactionType.SCHEMA) as tx:
                tx.query(tql).resolve()
                tx.commit()
        else:
            statements = _split_tql_statements(tql)
            for stmt in statements:
                with driver.transaction(db, TransactionType.WRITE) as tx:
                    tx.query(stmt).resolve()
                    tx.commit()


@pytest.mark.typedb
class TestBootstrapUniquenessIntegration:
    """Integration tests that apply bootstrap plans to a real TypeDB instance."""

    def test_single_package_bootstrap_no_constraint_violations(
        self, typedb_driver, ephemeral_db
    ):
        """Each package's bootstrap plan must apply cleanly to a fresh database.

        This is the core test for the reported bug: applying a package's
        bootstrap should not produce @unique constraint violations.

        Tests each package independently with its dependency chain.
        """
        chain = _resolve_package_chain()
        for repo_root, manifest in chain:
            # Apply this package (and implicitly its dependencies that were
            # already applied earlier in the chain since we iterate in order)
            _apply_package_bootstrap(
                typedb_driver, ephemeral_db, repo_root, manifest
            )

    def test_full_chain_bootstrap_idempotent(
        self, typedb_driver, ephemeral_db
    ):
        """Re-applying the entire bootstrap chain must not produce errors.

        Since all data files use `put` (idempotent), the second application
        should be a no-op.
        """
        chain = _resolve_package_chain()

        # First pass
        for repo_root, manifest in chain:
            _apply_package_bootstrap(
                typedb_driver, ephemeral_db, repo_root, manifest
            )

        # Second pass -- must not fail
        for repo_root, manifest in chain:
            _apply_package_bootstrap(
                typedb_driver, ephemeral_db, repo_root, manifest
            )

    def test_provenance_records_queryable_after_bootstrap(
        self, typedb_driver, ephemeral_db
    ):
        """After bootstrap, each package's OntologyPackageBuild must be queryable."""
        from typedb.driver import TransactionType

        chain = _resolve_package_chain()
        for repo_root, manifest in chain:
            _apply_package_bootstrap(
                typedb_driver, ephemeral_db, repo_root, manifest
            )

        # Verify each package has exactly one OntologyPackageBuild
        with typedb_driver.transaction(
            ephemeral_db, TransactionType.READ
        ) as tx:
            rows = list(
                tx.query(
                    "match $b isa OntologyPackageBuild; select $b;"
                )
                .resolve()
                .as_concept_rows()
            )
            package_names_in_chain = [m["name"] for _, m in chain]
            assert len(rows) == len(package_names_in_chain), (
                f"Expected {len(package_names_in_chain)} OntologyPackageBuild "
                f"records, got {len(rows)}"
            )


# ---------------------------------------------------------------------------
# Unit tests for the TQL parsing helpers themselves
# ---------------------------------------------------------------------------


class TestTqlParsingHelpers:
    """Verify that TQL parsing helpers correctly extract key information."""

    def test_extract_key_attrs_from_schema(self):
        schema = """define

entity OntologyPackageBuild,
    owns buildKey @key,
    owns packageName,
    owns packageVersion;

entity SchemaModule,
    owns moduleKey @key,
    owns moduleName;

entity SchemaResource,
    owns docKey @key,
    owns typeLabel;

entity Aspect sub Thing,
    owns name,
    owns description;
"""
        result = _extract_key_attrs_from_schema(schema)
        assert "OntologyPackageBuild" in result
        assert result["OntologyPackageBuild"] == ["buildKey"]
        assert "SchemaModule" in result
        assert result["SchemaModule"] == ["moduleKey"]
        assert "SchemaResource" in result
        assert result["SchemaResource"] == ["docKey"]
        # Aspect has no @key attrs
        assert "Aspect" not in result

    def test_extract_put_key_values(self):
        tql = '''
put $build isa OntologyPackageBuild,
  has buildKey "gist@14.0.0:abc123",
  has packageName "gist",
  has packageVersion "14.0.0";

put $s1 isa SourceArtifactRecord,
  has sourcePath "schema/gistCore.tql",
  has sourceSha256 "deadbeef";
'''
        values = _extract_put_key_values(tql, "OntologyPackageBuild", "buildKey")
        assert values == ["gist@14.0.0:abc123"]

        # No OntologyPackageBuild with sourcePath key
        values2 = _extract_put_key_values(tql, "OntologyPackageBuild", "sourcePath")
        assert values2 == []

    def test_extract_put_key_values_multiple(self):
        tql = '''
put $m1 isa SchemaModule,
  has moduleKey "gist-core",
  has moduleName "gistCore";

put $m2 isa SchemaModule,
  has moduleKey "beads",
  has moduleName "beads";
'''
        values = _extract_put_key_values(tql, "SchemaModule", "moduleKey")
        assert values == ["gist-core", "beads"]

    def test_detect_insert_for_keyed_entity(self):
        tql = '''
insert $build isa OntologyPackageBuild,
  has buildKey "gist@14.0.0:abc123",
  has packageName "gist";
'''
        keyed = {"OntologyPackageBuild": ["buildKey"]}
        violations = _find_insert_statements_for_keyed_entities(tql, keyed)
        assert len(violations) == 1
        assert violations[0]["entity_type"] == "OntologyPackageBuild"
        assert violations[0]["value"] == "gist@14.0.0:abc123"

    def test_put_for_keyed_entity_is_not_violation(self):
        tql = '''
put $build isa OntologyPackageBuild,
  has buildKey "gist@14.0.0:abc123",
  has packageName "gist";
'''
        keyed = {"OntologyPackageBuild": ["buildKey"]}
        violations = _find_insert_statements_for_keyed_entities(tql, keyed)
        assert len(violations) == 0

    def test_split_tql_statements(self):
        tql = '''# comment
put $a isa Foo,
  has name "bar";

put $b isa Foo,
  has name "baz";
'''
        stmts = _split_tql_statements(tql)
        assert len(stmts) == 2
        assert 'has name "bar"' in stmts[0]
        assert 'has name "baz"' in stmts[1]

    def test_split_tql_match_insert(self):
        tql = '''match
  $d isa Domain, has name "gist";
  $doc isa TypeDoc, has typeLabel "Event";
insert
  (categorized: $doc, category: $d) isa isCategorizedBy;

match
  $d isa Domain, has name "gist";
  $doc isa TypeDoc, has typeLabel "Task";
insert
  (categorized: $doc, category: $d) isa isCategorizedBy;
'''
        stmts = _split_tql_statements(tql)
        assert len(stmts) == 2

    def test_duplicate_detection_within_file(self):
        """Simulates the case where a file has duplicate buildKey values."""
        tql = '''
put $b1 isa OntologyPackageBuild,
  has buildKey "pkg@1.0:abc",
  has packageName "pkg";

put $b2 isa OntologyPackageBuild,
  has buildKey "pkg@1.0:abc",
  has packageName "pkg";
'''
        values = _extract_put_key_values(tql, "OntologyPackageBuild", "buildKey")
        assert len(values) == 2
        assert values[0] == values[1]
        # This is the duplicate we want to catch
        seen: dict[str, int] = {}
        duplicates = []
        for v in values:
            seen[v] = seen.get(v, 0) + 1
            if seen[v] == 2:
                duplicates.append(v)
        assert duplicates == ["pkg@1.0:abc"]
