"""
Integration tests for applying ontology schemas and data to a real TypeDB instance.

Verifies:
- All bundled schemas apply in dependency order (gistCore → beads → gastown)
- Schema re-application is idempotent (define is additive)
- put-based data files apply without @unique constraint violations
- put-based data re-application is idempotent (no duplicates)
- Self-describing meta-schema applies and SchemaResource/SchemaModule data works
- match-insert categorization works after put data exists
- transactionType detection correctly classifies put as write

Requires: TypeDB running at TYPEDB_ADDRESS (default: 192.168.66.2:1729)
"""

import os
import re
import uuid
from pathlib import Path

import pytest
from typedb.driver import (
    Credentials,
    DriverOptions,
    TransactionType,
    TypeDB,
)

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

TYPEDB_ADDRESS = os.environ.get("TYPEDB_ADDRESS", "192.168.66.2:1729")
TYPEDB_USER = os.environ.get("TYPEDB_USER", "admin")
TYPEDB_PASS = os.environ.get("TYPEDB_PASS", "password")

# Path to bundled ontology packages in the OneWorkspace repo
PACKAGES_ROOT = (
    Path(__file__).resolve().parent.parent.parent.parent
    / "collection-one"
    / "OneWorkspace"
    / "OneApps"
    / "Ontology"
    / "Resources"
    / "Packages"
)

SELF_DESCRIBING_META_SCHEMA = """\
define

  attribute docKey, value string;
  attribute iri, value string;
  attribute typeLabel, value string;
  attribute kind, value string;

  attribute prefLabel, value string;
  attribute definition, value string;
  attribute scopeNote, value string;
  attribute example, value string;

  attribute editorialNote, value string;
  attribute altLabel, value string;
  attribute historyNote, value string;
  attribute seeAlso, value string;
  attribute domainIncludes, value string;
  attribute deprecated, value boolean;

  attribute moduleKey, value string;
  attribute moduleName, value string;
  attribute moduleKind, value string;

  entity SchemaModule,
    owns moduleKey @key,
    owns moduleName,
    owns moduleKind;

  entity SchemaResource,
    owns docKey @key,
    owns iri,
    owns typeLabel,
    owns kind,
    owns prefLabel,
    owns definition,
    owns scopeNote,
    owns example,
    owns editorialNote,
    owns altLabel,
    owns historyNote,
    owns seeAlso,
    owns domainIncludes,
    owns deprecated @card(0..1);

  relation inModule,
    relates resource,
    relates module;

  SchemaResource plays inModule:resource;
  SchemaModule plays inModule:module;
"""


def _ephemeral_db_name(prefix: str = "pytest") -> str:
    short = uuid.uuid4().hex[:8]
    return f"{prefix}_{short}"


def _read_tql(relative_path: str) -> str:
    path = PACKAGES_ROOT / relative_path
    assert path.exists(), f"TQL file not found: {path}"
    return path.read_text(encoding="utf-8")


def _split_put_statements(tql: str) -> list[str]:
    """Split a TQL file with multiple `put` statements into individual queries."""
    queries: list[str] = []
    current = ""
    for line in tql.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.startswith("put ") and current:
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


def _split_match_insert_statements(tql: str) -> list[str]:
    """Split a TQL file with multiple match-insert blocks into individual queries."""
    queries: list[str] = []
    current = ""
    for line in tql.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.startswith("match") and current:
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


def _count_entities(driver, db: str, entity_type: str) -> int:
    with driver.transaction(db, TransactionType.READ) as tx:
        rows = list(
            tx.query(f"match $x isa {entity_type}; select $x;")
            .resolve()
            .as_concept_rows()
        )
        return len(rows)


@pytest.fixture(scope="module")
def driver():
    creds = Credentials(TYPEDB_USER, TYPEDB_PASS)
    opts = DriverOptions(is_tls_enabled=False)
    d = TypeDB.driver(TYPEDB_ADDRESS, creds, opts)
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


def _apply_all_schemas(driver, db: str) -> None:
    """Apply gistCore → beads → gastown schemas in dependency order."""
    for path in [
        "ontology-gist/schema/gistCore.tql",
        "ontology-beads/schema/beads.tql",
        "ontology-gastown/schema/gastown.tql",
    ]:
        tql = _read_tql(path)
        with driver.transaction(db, TransactionType.SCHEMA) as tx:
            tx.query(tql).resolve()
            tx.commit()


