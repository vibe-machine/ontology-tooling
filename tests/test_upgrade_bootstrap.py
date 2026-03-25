"""
Integration test: Delete All Types -> re-apply reproduces @key constraint violation.

Simulates the exact flow the Cannon UI performs:
1. Apply all bootstrap units (schemas via schema tx, data via write tx)
2. Delete and recreate the database ("Delete All Types")
3. Re-apply all bootstrap units in the same order

The second apply of gistCore-provenance.tql was expected to fail with a
@key constraint violation on OntologyPackageBuild.buildKey.

Root cause hypothesis: gistCore-provenance.tql contains multiple `put`
statements where `$build` is defined in the first statement and then
referenced in subsequent relation `put` statements (e.g.
`put (build: $build, sourceArtifact: $s1) isa buildHasSourceArtifact;`).
TypeQL 3.x may not carry variable bindings across separate `put`
statements within a single query, so the relation `put` attempts to
create a second OntologyPackageBuild, violating the @key constraint.

FINDING: The Python TypeDB driver (typedb-driver >= 2.29.0) handles
multi-statement `put` queries with cross-statement variable references
correctly. Both fresh install and delete-all-types -> re-apply succeed
without constraint violations. If the bug is real, it is specific to
the Swift/C TypeDB driver or something else in the Swift app's flow.

Critical detail: the Swift app sends the ENTIRE file content as a single
query in one transaction -- it does NOT split into individual statements.
This test replicates that exact behavior.

Requires: TypeDB running at TYPEDB_ADDRESS (default: 192.168.66.2:1729)
"""

import json
import os
import re
import uuid
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

TYPEDB_ADDRESS = os.environ.get("TYPEDB_ADDRESS", "192.168.66.2:1729")
TYPEDB_USER = os.environ.get("TYPEDB_USER", "admin")
TYPEDB_PASS = os.environ.get("TYPEDB_PASS", "password")

# The ontology-gist repo lives alongside ontology-tooling
GIST_REPO = Path(__file__).resolve().parent.parent.parent / "ontology-gist"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _ephemeral_db_name() -> str:
    return f"pytest_upgrade_{uuid.uuid4().hex[:8]}"


def _load_gist_manifest() -> dict:
    manifest_path = GIST_REPO / "package.json"
    assert manifest_path.exists(), f"gist package.json not found: {manifest_path}"
    return json.loads(manifest_path.read_text(encoding="utf-8"))


def _classify_unit(manifest: dict, path: str) -> str:
    """Return 'schema' or 'data' for a loadOrder entry, matching the Swift app logic."""
    schema_files = {s["file"] for s in manifest.get("schemas", [])}
    return "schema" if path in schema_files else "data"


def _detect_transaction_type(query: str) -> str:
    """Mirror the Swift app's transactionType(for:) regex logic."""
    normalized = query.lower()
    if re.search(r"\b(define|undefine)\b", normalized):
        return "schema"
    if re.search(r"\b(insert|delete|update|put)\b", normalized):
        return "write"
    return "read"


def _read_tql(relative_path: str) -> str:
    path = GIST_REPO / relative_path
    assert path.exists(), f"TQL file not found: {path}"
    return path.read_text(encoding="utf-8")


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def driver():
    """Create a TypeDB driver connection (module-scoped)."""
    try:
        from typedb.driver import Credentials, DriverOptions, TypeDB
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
def ephemeral_db(driver):
    """Create an ephemeral database, yield its name, delete on teardown."""
    db = _ephemeral_db_name()
    driver.databases.create(db)
    yield db
    if driver.databases.contains(db):
        driver.databases.get(db).delete()


# ---------------------------------------------------------------------------
# Core apply logic (mirrors the Swift app exactly)
# ---------------------------------------------------------------------------


