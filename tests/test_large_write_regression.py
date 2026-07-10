"""
Regression test for oversized one-shot TypeDB write queries.

This covers the production contract we actually care about: applying a shipped
ontology package against a real private TypeDB server must not crash the server.

Today, applying the large `gistCore-schema-docs.tql` file as one write query can
kill the database process with:

    thread 'tokio-runtime-worker' has overflowed its stack

Why this lives in ontology-tooling:
- it exercises the shipped ontology package artifacts directly
- it uses a real private TypeDB server process
- it does not depend on Xcode or any downstream app code

This test is marked xfail until the installer path is fixed to batch large write
queries instead of sending the whole file in one transaction.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import socket
import subprocess
import tempfile
import time
import uuid
from pathlib import Path

import pytest


TOOLING_ROOT = Path(__file__).resolve().parent.parent
GIST_REPO = TOOLING_ROOT.parent / "ontology-gist"
TYPEDB_BIN = Path(os.environ.get("TYPEDB_BIN", Path.home() / ".typedb" / "typedb"))


def _free_port() -> int:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    try:
        return sock.getsockname()[1]
    finally:
        sock.close()


def _git_show(repo: Path, ref: str, relative_path: str) -> str:
    return subprocess.check_output(
        ["git", "show", f"{ref}:{relative_path}"],
        cwd=repo,
        text=True,
    )


def _load_manifest(repo: Path, ref: str) -> dict:
    return json.loads(_git_show(repo, ref, "package.json"))


def _archive_repo_at_ref(repo: Path, ref: str, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    archive = subprocess.Popen(
        ["git", "archive", ref],
        cwd=repo,
        stdout=subprocess.PIPE,
    )
    try:
        subprocess.check_call(["tar", "-x", "-C", str(destination)], stdin=archive.stdout)
    finally:
        if archive.stdout is not None:
            archive.stdout.close()
        archive.wait(timeout=10)


def _prepare_executable_package(repo_path: Path) -> None:
    # Delegates to the Rust `ont` CLI (the JS release tooling was removed).
    # The bin/ shim builds `ont` on first use if needed.
    ont_shim = TOOLING_ROOT / "bin" / "ontology-release"
    ont_bin = TOOLING_ROOT / "target" / "release" / "ont"
    if not ont_bin.exists():
        # Trigger the shim's one-time build without running a release.
        subprocess.check_call([str(ont_shim), "--help"], cwd=TOOLING_ROOT,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.check_call(
        [str(ont_bin), "prepare-package", "--repo", str(repo_path)],
        cwd=TOOLING_ROOT,
        stdout=subprocess.DEVNULL,
    )


def _detect_transaction_type(query: str):
    from typedb.driver import TransactionType

    normalized = query.lower()
    if re.search(r"\b(define|undefine)\b", normalized):
        return TransactionType.SCHEMA
    if re.search(r"\b(insert|delete|update|put)\b", normalized):
        return TransactionType.WRITE
    return TransactionType.READ


def _apply_query(driver, database_name: str, query: str) -> None:
    tx_type = _detect_transaction_type(query)
    with driver.transaction(database_name, tx_type) as tx:
        tx.query(query).resolve()
        tx.commit()


def _log_tail(log_path: Path, lines: int = 80) -> str:
    text = log_path.read_text(encoding="utf-8", errors="ignore")
    parts = text.splitlines()
    return "\n".join(parts[-lines:])


@pytest.fixture()
def isolated_typedb_server():
    if not TYPEDB_BIN.exists():
        pytest.skip(f"TypeDB binary not found at {TYPEDB_BIN}")

    temp_root = Path(tempfile.mkdtemp(prefix="typedb-large-write-regression-"))
    log_path = temp_root / "server.log"

    grpc_port = _free_port()
    http_port = _free_port()

    with log_path.open("w+", encoding="utf-8") as log_handle:
        proc = subprocess.Popen(
            [
                str(TYPEDB_BIN),
                "server",
                "--server.address",
                f"127.0.0.1:{grpc_port}",
                "--server.http.address",
                f"127.0.0.1:{http_port}",
                "--storage.data-directory",
                str(temp_root / "data"),
                "--logging.directory",
                str(temp_root / "logs"),
                "--diagnostics.reporting.metrics",
                "false",
                "--diagnostics.reporting.errors",
                "false",
                "--diagnostics.monitoring.enabled",
                "false",
            ],
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            text=True,
        )

        deadline = time.time() + 60
        while time.time() < deadline:
            time.sleep(1)
            log_handle.flush()
            if "Ready!" in log_path.read_text(encoding="utf-8", errors="ignore"):
                break
            if proc.poll() is not None:
                break
        else:
            proc.terminate()
            raise AssertionError(f"Timed out waiting for TypeDB readiness.\n{log_path.read_text(encoding='utf-8', errors='ignore')}")

        if proc.poll() is not None:
            raise AssertionError(f"TypeDB exited before becoming ready.\n{log_path.read_text(encoding='utf-8', errors='ignore')}")

        yield {
            "address": f"127.0.0.1:{grpc_port}",
            "process": proc,
            "log_path": log_path,
            "temp_root": temp_root,
        }

        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()

    shutil.rmtree(temp_root, ignore_errors=True)


@pytest.fixture()
def typedb_driver(isolated_typedb_server):
    try:
        from typedb.driver import Credentials, DriverOptions, TypeDB
    except ImportError:
        pytest.skip("typedb-driver not installed")

    driver = TypeDB.driver(
        isolated_typedb_server["address"],
        Credentials("admin", "password"),
        DriverOptions(is_tls_enabled=False),
    )
    try:
        yield driver
    finally:
        driver.close()


@pytest.fixture()
def ephemeral_db(typedb_driver):
    db_name = f"pytest_large_write_{uuid.uuid4().hex[:8]}"
    typedb_driver.databases.create(db_name)
    try:
        yield db_name
    finally:
        try:
            if typedb_driver.databases.contains(db_name):
                typedb_driver.databases.get(db_name).delete()
        except Exception:
            # The regression under test can kill the server mid-test.
            pass


@pytest.mark.typedb
@pytest.mark.xfail(
    strict=True,
    reason="A one-shot gist bootstrap can still crash a private TypeDB server on the large schema-docs write.",
)
def test_gist_bootstrap_one_shot_keeps_private_typedb_server_alive(
    isolated_typedb_server,
    typedb_driver,
    ephemeral_db,
):
    """A shipped gist bootstrap should not crash a private TypeDB server.

    This mirrors the unsafe production behavior exactly: each load-order asset is
    sent as a single full-file query, with no batching for large write units.
    """

    manifest = _load_manifest(GIST_REPO, "v2.0.0")
    last_asset = None

    try:
        for asset_path in manifest["assembly"]["loadOrder"]:
            last_asset = asset_path
            query = _git_show(GIST_REPO, "v2.0.0", asset_path)
            _apply_query(typedb_driver, ephemeral_db, query)
    except Exception as exc:
        process_status = isolated_typedb_server["process"].poll()
        log_tail = _log_tail(isolated_typedb_server["log_path"])
        pytest.fail(
            "One-shot gist bootstrap crashed or disconnected the private TypeDB server.\n"
            f"Last asset: {last_asset}\n"
            f"Process status: {process_status}\n"
            f"Driver error: {exc!r}\n"
            f"Server log tail:\n{log_tail}"
        )

    assert isolated_typedb_server["process"].poll() is None, (
        "TypeDB server died during one-shot gist bootstrap.\n"
        f"Server log tail:\n{_log_tail(isolated_typedb_server['log_path'])}"
    )


@pytest.mark.typedb
def test_normalized_gist_bootstrap_one_shot_keeps_private_typedb_server_alive(
    isolated_typedb_server,
    typedb_driver,
    ephemeral_db,
):
    """The published executable form should be safe for a dumb one-shot loader."""

    temp_repo = Path(tempfile.mkdtemp(prefix="gist-safe-publish-"))
    try:
        _archive_repo_at_ref(GIST_REPO, "v2.0.0", temp_repo)
        _prepare_executable_package(temp_repo)

        manifest = json.loads((temp_repo / "package.json").read_text(encoding="utf-8"))
        for asset_path in manifest["assembly"]["loadOrder"]:
            query = (temp_repo / asset_path).read_text(encoding="utf-8")
            _apply_query(typedb_driver, ephemeral_db, query)

        assert isolated_typedb_server["process"].poll() is None, (
            "TypeDB server died while applying normalized gist bootstrap.\n"
            f"Server log tail:\n{_log_tail(isolated_typedb_server['log_path'])}"
        )
    finally:
        shutil.rmtree(temp_repo, ignore_errors=True)