# ---------------------------------------------------------------------------
# Tests: Schema Application
# ---------------------------------------------------------------------------


@pytest.mark.typedb
class TestSchemaApplication:
    def test_apply_all_schemas_in_order(self, driver, ephemeral_db):
        """gistCore + beads + gastown schemas apply without error."""
        _apply_all_schemas(driver, ephemeral_db)

    def test_reapply_schemas_is_idempotent(self, driver, ephemeral_db):
        """Re-applying the same schemas does not fail (define is additive)."""
        _apply_all_schemas(driver, ephemeral_db)
        # Second pass
        _apply_all_schemas(driver, ephemeral_db)


# ---------------------------------------------------------------------------
# Tests: Data Application (put-based)
# ---------------------------------------------------------------------------


@pytest.mark.typedb
class TestBeadsReferenceData:
    def test_apply_beads_reference(self, driver, ephemeral_db):
        """beads-reference.tql put statements apply after schemas."""
        _apply_all_schemas(driver, ephemeral_db)

        tql = _read_tql("ontology-beads/data/beads-reference.tql")
        queries = _split_put_statements(tql)
        assert len(queries) == 15, f"Expected 15 put stmts, got {len(queries)}"

        for q in queries:
            with driver.transaction(ephemeral_db, TransactionType.WRITE) as tx:
                tx.query(q).resolve()
                tx.commit()

        assert _count_entities(driver, ephemeral_db, "BeadType") == 4
        assert _count_entities(driver, ephemeral_db, "BeadStatus") == 6
        assert _count_entities(driver, ephemeral_db, "BeadPriority") == 5

    def test_beads_reference_idempotent(self, driver, ephemeral_db):
        """Re-applying beads-reference.tql does not create duplicates."""
        _apply_all_schemas(driver, ephemeral_db)

        tql = _read_tql("ontology-beads/data/beads-reference.tql")
        queries = _split_put_statements(tql)

        # First pass
        for q in queries:
            with driver.transaction(ephemeral_db, TransactionType.WRITE) as tx:
                tx.query(q).resolve()
                tx.commit()

        # Second pass — must not fail or duplicate
        for q in queries:
            with driver.transaction(ephemeral_db, TransactionType.WRITE) as tx:
                tx.query(q).resolve()
                tx.commit()

        assert _count_entities(driver, ephemeral_db, "BeadType") == 4
        assert _count_entities(driver, ephemeral_db, "BeadStatus") == 6
        assert _count_entities(driver, ephemeral_db, "BeadPriority") == 5


# ---------------------------------------------------------------------------
# Tests: Self-Describing Meta-Schema
# ---------------------------------------------------------------------------