def _apply_gist_bootstrap(driver, db: str) -> None:
    """Apply the full gist bootstrap plan, mirroring the Swift app behavior.

    For each unit in assembly.loadOrder:
      - schema units: sent as a single schema transaction (the whole file)
      - data units: sent as a single write transaction (the whole file)

    The Swift app does NOT split data files into individual statements.
    It sends the entire file content as one query via oneShotQuery().
    """
    from typedb.driver import TransactionType

    manifest = _load_gist_manifest()
    load_order = manifest["assembly"]["loadOrder"]

    for asset_path in load_order:
        tql = _read_tql(asset_path)
        kind = _classify_unit(manifest, asset_path)

        if kind == "schema":
            # Schema: single schema transaction with full file content
            with driver.transaction(db, TransactionType.SCHEMA) as tx:
                tx.query(tql).resolve()
                tx.commit()
        else:
            # Data: detect transaction type from content, send full file as one query
            tx_type_str = _detect_transaction_type(tql)
            if tx_type_str == "schema":
                tx_type = TransactionType.SCHEMA
            elif tx_type_str == "write":
                tx_type = TransactionType.WRITE
            else:
                tx_type = TransactionType.READ

            with driver.transaction(db, tx_type) as tx:
                tx.query(tql).resolve()
                tx.commit()


def _delete_all_types(driver, db: str) -> None:
    """Simulate the 'Delete All Types' button: delete + recreate database."""
    driver.databases.get(db).delete()
    driver.databases.create(db)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.mark.typedb
class TestUpgradeBootstrap:
    """Reproduce the Delete All Types -> re-apply @key constraint violation."""

    def test_fresh_install(self, driver, ephemeral_db):
        """Apply all 6 gist bootstrap units to a fresh database.

        This verifies the first-time install path works. Each unit is
        applied in loadOrder: 3 schemas, then 3 data files. The entire
        file content is sent as a single query per unit, matching the
        Swift app's oneShotQuery() behavior.
        """
        from typedb.driver import TransactionType

        _apply_gist_bootstrap(driver, ephemeral_db)

        # Sanity check: OntologyPackageBuild was created
        with driver.transaction(ephemeral_db, TransactionType.READ) as tx:
            rows = list(
                tx.query(
                    "match $b isa OntologyPackageBuild; select $b;"
                )
                .resolve()
                .as_concept_rows()
            )
            assert len(rows) >= 1, (
                "Expected at least 1 OntologyPackageBuild after fresh install"
            )

        # Sanity check: SchemaModule was created (from schema-docs)
        with driver.transaction(ephemeral_db, TransactionType.READ) as tx:
            rows = list(
                tx.query(
                    "match $m isa SchemaModule; select $m;"
                )
                .resolve()
                .as_concept_rows()
            )
            assert len(rows) >= 1, (
                "Expected at least 1 SchemaModule after fresh install"
            )

    def test_upgrade_after_delete_all_types(self, driver, ephemeral_db):
        """Apply gist, delete all types, re-apply gist.

        The second apply should succeed (same fresh-database conditions).
        The UI reported a failure with:

            [CNT9] Constraint '@unique' has been violated:
            there is a conflict for value
            '"gist@14.0.0:6ab80c158a7fa56a1b5d3d824b125b92107e8f08"'.
            Instance of type 'OntologyPackageBuild' has a key constraint
            violation for attribute ownership of 'buildKey'.

        This test exercises the exact same flow using the Python TypeDB
        driver. If it passes, the bug is specific to the Swift/C driver
        or something else in the app's flow (e.g., the C driver's
        transaction_query() not handling multi-statement put).
        """
        from typedb.driver import TransactionType

        # --- First apply: fresh database ---
        _apply_gist_bootstrap(driver, ephemeral_db)

        # Sanity check: OntologyPackageBuild was created
        with driver.transaction(ephemeral_db, TransactionType.READ) as tx:
            rows = list(
                tx.query(
                    "match $b isa OntologyPackageBuild; select $b;"
                )
                .resolve()
                .as_concept_rows()
            )
            assert len(rows) >= 1, (
                "Expected at least 1 OntologyPackageBuild after first apply"
            )

        # --- Delete All Types (simulates the UI button) ---
        _delete_all_types(driver, ephemeral_db)

        # --- Second apply: same bootstrap to a now-empty database ---
        _apply_gist_bootstrap(driver, ephemeral_db)

        # Verify data integrity after re-apply
        with driver.transaction(ephemeral_db, TransactionType.READ) as tx:
            rows = list(
                tx.query(
                    "match $b isa OntologyPackageBuild; select $b;"
                )
                .resolve()
                .as_concept_rows()
            )
            assert len(rows) == 1, (
                f"Expected exactly 1 OntologyPackageBuild after re-apply, "
                f"got {len(rows)}"
            )