@pytest.mark.typedb
class TestSelfDescribingMetaSchema:
    def test_meta_schema_applies(self, driver, ephemeral_db):
        """Self-describing meta-schema applies on top of domain schemas."""
        _apply_all_schemas(driver, ephemeral_db)
        with driver.transaction(ephemeral_db, TransactionType.SCHEMA) as tx:
            tx.query(SELF_DESCRIBING_META_SCHEMA).resolve()
            tx.commit()

    def test_schema_resource_put_idempotent(self, driver, ephemeral_db):
        """SchemaResource and SchemaModule put data is idempotent."""
        _apply_all_schemas(driver, ephemeral_db)
        with driver.transaction(ephemeral_db, TransactionType.SCHEMA) as tx:
            tx.query(SELF_DESCRIBING_META_SCHEMA).resolve()
            tx.commit()

        module_put = '''
            put $m isa SchemaModule,
              has moduleKey "gist-core",
              has moduleName "gistCore",
              has moduleKind "gist-core";
        '''
        resource_put = '''
            put $r isa SchemaResource,
              has docKey "https://w3id.org/semanticarts/ontology/gistCore#Event",
              has typeLabel "Event",
              has kind "entity",
              has prefLabel "Event",
              has definition "A temporal occurrence.";
        '''

        # Apply twice
        for _ in range(2):
            with driver.transaction(ephemeral_db, TransactionType.WRITE) as tx:
                tx.query(module_put).resolve()
                tx.commit()
            with driver.transaction(ephemeral_db, TransactionType.WRITE) as tx:
                tx.query(resource_put).resolve()
                tx.commit()

        assert _count_entities(driver, ephemeral_db, "SchemaModule") == 1
        assert _count_entities(driver, ephemeral_db, "SchemaResource") == 1

    def test_in_module_categorization(self, driver, ephemeral_db):
        """match-insert inModule relation works after put data exists."""
        _apply_all_schemas(driver, ephemeral_db)
        with driver.transaction(ephemeral_db, TransactionType.SCHEMA) as tx:
            tx.query(SELF_DESCRIBING_META_SCHEMA).resolve()
            tx.commit()

        with driver.transaction(ephemeral_db, TransactionType.WRITE) as tx:
            tx.query('''
                put $m isa SchemaModule,
                  has moduleKey "test-mod",
                  has moduleName "TestModule",
                  has moduleKind "test";
            ''').resolve()
            tx.commit()

        with driver.transaction(ephemeral_db, TransactionType.WRITE) as tx:
            tx.query('''
                put $r isa SchemaResource,
                  has docKey "test:type:Event",
                  has typeLabel "Event",
                  has kind "entity",
                  has prefLabel "Event",
                  has definition "Test event.";
            ''').resolve()
            tx.commit()

        with driver.transaction(ephemeral_db, TransactionType.WRITE) as tx:
            tx.query('''
                match
                  $m isa SchemaModule, has moduleKey "test-mod";
                  $r isa SchemaResource, has typeLabel "Event";
                insert
                  (resource: $r, module: $m) isa inModule;
            ''').resolve()
            tx.commit()

        # Verify
        with driver.transaction(ephemeral_db, TransactionType.READ) as tx:
            rows = list(
                tx.query('''
                    match
                      $m isa SchemaModule, has moduleKey "test-mod";
                      (resource: $r, module: $m) isa inModule;
                      $r has typeLabel $label;
                    select $label;
                ''')
                .resolve()
                .as_concept_rows()
            )
            assert len(rows) == 1
            assert rows[0].get("label").get_value() == "Event"


# ---------------------------------------------------------------------------
# Tests: TypeDoc/Domain Schema Gap
# ---------------------------------------------------------------------------


@pytest.mark.typedb
class TestTypedocDomainGap:
    def test_domain_type_not_in_bundled_schemas(self, driver, ephemeral_db):
        """Domain entity type is not defined in bundled schemas (documents gap).

        The data files reference TypeDoc and Domain types, but no bundled schema
        defines them. The ontology-typedoc package is missing. This test documents
        the gap — it should be converted to a positive test once the schema is added.
        """
        _apply_all_schemas(driver, ephemeral_db)

        with pytest.raises(Exception):
            with driver.transaction(ephemeral_db, TransactionType.WRITE) as tx:
                tx.query('''
                    put $d isa Domain,
                      has name "gist",
                      has description "gist upper ontology.";
                ''').resolve()
                tx.commit()


# ---------------------------------------------------------------------------
# Tests: Transaction Type Detection (unit test, no TypeDB needed)
# ---------------------------------------------------------------------------


class TestTransactionTypeDetection:
    """Mirrors the regex from OntologyViewModel+TypeDB.swift transactionType(for:)."""

    WRITE_RE = re.compile(r"\b(insert|delete|update|put)\b", re.IGNORECASE)
    SCHEMA_RE = re.compile(r"\b(define|undefine)\b", re.IGNORECASE)

    def _classify(self, query: str) -> str:
        if self.SCHEMA_RE.search(query):
            return "schema"
        if self.WRITE_RE.search(query):
            return "write"
        return "read"

    def test_put_is_write(self):
        q = 'put $epic isa BeadType, has name "epic";'
        assert self._classify(q) == "write"

    def test_insert_is_write(self):
        q = 'insert $f isa Foo, has name "bar";'
        assert self._classify(q) == "write"

    def test_match_select_is_read(self):
        q = "match $t isa BeadType; select $t;"
        assert self._classify(q) == "read"

    def test_define_is_schema(self):
        q = "define entity Foo;"
        assert self._classify(q) == "schema"

    def test_match_insert_is_write(self):
        q = 'match $d isa Domain; insert (categorized: $d) isa isCategorizedBy;'
        assert self._classify(q) == "write"
