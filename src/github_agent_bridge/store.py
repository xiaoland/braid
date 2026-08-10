"""Durable transport authority backed by a single SQLite database.

Only Wrapper-owned references and delivery state belong here.  GitHub content
and provider thread history deliberately do not.
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime
from enum import StrEnum
import math
from pathlib import Path
import sqlite3
import time
from typing import Iterator


SCHEMA_VERSION = 1


class StoreError(RuntimeError):
    """Base error for durable transport state."""


class SchemaVersionError(StoreError):
    """The database schema cannot be opened by this runtime."""


class LeaseHeld(StoreError):
    """Another process still owns the runtime lease."""


class StaleLease(StoreError):
    """A write attempted to use an expired or superseded lease."""


class StateConflict(StoreError):
    """A durable identity was reused for different immutable facts."""


class InvalidTransition(StoreError):
    """A state transition would violate the transport protocol."""


class BindingLifecycle(StrEnum):
    ACTIVE = "active"
    PAUSED = "paused"
    REVOKED = "revoked"


class SurfaceKind(StrEnum):
    ISSUE = "issue"
    PULL_REQUEST = "pull_request"


class EventState(StrEnum):
    PENDING = "pending"
    DELIVERED = "delivered"
    SUPERSEDED = "superseded"


class OutboxState(StrEnum):
    PENDING = "pending"
    SENDING = "sending"
    UNCERTAIN = "uncertain"
    ACKED = "acked"


TRUSTED_URGENT_PERMISSION_ROLES = frozenset(
    {"triage", "write", "maintain", "admin"}
)


@dataclass(frozen=True, slots=True)
class LeaseToken:
    owner_id: str
    generation: int
    expires_at: float


@dataclass(frozen=True, slots=True)
class Binding:
    binding_id: str
    repository_node_id: str
    repository_full_name: str
    issue_node_id: str
    issue_number: int
    issue_url: str
    thread_address: str
    agent_identity: str
    wrapper_identity: str
    trusted_permission: str
    instruction_digest: str
    lifecycle: BindingLifecycle = BindingLifecycle.ACTIVE


@dataclass(frozen=True, slots=True)
class SurfaceRoute:
    binding_id: str
    surface_kind: SurfaceKind
    repository_node_id: str
    repository_full_name: str
    surface_node_id: str
    surface_number: int
    canonical_url: str
    association_version: str
    active: bool = True


@dataclass(frozen=True, slots=True)
class EventEnvelope:
    event_key: str
    binding_id: str
    event_name: str
    action: str
    object_node_id: str
    surface_kind: str
    surface_node_id: str
    object_version: str
    body_digest: str
    canonical_url: str
    observed_at: float
    actor_node_id: str | None = None
    actor_login: str | None = None
    author_association: str | None = None
    permission_role: str | None = None
    mention_detected: bool = False
    urgent: bool = False
    wake_eligible: bool = True
    delivery_id: str | None = None


@dataclass(frozen=True, slots=True)
class StoredEvent:
    event_id: int
    event_key: str
    delivery_id: str | None
    binding_id: str
    event_name: str
    action: str
    object_node_id: str
    surface_kind: str
    surface_node_id: str
    object_version: str
    body_digest: str
    canonical_url: str
    observed_at: float
    actor_node_id: str | None
    actor_login: str | None
    author_association: str | None
    permission_role: str | None
    mention_detected: bool
    urgent: bool
    wake_eligible: bool
    scheduled_at: float | None
    state: EventState


@dataclass(frozen=True, slots=True)
class SchedulerSnapshot:
    binding_id: str
    generation: int
    quiet_deadline: float | None
    urgent_generation: int
    active_turn_handle: str | None
    transport_status: str


@dataclass(frozen=True, slots=True)
class OutboxIntent:
    operation_key: str
    binding_id: str
    operation_kind: str
    target_node_id: str
    intended_digest: str


@dataclass(frozen=True, slots=True)
class StoredOutbox:
    operation_key: str
    binding_id: str
    operation_kind: str
    target_node_id: str
    intended_digest: str
    state: OutboxState
    attempts: int
    remote_id: str | None
    remote_digest: str | None
    reconciliation_checked_at: float | None


@dataclass(frozen=True, slots=True)
class MirrorChunkIntent:
    chunk_index: int
    body_digest: str
    ownership_marker: str


@dataclass(frozen=True, slots=True)
class StoredMirrorChunk:
    turn_id: str
    chunk_index: int
    revision: int
    body_digest: str
    ownership_marker: str
    remote_id: str | None
    remote_url: str | None
    remote_digest: str | None
    active: bool


@dataclass(frozen=True, slots=True)
class MirrorState:
    turn_id: str
    binding_id: str
    target_node_id: str
    terminal_state: str | None
    revision: int


SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS owner_lease (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    owner_id TEXT,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    lease_expires_at REAL NOT NULL,
    updated_at REAL NOT NULL
);
INSERT OR IGNORE INTO owner_lease(singleton, owner_id, generation, lease_expires_at, updated_at)
VALUES (1, NULL, 0, 0, 0);

CREATE TABLE IF NOT EXISTS bindings (
    binding_id TEXT PRIMARY KEY,
    repository_node_id TEXT NOT NULL,
    repository_full_name TEXT NOT NULL,
    issue_node_id TEXT NOT NULL UNIQUE,
    issue_number INTEGER NOT NULL CHECK (issue_number > 0),
    issue_url TEXT NOT NULL,
    thread_address TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    wrapper_identity TEXT NOT NULL,
    trusted_permission TEXT NOT NULL,
    instruction_digest TEXT NOT NULL,
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('active', 'paused', 'revoked')),
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    event_id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_key TEXT NOT NULL UNIQUE,
    delivery_id TEXT,
    binding_id TEXT NOT NULL REFERENCES bindings(binding_id),
    event_name TEXT NOT NULL,
    action TEXT NOT NULL,
    object_node_id TEXT NOT NULL,
    surface_kind TEXT NOT NULL,
    surface_node_id TEXT NOT NULL,
    object_version TEXT NOT NULL,
    body_digest TEXT NOT NULL,
    canonical_url TEXT NOT NULL,
    observed_at REAL NOT NULL,
    actor_node_id TEXT,
    actor_login TEXT,
    author_association TEXT,
    permission_role TEXT,
    mention_detected INTEGER NOT NULL CHECK (mention_detected IN (0, 1)),
    urgent INTEGER NOT NULL CHECK (urgent IN (0, 1)),
    wake_eligible INTEGER NOT NULL CHECK (wake_eligible IN (0, 1)),
    scheduled_at REAL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'delivered', 'superseded')),
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS events_delivery_id_unique
ON events(delivery_id) WHERE delivery_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS events_pending_order
ON events(binding_id, state, observed_at, event_id);

CREATE TABLE IF NOT EXISTS schedulers (
    binding_id TEXT PRIMARY KEY REFERENCES bindings(binding_id),
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    quiet_deadline REAL,
    urgent_generation INTEGER NOT NULL DEFAULT 0 CHECK (urgent_generation >= 0),
    active_turn_handle TEXT,
    transport_status TEXT NOT NULL DEFAULT 'idle',
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS surface_routes (
    surface_node_id TEXT PRIMARY KEY,
    binding_id TEXT NOT NULL REFERENCES bindings(binding_id),
    surface_kind TEXT NOT NULL CHECK (surface_kind IN ('pull_request')),
    repository_node_id TEXT NOT NULL,
    repository_full_name TEXT NOT NULL,
    surface_number INTEGER NOT NULL CHECK (surface_number > 0),
    canonical_url TEXT NOT NULL,
    association_version TEXT NOT NULL,
    active INTEGER NOT NULL CHECK (active IN (0, 1)),
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS surface_routes_one_active_pr
ON surface_routes(binding_id) WHERE active = 1 AND surface_kind = 'pull_request';

CREATE TABLE IF NOT EXISTS mirrors (
    turn_id TEXT PRIMARY KEY,
    binding_id TEXT NOT NULL REFERENCES bindings(binding_id),
    target_node_id TEXT NOT NULL,
    remote_comment_id TEXT,
    terminal_state TEXT,
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    body_digest TEXT NOT NULL,
    ownership_state TEXT NOT NULL DEFAULT 'owned',
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS mirror_chunks (
    turn_id TEXT NOT NULL REFERENCES mirrors(turn_id),
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    body_digest TEXT NOT NULL,
    ownership_marker TEXT NOT NULL,
    remote_id TEXT,
    remote_url TEXT,
    remote_digest TEXT,
    active INTEGER NOT NULL CHECK (active IN (0, 1)),
    updated_at REAL NOT NULL,
    PRIMARY KEY(turn_id, chunk_index),
    UNIQUE(ownership_marker)
);

CREATE TABLE IF NOT EXISTS outbox (
    operation_key TEXT PRIMARY KEY,
    binding_id TEXT NOT NULL REFERENCES bindings(binding_id),
    operation_kind TEXT NOT NULL,
    target_node_id TEXT NOT NULL,
    intended_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'sending', 'uncertain', 'acked')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    remote_id TEXT,
    remote_digest TEXT,
    reconciliation_checked_at REAL,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS outbox_state_order ON outbox(state, created_at, operation_key);

CREATE TABLE IF NOT EXISTS sync_cursors (
    cursor_key TEXT PRIMARY KEY,
    binding_id TEXT REFERENCES bindings(binding_id),
    cursor_value TEXT NOT NULL,
    canonical_observed_at REAL NOT NULL,
    updated_at REAL NOT NULL
);
"""


class TransportStore:
    """A short-transaction async façade over one SQLite connection."""

    def __init__(
        self,
        path: Path,
        connection: sqlite3.Connection,
        *,
        clock: Callable[[], float],
    ) -> None:
        self.path = path
        self._connection = connection
        self._clock = clock
        self._lock = asyncio.Lock()
        self._closed = False

    @classmethod
    async def open(
        cls,
        path: Path,
        *,
        clock: Callable[[], float] = time.time,
    ) -> TransportStore:
        if not path.is_absolute():
            raise ValueError("state database path must be absolute")
        resolved = path.resolve()
        if not resolved.parent.is_dir():
            raise ValueError("state database parent directory must exist")
        connection = sqlite3.connect(
            resolved,
            isolation_level=None,
            timeout=5.0,
        )
        connection.row_factory = sqlite3.Row
        try:
            connection.execute("PRAGMA foreign_keys = ON")
            connection.execute("PRAGMA busy_timeout = 5000")
            version = int(connection.execute("PRAGMA user_version").fetchone()[0])
            if version not in {0, SCHEMA_VERSION}:
                raise SchemaVersionError(
                    f"database schema {version} is not supported by {SCHEMA_VERSION}"
                )
            connection.execute("PRAGMA journal_mode = WAL")
            connection.execute("PRAGMA synchronous = FULL")
            if version == 0:
                try:
                    connection.executescript(
                        "BEGIN IMMEDIATE;\n"
                        + SCHEMA_SQL
                        + f"\nPRAGMA user_version = {SCHEMA_VERSION};\nCOMMIT;"
                    )
                except BaseException:
                    if connection.in_transaction:
                        connection.rollback()
                    raise
        except BaseException:
            connection.close()
            raise
        return cls(resolved, connection, clock=clock)

    async def close(self) -> None:
        async with self._lock:
            if self._closed:
                return
            self._closed = True
            self._connection.close()

    async def schema_version(self) -> int:
        async with self._lock:
            self._require_open()
            return int(self._connection.execute("PRAGMA user_version").fetchone()[0])

    async def acquire_owner(self, owner_id: str, ttl_seconds: float) -> LeaseToken:
        _require_text(owner_id, "owner_id")
        _require_positive(ttl_seconds, "ttl_seconds")
        async with self._lock:
            now = self._clock()
            with self._transaction():
                row = self._connection.execute(
                    "SELECT owner_id, generation, lease_expires_at FROM owner_lease "
                    "WHERE singleton = 1"
                ).fetchone()
                assert row is not None
                if (
                    row["owner_id"] is not None
                    and row["owner_id"] != owner_id
                    and float(row["lease_expires_at"]) > now
                ):
                    raise LeaseHeld("runtime owner lease is held by another process")
                generation = int(row["generation"]) + 1
                expires_at = now + ttl_seconds
                self._connection.execute(
                    "UPDATE owner_lease SET owner_id = ?, generation = ?, "
                    "lease_expires_at = ?, updated_at = ? WHERE singleton = 1",
                    (owner_id, generation, expires_at, now),
                )
            return LeaseToken(owner_id, generation, expires_at)

    async def renew_owner(
        self, token: LeaseToken, ttl_seconds: float
    ) -> LeaseToken:
        _require_positive(ttl_seconds, "ttl_seconds")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                expires_at = now + ttl_seconds
                self._connection.execute(
                    "UPDATE owner_lease SET lease_expires_at = ?, updated_at = ? "
                    "WHERE singleton = 1",
                    (expires_at, now),
                )
            return LeaseToken(token.owner_id, token.generation, expires_at)

    async def release_owner(self, token: LeaseToken) -> None:
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                self._connection.execute(
                    "UPDATE owner_lease SET owner_id = NULL, lease_expires_at = 0, "
                    "updated_at = ? WHERE singleton = 1",
                    (now,),
                )

    async def put_binding(self, token: LeaseToken, binding: Binding) -> None:
        _validate_binding(binding)
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                existing = self._connection.execute(
                    "SELECT * FROM bindings WHERE binding_id = ?",
                    (binding.binding_id,),
                ).fetchone()
                immutable = (
                    binding.repository_node_id,
                    binding.repository_full_name,
                    binding.issue_node_id,
                    binding.issue_number,
                    binding.issue_url,
                    binding.thread_address,
                    binding.agent_identity,
                    binding.wrapper_identity,
                    binding.trusted_permission,
                    binding.instruction_digest,
                )
                if existing is None:
                    self._connection.execute(
                        "INSERT INTO bindings(binding_id, repository_node_id, "
                        "repository_full_name, issue_node_id, issue_number, issue_url, "
                        "thread_address, agent_identity, wrapper_identity, "
                        "trusted_permission, instruction_digest, lifecycle, created_at, "
                        "updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        (binding.binding_id, *immutable, binding.lifecycle.value, now, now),
                    )
                    self._connection.execute(
                        "INSERT INTO schedulers(binding_id, updated_at) VALUES (?, ?)",
                        (binding.binding_id, now),
                    )
                    return
                current_immutable = tuple(
                    existing[name]
                    for name in (
                        "repository_node_id",
                        "repository_full_name",
                        "issue_node_id",
                        "issue_number",
                        "issue_url",
                        "thread_address",
                        "agent_identity",
                        "wrapper_identity",
                        "trusted_permission",
                        "instruction_digest",
                    )
                )
                if current_immutable != immutable:
                    raise StateConflict("binding identity cannot be rewritten")
                self._connection.execute(
                    "UPDATE bindings SET lifecycle = ?, updated_at = ? "
                    "WHERE binding_id = ?",
                    (binding.lifecycle.value, now, binding.binding_id),
                )

    async def binding_for_issue(self, issue_node_id: str) -> Binding | None:
        _require_text(issue_node_id, "issue_node_id")
        async with self._lock:
            self._require_open()
            row = self._connection.execute(
                "SELECT * FROM bindings WHERE issue_node_id = ?", (issue_node_id,)
            ).fetchone()
            return None if row is None else _binding(row)

    async def get_binding(self, binding_id: str) -> Binding | None:
        _require_text(binding_id, "binding_id")
        async with self._lock:
            self._require_open()
            row = self._connection.execute(
                "SELECT * FROM bindings WHERE binding_id = ?", (binding_id,)
            ).fetchone()
            return None if row is None else _binding(row)

    async def replace_unmaterialized_thread_address(
        self,
        token: LeaseToken,
        binding_id: str,
        *,
        expected_thread_address: str,
        replacement_thread_address: str,
    ) -> Binding:
        """Replace an address only before any provider turn was delivered.

        Codex may not persist an empty thread.  This does not replace provider
        history: the transaction proves that no event reached a turn and no
        turn mirror exists for the binding.
        """

        for value, name in (
            (binding_id, "binding_id"),
            (expected_thread_address, "expected_thread_address"),
            (replacement_thread_address, "replacement_thread_address"),
        ):
            _require_text(value, name)
        if expected_thread_address == replacement_thread_address:
            raise ValueError("replacement thread address must be different")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                binding_row = self._connection.execute(
                    "SELECT * FROM bindings WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                if binding_row is None:
                    raise KeyError(binding_id)
                if binding_row["thread_address"] != expected_thread_address:
                    raise StateConflict("binding thread address was superseded")
                scheduler = self._connection.execute(
                    "SELECT active_turn_handle FROM schedulers WHERE binding_id = ?",
                    (binding_id,),
                ).fetchone()
                assert scheduler is not None
                delivered = self._connection.execute(
                    "SELECT COUNT(*) FROM events WHERE binding_id = ? "
                    "AND state = 'delivered'",
                    (binding_id,),
                ).fetchone()[0]
                mirrors = self._connection.execute(
                    "SELECT COUNT(*) FROM mirrors WHERE binding_id = ?",
                    (binding_id,),
                ).fetchone()[0]
                if (
                    scheduler["active_turn_handle"] is not None
                    or delivered
                    or mirrors
                ):
                    raise StateConflict(
                        "materialized binding thread address cannot be replaced"
                    )
                self._connection.execute(
                    "UPDATE bindings SET thread_address = ?, updated_at = ? "
                    "WHERE binding_id = ?",
                    (replacement_thread_address, now, binding_id),
                )
                updated = self._connection.execute(
                    "SELECT * FROM bindings WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                assert updated is not None
                return _binding(updated)

    async def require_unmaterialized_thread_address(
        self,
        token: LeaseToken,
        binding_id: str,
        *,
        expected_thread_address: str,
    ) -> None:
        """Fail closed unless an opaque address has never carried a turn."""

        _require_text(binding_id, "binding_id")
        _require_text(expected_thread_address, "expected_thread_address")
        async with self._lock:
            with self._transaction(token):
                binding_row = self._connection.execute(
                    "SELECT thread_address FROM bindings WHERE binding_id = ?",
                    (binding_id,),
                ).fetchone()
                if binding_row is None:
                    raise KeyError(binding_id)
                if binding_row["thread_address"] != expected_thread_address:
                    raise StateConflict("binding thread address was superseded")
                scheduler = self._connection.execute(
                    "SELECT active_turn_handle FROM schedulers WHERE binding_id = ?",
                    (binding_id,),
                ).fetchone()
                assert scheduler is not None
                delivered = self._connection.execute(
                    "SELECT COUNT(*) FROM events WHERE binding_id = ? "
                    "AND state = 'delivered'",
                    (binding_id,),
                ).fetchone()[0]
                mirrors = self._connection.execute(
                    "SELECT COUNT(*) FROM mirrors WHERE binding_id = ?",
                    (binding_id,),
                ).fetchone()[0]
                if (
                    scheduler["active_turn_handle"] is not None
                    or delivered
                    or mirrors
                ):
                    raise StateConflict(
                        "materialized binding thread address cannot be replaced"
                    )

    async def binding_for_surface(self, surface_node_id: str) -> Binding | None:
        """Resolve an exact active Issue or native-associated PR route."""

        _require_text(surface_node_id, "surface_node_id")
        issue_binding = await self.binding_for_issue(surface_node_id)
        if issue_binding is not None:
            return issue_binding
        async with self._lock:
            self._require_open()
            row = self._connection.execute(
                "SELECT bindings.* FROM surface_routes JOIN bindings "
                "USING(binding_id) WHERE surface_routes.surface_node_id = ? "
                "AND surface_routes.active = 1",
                (surface_node_id,),
            ).fetchone()
            return None if row is None else _binding(row)

    async def current_pr_route(self, binding_id: str) -> SurfaceRoute | None:
        _require_text(binding_id, "binding_id")
        async with self._lock:
            self._require_open()
            row = self._connection.execute(
                "SELECT * FROM surface_routes WHERE binding_id = ? "
                "AND surface_kind = 'pull_request' AND active = 1",
                (binding_id,),
            ).fetchone()
            return None if row is None else _surface_route(row)

    async def replace_current_pr_route(
        self,
        token: LeaseToken,
        binding_id: str,
        route: SurfaceRoute | None,
    ) -> SurfaceRoute | None:
        """Mechanically replace the sole current PR wake alias.

        Historical aliases remain inactive for audit and deduplication.  This
        stores no PR lifecycle, branch, commit, or worktree state.
        """

        _require_text(binding_id, "binding_id")
        if route is not None:
            _validate_surface_route(route)
            if route.binding_id != binding_id:
                raise ValueError("route binding_id does not match target binding")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                if self._connection.execute(
                    "SELECT 1 FROM bindings WHERE binding_id = ?", (binding_id,)
                ).fetchone() is None:
                    raise KeyError(binding_id)
                self._connection.execute(
                    "UPDATE surface_routes SET active = 0, updated_at = ? "
                    "WHERE binding_id = ? AND surface_kind = 'pull_request' "
                    "AND active = 1 AND (? IS NULL OR surface_node_id != ?)",
                    (
                        now,
                        binding_id,
                        None if route is None else route.surface_node_id,
                        None if route is None else route.surface_node_id,
                    ),
                )
                if route is None:
                    return None
                existing = self._connection.execute(
                    "SELECT * FROM surface_routes WHERE surface_node_id = ?",
                    (route.surface_node_id,),
                ).fetchone()
                immutable = (
                    route.binding_id,
                    route.surface_kind.value,
                    route.repository_node_id,
                    route.repository_full_name,
                    route.surface_number,
                    route.canonical_url,
                )
                if existing is None:
                    self._connection.execute(
                        "INSERT INTO surface_routes(surface_node_id, binding_id, "
                        "surface_kind, repository_node_id, repository_full_name, "
                        "surface_number, canonical_url, association_version, active, "
                        "created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
                        (route.surface_node_id, *immutable, route.association_version, now, now),
                    )
                else:
                    current_immutable = tuple(
                        existing[name]
                        for name in (
                            "binding_id",
                            "surface_kind",
                            "repository_node_id",
                            "repository_full_name",
                            "surface_number",
                            "canonical_url",
                        )
                    )
                    if current_immutable != immutable:
                        raise StateConflict(
                            "surface node identity cannot be rebound or rewritten"
                        )
                    self._connection.execute(
                        "UPDATE surface_routes SET association_version = ?, active = 1, "
                        "updated_at = ? WHERE surface_node_id = ?",
                        (route.association_version, now, route.surface_node_id),
                    )
                stored = self._connection.execute(
                    "SELECT * FROM surface_routes WHERE surface_node_id = ?",
                    (route.surface_node_id,),
                ).fetchone()
                assert stored is not None
                return _surface_route(stored)

    async def ingest_event(
        self, token: LeaseToken, envelope: EventEnvelope
    ) -> tuple[StoredEvent, bool]:
        _validate_event(envelope)
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                existing = self._find_event(envelope.event_key, envelope.delivery_id)
                if existing is not None:
                    stored = _stored_event(existing)
                    if not _event_matches(stored, envelope):
                        raise StateConflict(
                            "event key or delivery id was reused for different facts"
                        )
                    return stored, False
                previous = self._connection.execute(
                    "SELECT * FROM events WHERE binding_id = ? "
                    "AND object_node_id = ? ORDER BY event_id DESC LIMIT 1",
                    (envelope.binding_id, envelope.object_node_id),
                ).fetchone()
                version_order = (
                    None
                    if previous is None
                    else _compare_object_versions(
                        envelope.object_version,
                        str(previous["object_version"]),
                    )
                )
                stale = version_order is not None and version_order < 0
                if version_order is not None and version_order > 0:
                    self._connection.execute(
                        "UPDATE events SET state = 'superseded', updated_at = ? "
                        "WHERE binding_id = ? AND object_node_id = ? "
                        "AND state = 'pending'",
                        (now, envelope.binding_id, envelope.object_node_id),
                    )
                cursor = self._connection.execute(
                    "INSERT INTO events(event_key, delivery_id, binding_id, event_name, "
                    "action, object_node_id, surface_kind, surface_node_id, "
                    "object_version, body_digest, canonical_url, observed_at, urgent, "
                    "actor_node_id, actor_login, author_association, permission_role, "
                    "mention_detected, wake_eligible, state, created_at, updated_at) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, "
                    "?, ?, ?)",
                    (
                        envelope.event_key,
                        envelope.delivery_id,
                        envelope.binding_id,
                        envelope.event_name,
                        envelope.action,
                        envelope.object_node_id,
                        envelope.surface_kind,
                        envelope.surface_node_id,
                        envelope.object_version,
                        envelope.body_digest,
                        envelope.canonical_url,
                        envelope.observed_at,
                        int(envelope.urgent),
                        envelope.actor_node_id,
                        envelope.actor_login,
                        envelope.author_association,
                        envelope.permission_role,
                        int(envelope.mention_detected),
                        int(envelope.wake_eligible),
                        (
                            EventState.PENDING.value
                            if envelope.wake_eligible and not stale
                            else EventState.SUPERSEDED.value
                        ),
                        now,
                        now,
                    ),
                )
                row = self._connection.execute(
                    "SELECT * FROM events WHERE event_id = ?", (cursor.lastrowid,)
                ).fetchone()
                assert row is not None
                return _stored_event(row), True

    async def pending_events(
        self, token: LeaseToken, binding_id: str, *, limit: int = 100
    ) -> tuple[StoredEvent, ...]:
        _require_text(binding_id, "binding_id")
        if limit < 1 or limit > 1000:
            raise ValueError("limit must be between 1 and 1000")
        async with self._lock:
            with self._transaction(token):
                rows = self._connection.execute(
                    "SELECT * FROM events WHERE binding_id = ? AND state = 'pending' "
                    "ORDER BY observed_at, event_id LIMIT ?",
                    (binding_id, limit),
                ).fetchall()
                return tuple(_stored_event(row) for row in rows)

    async def latest_event_for_object(
        self,
        token: LeaseToken,
        binding_id: str,
        object_node_id: str,
    ) -> StoredEvent | None:
        _require_text(binding_id, "binding_id")
        _require_text(object_node_id, "object_node_id")
        async with self._lock:
            with self._transaction(token):
                row = self._connection.execute(
                    "SELECT * FROM events WHERE binding_id = ? "
                    "AND object_node_id = ? ORDER BY event_id DESC LIMIT 1",
                    (binding_id, object_node_id),
                ).fetchone()
                return None if row is None else _stored_event(row)

    async def latest_events_for_surface(
        self,
        token: LeaseToken,
        binding_id: str,
        *,
        event_name: str,
        surface_node_id: str,
    ) -> tuple[StoredEvent, ...]:
        """Return the most recently observed fact for each surface object."""

        _require_text(binding_id, "binding_id")
        _require_text(event_name, "event_name")
        _require_text(surface_node_id, "surface_node_id")
        async with self._lock:
            with self._transaction(token):
                rows = self._connection.execute(
                    "SELECT events.* FROM events JOIN ("
                    "SELECT object_node_id, MAX(event_id) AS latest_id FROM events "
                    "WHERE binding_id = ? AND event_name = ? AND surface_node_id = ? "
                    "GROUP BY object_node_id"
                    ") latest ON events.event_id = latest.latest_id "
                    "ORDER BY events.event_id",
                    (binding_id, event_name, surface_node_id),
                ).fetchall()
                return tuple(_stored_event(row) for row in rows)

    async def scheduler_snapshot(self, binding_id: str) -> SchedulerSnapshot:
        _require_text(binding_id, "binding_id")
        async with self._lock:
            self._require_open()
            row = self._connection.execute(
                "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
            ).fetchone()
            if row is None:
                raise KeyError(binding_id)
            return _scheduler_snapshot(row)

    async def restart_pending_quiet_window(
        self,
        token: LeaseToken,
        binding_id: str,
        *,
        observed_at: float,
        quiet_window_seconds: float,
    ) -> SchedulerSnapshot:
        """Restart settling from this process's canonical observation time.

        Persisted wall-clock deadlines cannot prove that GitHub was quiet while
        the Wrapper was offline.  A fresh owner therefore gives ordinary
        pending events a full quiet period.  A previously authorized urgent
        hint remains immediately ready.
        """

        _require_text(binding_id, "binding_id")
        _require_positive(quiet_window_seconds, "quiet_window_seconds")
        if not math.isfinite(observed_at):
            raise ValueError("observed_at must be finite")
        async with self._lock:
            with self._transaction(token):
                scheduler = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                if scheduler is None:
                    raise KeyError(binding_id)
                if scheduler["active_turn_handle"] is not None:
                    raise StateConflict(
                        "cannot restart quiet window while a provider turn is active"
                    )
                rows = self._connection.execute(
                    "SELECT urgent FROM events WHERE binding_id = ? "
                    "AND state = 'pending' AND wake_eligible = 1 "
                    "AND scheduled_at IS NOT NULL",
                    (binding_id,),
                ).fetchall()
                if rows:
                    deadline = (
                        observed_at
                        if any(bool(row["urgent"]) for row in rows)
                        else observed_at + quiet_window_seconds
                    )
                    status = "pending"
                else:
                    deadline = None
                    status = "idle"
                self._connection.execute(
                    "UPDATE schedulers SET quiet_deadline = ?, "
                    "transport_status = ?, updated_at = ? WHERE binding_id = ?",
                    (deadline, status, self._clock(), binding_id),
                )
                updated = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                assert updated is not None
                return _scheduler_snapshot(updated)

    async def schedule_event(
        self,
        token: LeaseToken,
        event_id: int,
        *,
        quiet_window_seconds: float,
        received_at: float,
    ) -> SchedulerSnapshot:
        _require_positive(quiet_window_seconds, "quiet_window_seconds")
        if not math.isfinite(received_at):
            raise ValueError("received_at must be finite")
        async with self._lock:
            with self._transaction(token):
                event = self._connection.execute(
                    "SELECT * FROM events WHERE event_id = ?", (event_id,)
                ).fetchone()
                if event is None:
                    raise KeyError(event_id)
                scheduler = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?",
                    (event["binding_id"],),
                ).fetchone()
                assert scheduler is not None
                if event["scheduled_at"] is not None:
                    return _scheduler_snapshot(scheduler)
                self._connection.execute(
                    "UPDATE events SET scheduled_at = ?, updated_at = ? "
                    "WHERE event_id = ?",
                    (received_at, self._clock(), event_id),
                )
                if not bool(event["wake_eligible"]) or event["state"] != "pending":
                    return _scheduler_snapshot(scheduler)
                active = scheduler["active_turn_handle"] is not None
                deadline = (
                    float(scheduler["quiet_deadline"])
                    if event["urgent"] and scheduler["quiet_deadline"] is not None
                    else received_at
                    if event["urgent"]
                    else received_at + quiet_window_seconds
                )
                self._connection.execute(
                    "UPDATE schedulers SET generation = generation + 1, "
                    "urgent_generation = urgent_generation + ?, quiet_deadline = ?, "
                    "transport_status = ?, updated_at = ? WHERE binding_id = ?",
                    (
                        int(event["urgent"]),
                        deadline,
                        "active-pending" if active else "pending",
                        self._clock(),
                        event["binding_id"],
                    ),
                )
                updated = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?",
                    (event["binding_id"],),
                ).fetchone()
                assert updated is not None
                return _scheduler_snapshot(updated)

    async def claim_ready_events(
        self,
        token: LeaseToken,
        binding_id: str,
        *,
        claim_handle: str,
        now: float,
    ) -> tuple[StoredEvent, ...]:
        _require_text(binding_id, "binding_id")
        _require_text(claim_handle, "claim_handle")
        if not math.isfinite(now):
            raise ValueError("now must be finite")
        async with self._lock:
            with self._transaction(token):
                scheduler = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                if scheduler is None:
                    raise KeyError(binding_id)
                if scheduler["active_turn_handle"] is not None:
                    return ()
                rows = self._connection.execute(
                    "SELECT * FROM events WHERE binding_id = ? AND state = 'pending' "
                    "AND wake_eligible = 1 AND scheduled_at IS NOT NULL "
                    "ORDER BY observed_at, event_id",
                    (binding_id,),
                ).fetchall()
                if not rows:
                    return ()
                urgent = any(bool(row["urgent"]) for row in rows)
                deadline = scheduler["quiet_deadline"]
                if not urgent and (deadline is None or float(deadline) > now):
                    return ()
                self._connection.execute(
                    "UPDATE schedulers SET active_turn_handle = ?, "
                    "transport_status = 'starting', quiet_deadline = NULL, "
                    "updated_at = ? WHERE binding_id = ?",
                    (claim_handle, self._clock(), binding_id),
                )
                return tuple(_stored_event(row) for row in rows)

    async def replace_active_turn_handle(
        self,
        token: LeaseToken,
        binding_id: str,
        *,
        expected_handle: str,
        active_turn_handle: str,
    ) -> SchedulerSnapshot:
        for value, name in (
            (binding_id, "binding_id"),
            (expected_handle, "expected_handle"),
            (active_turn_handle, "active_turn_handle"),
        ):
            _require_text(value, name)
        async with self._lock:
            with self._transaction(token):
                row = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                if row is None:
                    raise KeyError(binding_id)
                if row["active_turn_handle"] != expected_handle:
                    raise StateConflict("active turn claim was superseded")
                self._connection.execute(
                    "UPDATE schedulers SET active_turn_handle = ?, "
                    "transport_status = 'active', updated_at = ? WHERE binding_id = ?",
                    (active_turn_handle, self._clock(), binding_id),
                )
                updated = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                assert updated is not None
                return _scheduler_snapshot(updated)

    async def activate_claimed_turn(
        self,
        token: LeaseToken,
        binding_id: str,
        *,
        expected_claim_handle: str,
        active_turn_handle: str,
        delivered_event_ids: tuple[int, ...],
    ) -> SchedulerSnapshot:
        """Atomically bind the provider turn and acknowledge its input refs."""

        for value, name in (
            (binding_id, "binding_id"),
            (expected_claim_handle, "expected_claim_handle"),
            (active_turn_handle, "active_turn_handle"),
        ):
            _require_text(value, name)
        if not delivered_event_ids or any(value < 1 for value in delivered_event_ids):
            raise ValueError("delivered_event_ids must be non-empty and positive")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                scheduler = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                if scheduler is None:
                    raise KeyError(binding_id)
                if scheduler["active_turn_handle"] != expected_claim_handle:
                    raise StateConflict("active turn claim was superseded")
                for event_id in delivered_event_ids:
                    event = self._connection.execute(
                        "SELECT binding_id, state FROM events WHERE event_id = ?",
                        (event_id,),
                    ).fetchone()
                    if event is None:
                        raise KeyError(event_id)
                    if event["binding_id"] != binding_id:
                        raise StateConflict("claimed event belongs to another binding")
                    if event["state"] != EventState.PENDING.value:
                        raise InvalidTransition("claimed event is no longer pending")
                placeholders = ",".join("?" for _ in delivered_event_ids)
                self._connection.execute(
                    f"UPDATE events SET state = 'delivered', updated_at = ? "
                    f"WHERE event_id IN ({placeholders})",
                    (now, *delivered_event_ids),
                )
                self._connection.execute(
                    "UPDATE schedulers SET active_turn_handle = ?, "
                    "transport_status = 'active', updated_at = ? WHERE binding_id = ?",
                    (active_turn_handle, now, binding_id),
                )
                updated = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                assert updated is not None
                return _scheduler_snapshot(updated)

    async def ready_events_for_active_turn(
        self,
        token: LeaseToken,
        binding_id: str,
        *,
        active_turn_handle: str,
        now: float,
    ) -> tuple[StoredEvent, ...]:
        _require_text(binding_id, "binding_id")
        _require_text(active_turn_handle, "active_turn_handle")
        if not math.isfinite(now):
            raise ValueError("now must be finite")
        async with self._lock:
            with self._transaction(token):
                scheduler = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                if scheduler is None:
                    raise KeyError(binding_id)
                if scheduler["active_turn_handle"] != active_turn_handle:
                    raise StateConflict("active turn does not own scheduler")
                rows = self._connection.execute(
                    "SELECT * FROM events WHERE binding_id = ? AND state = 'pending' "
                    "AND wake_eligible = 1 AND scheduled_at IS NOT NULL "
                    "ORDER BY observed_at, event_id",
                    (binding_id,),
                ).fetchall()
                if not rows:
                    return ()
                urgent = any(bool(row["urgent"]) for row in rows)
                deadline = scheduler["quiet_deadline"]
                if not urgent and (deadline is None or float(deadline) > now):
                    return ()
                return tuple(_stored_event(row) for row in rows)

    async def mark_events_delivered(
        self, token: LeaseToken, event_ids: tuple[int, ...]
    ) -> None:
        if not event_ids:
            return
        if any(value < 1 for value in event_ids):
            raise ValueError("event_ids must be positive")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                for event_id in event_ids:
                    row = self._connection.execute(
                        "SELECT state FROM events WHERE event_id = ?", (event_id,)
                    ).fetchone()
                    if row is None:
                        raise KeyError(event_id)
                    if row["state"] == EventState.DELIVERED.value:
                        continue
                    if row["state"] != EventState.PENDING.value:
                        raise InvalidTransition(
                            "only pending events can be delivered to provider"
                        )
                    self._connection.execute(
                        "UPDATE events SET state = 'delivered', updated_at = ? "
                        "WHERE event_id = ?",
                        (now, event_id),
                    )

    async def mark_events_superseded(
        self, token: LeaseToken, event_ids: tuple[int, ...]
    ) -> None:
        """Mechanically retire pending refs that no longer have a bound surface."""

        if not event_ids:
            return
        if any(value < 1 for value in event_ids):
            raise ValueError("event_ids must be positive")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                for event_id in event_ids:
                    row = self._connection.execute(
                        "SELECT state FROM events WHERE event_id = ?", (event_id,)
                    ).fetchone()
                    if row is None:
                        raise KeyError(event_id)
                    if row["state"] == EventState.SUPERSEDED.value:
                        continue
                    if row["state"] != EventState.PENDING.value:
                        raise InvalidTransition(
                            "only pending events can be superseded before delivery"
                        )
                    self._connection.execute(
                        "UPDATE events SET state = 'superseded', updated_at = ? "
                        "WHERE event_id = ?",
                        (now, event_id),
                    )

    async def supersede_pending_events_for_surface(
        self,
        token: LeaseToken,
        binding_id: str,
        surface_node_id: str,
    ) -> int:
        """Retire pending refs after a native surface association disappears."""

        _require_text(binding_id, "binding_id")
        _require_text(surface_node_id, "surface_node_id")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                cursor = self._connection.execute(
                    "UPDATE events SET state = 'superseded', updated_at = ? "
                    "WHERE binding_id = ? AND surface_node_id = ? "
                    "AND state = 'pending'",
                    (now, binding_id, surface_node_id),
                )
                scheduler = self._connection.execute(
                    "SELECT active_turn_handle FROM schedulers WHERE binding_id = ?",
                    (binding_id,),
                ).fetchone()
                if scheduler is None:
                    raise KeyError(binding_id)
                remaining = self._connection.execute(
                    "SELECT COUNT(*) FROM events WHERE binding_id = ? "
                    "AND state = 'pending' AND wake_eligible = 1 "
                    "AND scheduled_at IS NOT NULL",
                    (binding_id,),
                ).fetchone()[0]
                active = scheduler["active_turn_handle"] is not None
                status = (
                    "active-pending"
                    if active and remaining
                    else "active"
                    if active
                    else "pending"
                    if remaining
                    else "idle"
                )
                self._connection.execute(
                    "UPDATE schedulers SET transport_status = ?, "
                    "quiet_deadline = CASE WHEN ? = 0 THEN NULL "
                    "ELSE quiet_deadline END, updated_at = ? WHERE binding_id = ?",
                    (status, remaining, now, binding_id),
                )
                return int(cursor.rowcount)

    async def finish_active_turn(
        self,
        token: LeaseToken,
        binding_id: str,
        *,
        active_turn_handle: str,
    ) -> SchedulerSnapshot:
        _require_text(binding_id, "binding_id")
        _require_text(active_turn_handle, "active_turn_handle")
        async with self._lock:
            with self._transaction(token):
                row = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                if row is None:
                    raise KeyError(binding_id)
                if row["active_turn_handle"] != active_turn_handle:
                    raise StateConflict("terminal turn does not own scheduler")
                pending = self._connection.execute(
                    "SELECT COUNT(*) FROM events WHERE binding_id = ? "
                    "AND state = 'pending' AND wake_eligible = 1 "
                    "AND scheduled_at IS NOT NULL",
                    (binding_id,),
                ).fetchone()[0]
                self._connection.execute(
                    "UPDATE schedulers SET active_turn_handle = NULL, "
                    "transport_status = ?, quiet_deadline = CASE WHEN ? = 0 THEN NULL "
                    "ELSE quiet_deadline END, updated_at = ? WHERE binding_id = ?",
                    (
                        "pending" if pending else "idle",
                        pending,
                        self._clock(),
                        binding_id,
                    ),
                )
                updated = self._connection.execute(
                    "SELECT * FROM schedulers WHERE binding_id = ?", (binding_id,)
                ).fetchone()
                assert updated is not None
                return _scheduler_snapshot(updated)

    async def prepare_mirror_revision(
        self,
        token: LeaseToken,
        *,
        turn_id: str,
        binding_id: str,
        target_node_id: str,
        terminal_state: str | None,
        revision: int,
        aggregate_digest: str,
        chunks: tuple[MirrorChunkIntent, ...],
    ) -> tuple[StoredMirrorChunk, ...]:
        for value, name in (
            (turn_id, "turn_id"),
            (binding_id, "binding_id"),
            (target_node_id, "target_node_id"),
            (aggregate_digest, "aggregate_digest"),
        ):
            _require_text(value, name)
        if revision < 0:
            raise ValueError("revision must not be negative")
        if not chunks:
            raise ValueError("mirror revision must contain at least one chunk")
        if tuple(chunk.chunk_index for chunk in chunks) != tuple(range(len(chunks))):
            raise ValueError("mirror chunk indexes must be contiguous from zero")
        for chunk in chunks:
            _require_text(chunk.body_digest, "chunk.body_digest")
            _require_text(chunk.ownership_marker, "chunk.ownership_marker")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                existing = self._connection.execute(
                    "SELECT * FROM mirrors WHERE turn_id = ?", (turn_id,)
                ).fetchone()
                if existing is None:
                    self._connection.execute(
                        "INSERT INTO mirrors(turn_id, binding_id, target_node_id, "
                        "terminal_state, revision, body_digest, updated_at) "
                        "VALUES (?, ?, ?, ?, ?, ?, ?)",
                        (
                            turn_id,
                            binding_id,
                            target_node_id,
                            terminal_state,
                            revision,
                            aggregate_digest,
                            now,
                        ),
                    )
                else:
                    if (
                        existing["binding_id"] != binding_id
                        or existing["target_node_id"] != target_node_id
                    ):
                        raise StateConflict("mirror turn identity cannot be rewritten")
                    if int(existing["revision"]) > revision:
                        raise InvalidTransition("mirror revision cannot move backward")
                    if (
                        int(existing["revision"]) == revision
                        and existing["body_digest"] != aggregate_digest
                    ):
                        raise StateConflict(
                            "mirror revision was reused for different content"
                        )
                    self._connection.execute(
                        "UPDATE mirrors SET terminal_state = ?, revision = ?, "
                        "body_digest = ?, updated_at = ? WHERE turn_id = ?",
                        (
                            terminal_state,
                            revision,
                            aggregate_digest,
                            now,
                            turn_id,
                        ),
                    )
                self._connection.execute(
                    "UPDATE mirror_chunks SET active = 0, updated_at = ? "
                    "WHERE turn_id = ?",
                    (now, turn_id),
                )
                for chunk in chunks:
                    row = self._connection.execute(
                        "SELECT * FROM mirror_chunks WHERE turn_id = ? "
                        "AND chunk_index = ?",
                        (turn_id, chunk.chunk_index),
                    ).fetchone()
                    if row is None:
                        self._connection.execute(
                            "INSERT INTO mirror_chunks(turn_id, chunk_index, revision, "
                            "body_digest, ownership_marker, active, updated_at) "
                            "VALUES (?, ?, ?, ?, ?, 1, ?)",
                            (
                                turn_id,
                                chunk.chunk_index,
                                revision,
                                chunk.body_digest,
                                chunk.ownership_marker,
                                now,
                            ),
                        )
                    else:
                        if row["ownership_marker"] != chunk.ownership_marker:
                            raise StateConflict(
                                "mirror chunk ownership marker cannot be rewritten"
                            )
                        self._connection.execute(
                            "UPDATE mirror_chunks SET revision = ?, body_digest = ?, "
                            "active = 1, updated_at = ? WHERE turn_id = ? "
                            "AND chunk_index = ?",
                            (
                                revision,
                                chunk.body_digest,
                                now,
                                turn_id,
                                chunk.chunk_index,
                            ),
                        )
                return self._mirror_chunks(turn_id, active_only=True)

    async def mirror_chunks(
        self, turn_id: str, *, active_only: bool = True
    ) -> tuple[StoredMirrorChunk, ...]:
        _require_text(turn_id, "turn_id")
        async with self._lock:
            self._require_open()
            return self._mirror_chunks(turn_id, active_only=active_only)

    async def mirror_state(self, turn_id: str) -> MirrorState | None:
        _require_text(turn_id, "turn_id")
        async with self._lock:
            self._require_open()
            row = self._connection.execute(
                "SELECT turn_id, binding_id, target_node_id, terminal_state, revision "
                "FROM mirrors WHERE turn_id = ?",
                (turn_id,),
            ).fetchone()
            if row is None:
                return None
            return MirrorState(
                turn_id=str(row["turn_id"]),
                binding_id=str(row["binding_id"]),
                target_node_id=str(row["target_node_id"]),
                terminal_state=(
                    None
                    if row["terminal_state"] is None
                    else str(row["terminal_state"])
                ),
                revision=int(row["revision"]),
            )

    async def record_mirror_chunk_remote(
        self,
        token: LeaseToken,
        *,
        turn_id: str,
        chunk_index: int,
        expected_body_digest: str,
        remote_id: str,
        remote_url: str,
        remote_digest: str,
    ) -> StoredMirrorChunk:
        for value, name in (
            (turn_id, "turn_id"),
            (expected_body_digest, "expected_body_digest"),
            (remote_id, "remote_id"),
            (remote_url, "remote_url"),
            (remote_digest, "remote_digest"),
        ):
            _require_text(value, name)
        if chunk_index < 0:
            raise ValueError("chunk_index must not be negative")
        if remote_digest != expected_body_digest:
            raise StateConflict("remote mirror digest does not match intended body")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                row = self._connection.execute(
                    "SELECT * FROM mirror_chunks WHERE turn_id = ? "
                    "AND chunk_index = ?",
                    (turn_id, chunk_index),
                ).fetchone()
                if row is None:
                    raise KeyError((turn_id, chunk_index))
                if row["body_digest"] != expected_body_digest:
                    raise StateConflict("mirror chunk advanced before remote ack")
                if row["remote_id"] is not None and row["remote_id"] != remote_id:
                    raise StateConflict("mirror chunk maps to another remote comment")
                self._connection.execute(
                    "UPDATE mirror_chunks SET remote_id = ?, remote_url = ?, "
                    "remote_digest = ?, updated_at = ? WHERE turn_id = ? "
                    "AND chunk_index = ?",
                    (
                        remote_id,
                        remote_url,
                        remote_digest,
                        now,
                        turn_id,
                        chunk_index,
                    ),
                )
                updated = self._connection.execute(
                    "SELECT * FROM mirror_chunks WHERE turn_id = ? "
                    "AND chunk_index = ?",
                    (turn_id, chunk_index),
                ).fetchone()
                assert updated is not None
                return _stored_mirror_chunk(updated)

    async def mark_mirror_conflict(
        self, token: LeaseToken, turn_id: str
    ) -> None:
        _require_text(turn_id, "turn_id")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                cursor = self._connection.execute(
                    "UPDATE mirrors SET ownership_state = 'conflict', updated_at = ? "
                    "WHERE turn_id = ?",
                    (now, turn_id),
                )
                if cursor.rowcount != 1:
                    raise KeyError(turn_id)

    async def transition_event(
        self,
        token: LeaseToken,
        event_id: int,
        target: EventState,
    ) -> StoredEvent:
        if target not in {EventState.DELIVERED, EventState.SUPERSEDED}:
            raise InvalidTransition("pending is not a valid event transition target")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                row = self._connection.execute(
                    "SELECT * FROM events WHERE event_id = ?", (event_id,)
                ).fetchone()
                if row is None:
                    raise KeyError(event_id)
                current = EventState(row["state"])
                if current == target:
                    return _stored_event(row)
                if current != EventState.PENDING:
                    raise InvalidTransition(f"event is already {current.value}")
                self._connection.execute(
                    "UPDATE events SET state = ?, updated_at = ? WHERE event_id = ?",
                    (target.value, now, event_id),
                )
                updated = self._connection.execute(
                    "SELECT * FROM events WHERE event_id = ?", (event_id,)
                ).fetchone()
                assert updated is not None
                return _stored_event(updated)

    async def resolve_event_permission(
        self,
        token: LeaseToken,
        event_id: int,
        permission_role: str,
    ) -> StoredEvent:
        """Resolve urgency after canonical actor permission is known."""

        _require_text(permission_role, "permission_role")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                row = self._connection.execute(
                    "SELECT * FROM events WHERE event_id = ?", (event_id,)
                ).fetchone()
                if row is None:
                    raise KeyError(event_id)
                existing_role = row["permission_role"]
                if existing_role is not None and existing_role != permission_role:
                    raise StateConflict("event permission evidence cannot be rewritten")
                urgent = bool(row["mention_detected"]) and (
                    permission_role in TRUSTED_URGENT_PERMISSION_ROLES
                )
                self._connection.execute(
                    "UPDATE events SET permission_role = ?, urgent = ?, updated_at = ? "
                    "WHERE event_id = ?",
                    (permission_role, int(urgent), now, event_id),
                )
                if (
                    urgent
                    and not bool(row["urgent"])
                    and bool(row["wake_eligible"])
                    and row["state"] == EventState.PENDING.value
                    and row["scheduled_at"] is not None
                ):
                    scheduler = self._connection.execute(
                        "SELECT active_turn_handle FROM schedulers "
                        "WHERE binding_id = ?",
                        (row["binding_id"],),
                    ).fetchone()
                    assert scheduler is not None
                    self._connection.execute(
                        "UPDATE schedulers SET urgent_generation = "
                        "urgent_generation + 1, transport_status = ?, "
                        "updated_at = ? WHERE binding_id = ?",
                        (
                            "active-pending"
                            if scheduler["active_turn_handle"] is not None
                            else "pending",
                            now,
                            row["binding_id"],
                        ),
                    )
                updated = self._connection.execute(
                    "SELECT * FROM events WHERE event_id = ?", (event_id,)
                ).fetchone()
                assert updated is not None
                return _stored_event(updated)

    async def enqueue_outbox(
        self, token: LeaseToken, intent: OutboxIntent
    ) -> tuple[StoredOutbox, bool]:
        _validate_outbox_intent(intent)
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                row = self._connection.execute(
                    "SELECT * FROM outbox WHERE operation_key = ?",
                    (intent.operation_key,),
                ).fetchone()
                if row is not None:
                    stored = _stored_outbox(row)
                    if (
                        stored.binding_id,
                        stored.operation_kind,
                        stored.target_node_id,
                        stored.intended_digest,
                    ) != (
                        intent.binding_id,
                        intent.operation_kind,
                        intent.target_node_id,
                        intent.intended_digest,
                    ):
                        raise StateConflict(
                            "outbox operation key was reused for a different intent"
                        )
                    return stored, False
                self._connection.execute(
                    "INSERT INTO outbox(operation_key, binding_id, operation_kind, "
                    "target_node_id, intended_digest, state, created_at, updated_at) "
                    "VALUES (?, ?, ?, ?, ?, 'pending', ?, ?)",
                    (
                        intent.operation_key,
                        intent.binding_id,
                        intent.operation_kind,
                        intent.target_node_id,
                        intent.intended_digest,
                        now,
                        now,
                    ),
                )
                created = self._connection.execute(
                    "SELECT * FROM outbox WHERE operation_key = ?",
                    (intent.operation_key,),
                ).fetchone()
                assert created is not None
                return _stored_outbox(created), True

    async def start_outbox_send(
        self, token: LeaseToken, operation_key: str
    ) -> StoredOutbox:
        return await self._transition_outbox(
            token,
            operation_key,
            allowed={OutboxState.PENDING},
            target=OutboxState.SENDING,
            increment_attempts=True,
        )

    async def mark_outbox_uncertain(
        self, token: LeaseToken, operation_key: str
    ) -> StoredOutbox:
        return await self._transition_outbox(
            token,
            operation_key,
            allowed={OutboxState.SENDING},
            target=OutboxState.UNCERTAIN,
        )

    async def recover_sending_outbox(
        self, token: LeaseToken, operation_key: str
    ) -> StoredOutbox:
        """Treat an owner-recovered in-flight send as outcome-uncertain."""

        return await self._transition_outbox(
            token,
            operation_key,
            allowed={OutboxState.SENDING},
            target=OutboxState.UNCERTAIN,
        )

    async def acknowledge_outbox(
        self,
        token: LeaseToken,
        operation_key: str,
        *,
        remote_id: str,
        remote_digest: str,
    ) -> StoredOutbox:
        _require_text(remote_id, "remote_id")
        _require_text(remote_digest, "remote_digest")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                row = self._outbox_row(operation_key)
                current = OutboxState(row["state"])
                if remote_digest != row["intended_digest"]:
                    raise StateConflict("remote evidence digest does not match intent")
                if current == OutboxState.ACKED:
                    if row["remote_id"] != remote_id:
                        raise StateConflict("acked operation maps to another remote id")
                    return _stored_outbox(row)
                if current not in {OutboxState.SENDING, OutboxState.UNCERTAIN}:
                    raise InvalidTransition(f"cannot acknowledge {current.value} outbox")
                self._connection.execute(
                    "UPDATE outbox SET state = 'acked', remote_id = ?, remote_digest = ?, "
                    "reconciliation_checked_at = ?, updated_at = ? "
                    "WHERE operation_key = ?",
                    (remote_id, remote_digest, now, now, operation_key),
                )
                return _stored_outbox(self._outbox_row(operation_key))

    async def reconcile_outbox_absent(
        self, token: LeaseToken, operation_key: str
    ) -> StoredOutbox:
        """Permit retry only after canonical remote absence was checked."""

        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                row = self._outbox_row(operation_key)
                if OutboxState(row["state"]) != OutboxState.UNCERTAIN:
                    raise InvalidTransition("only uncertain outbox can reconcile absent")
                self._connection.execute(
                    "UPDATE outbox SET state = 'pending', reconciliation_checked_at = ?, "
                    "updated_at = ? WHERE operation_key = ?",
                    (now, now, operation_key),
                )
                return _stored_outbox(self._outbox_row(operation_key))

    async def _transition_outbox(
        self,
        token: LeaseToken,
        operation_key: str,
        *,
        allowed: set[OutboxState],
        target: OutboxState,
        increment_attempts: bool = False,
    ) -> StoredOutbox:
        _require_text(operation_key, "operation_key")
        async with self._lock:
            now = self._clock()
            with self._transaction(token, now=now):
                row = self._outbox_row(operation_key)
                current = OutboxState(row["state"])
                if current == target:
                    return _stored_outbox(row)
                if current not in allowed:
                    raise InvalidTransition(
                        f"cannot move outbox from {current.value} to {target.value}"
                    )
                attempts = int(row["attempts"]) + int(increment_attempts)
                self._connection.execute(
                    "UPDATE outbox SET state = ?, attempts = ?, updated_at = ? "
                    "WHERE operation_key = ?",
                    (target.value, attempts, now, operation_key),
                )
                return _stored_outbox(self._outbox_row(operation_key))

    def _find_event(
        self, event_key: str, delivery_id: str | None
    ) -> sqlite3.Row | None:
        row = self._connection.execute(
            "SELECT * FROM events WHERE event_key = ?", (event_key,)
        ).fetchone()
        if row is not None or delivery_id is None:
            return row
        return self._connection.execute(
            "SELECT * FROM events WHERE delivery_id = ?", (delivery_id,)
        ).fetchone()

    def _outbox_row(self, operation_key: str) -> sqlite3.Row:
        row = self._connection.execute(
            "SELECT * FROM outbox WHERE operation_key = ?", (operation_key,)
        ).fetchone()
        if row is None:
            raise KeyError(operation_key)
        return row

    def _mirror_chunks(
        self, turn_id: str, *, active_only: bool
    ) -> tuple[StoredMirrorChunk, ...]:
        suffix = " AND active = 1" if active_only else ""
        rows = self._connection.execute(
            "SELECT * FROM mirror_chunks WHERE turn_id = ?"
            + suffix
            + " ORDER BY chunk_index",
            (turn_id,),
        ).fetchall()
        return tuple(_stored_mirror_chunk(row) for row in rows)

    @contextmanager
    def _transaction(
        self,
        token: LeaseToken | None = None,
        *,
        now: float | None = None,
    ) -> Iterator[None]:
        self._require_open()
        self._connection.execute("BEGIN IMMEDIATE")
        try:
            if token is not None:
                self._assert_lease(token, self._clock() if now is None else now)
            yield
        except BaseException:
            self._connection.rollback()
            raise
        else:
            self._connection.commit()

    def _assert_lease(self, token: LeaseToken, now: float) -> None:
        row = self._connection.execute(
            "SELECT owner_id, generation, lease_expires_at FROM owner_lease "
            "WHERE singleton = 1"
        ).fetchone()
        assert row is not None
        if (
            row["owner_id"] != token.owner_id
            or int(row["generation"]) != token.generation
            or float(row["lease_expires_at"]) <= now
        ):
            raise StaleLease("runtime owner lease is expired or superseded")

    def _require_open(self) -> None:
        if self._closed:
            raise StoreError("transport store is closed")


def _stored_event(row: sqlite3.Row) -> StoredEvent:
    return StoredEvent(
        event_id=int(row["event_id"]),
        event_key=row["event_key"],
        delivery_id=row["delivery_id"],
        binding_id=row["binding_id"],
        event_name=row["event_name"],
        action=row["action"],
        object_node_id=row["object_node_id"],
        surface_kind=row["surface_kind"],
        surface_node_id=row["surface_node_id"],
        object_version=row["object_version"],
        body_digest=row["body_digest"],
        canonical_url=row["canonical_url"],
        observed_at=float(row["observed_at"]),
        actor_node_id=row["actor_node_id"],
        actor_login=row["actor_login"],
        author_association=row["author_association"],
        permission_role=row["permission_role"],
        mention_detected=bool(row["mention_detected"]),
        urgent=bool(row["urgent"]),
        wake_eligible=bool(row["wake_eligible"]),
        scheduled_at=(
            None if row["scheduled_at"] is None else float(row["scheduled_at"])
        ),
        state=EventState(row["state"]),
    )


def _binding(row: sqlite3.Row) -> Binding:
    return Binding(
        binding_id=row["binding_id"],
        repository_node_id=row["repository_node_id"],
        repository_full_name=row["repository_full_name"],
        issue_node_id=row["issue_node_id"],
        issue_number=int(row["issue_number"]),
        issue_url=row["issue_url"],
        thread_address=row["thread_address"],
        agent_identity=row["agent_identity"],
        wrapper_identity=row["wrapper_identity"],
        trusted_permission=row["trusted_permission"],
        instruction_digest=row["instruction_digest"],
        lifecycle=BindingLifecycle(row["lifecycle"]),
    )


def _surface_route(row: sqlite3.Row) -> SurfaceRoute:
    return SurfaceRoute(
        binding_id=row["binding_id"],
        surface_kind=SurfaceKind(row["surface_kind"]),
        repository_node_id=row["repository_node_id"],
        repository_full_name=row["repository_full_name"],
        surface_node_id=row["surface_node_id"],
        surface_number=int(row["surface_number"]),
        canonical_url=row["canonical_url"],
        association_version=row["association_version"],
        active=bool(row["active"]),
    )


def _scheduler_snapshot(row: sqlite3.Row) -> SchedulerSnapshot:
    return SchedulerSnapshot(
        binding_id=row["binding_id"],
        generation=int(row["generation"]),
        quiet_deadline=(
            None if row["quiet_deadline"] is None else float(row["quiet_deadline"])
        ),
        urgent_generation=int(row["urgent_generation"]),
        active_turn_handle=row["active_turn_handle"],
        transport_status=row["transport_status"],
    )


def _stored_outbox(row: sqlite3.Row) -> StoredOutbox:
    return StoredOutbox(
        operation_key=row["operation_key"],
        binding_id=row["binding_id"],
        operation_kind=row["operation_kind"],
        target_node_id=row["target_node_id"],
        intended_digest=row["intended_digest"],
        state=OutboxState(row["state"]),
        attempts=int(row["attempts"]),
        remote_id=row["remote_id"],
        remote_digest=row["remote_digest"],
        reconciliation_checked_at=row["reconciliation_checked_at"],
    )


def _stored_mirror_chunk(row: sqlite3.Row) -> StoredMirrorChunk:
    return StoredMirrorChunk(
        turn_id=row["turn_id"],
        chunk_index=int(row["chunk_index"]),
        revision=int(row["revision"]),
        body_digest=row["body_digest"],
        ownership_marker=row["ownership_marker"],
        remote_id=row["remote_id"],
        remote_url=row["remote_url"],
        remote_digest=row["remote_digest"],
        active=bool(row["active"]),
    )


def _event_matches(stored: StoredEvent, envelope: EventEnvelope) -> bool:
    return (
        stored.event_key,
        stored.delivery_id,
        stored.binding_id,
        stored.event_name,
        stored.action,
        stored.object_node_id,
        stored.surface_kind,
        stored.surface_node_id,
        stored.object_version,
        stored.body_digest,
        stored.canonical_url,
        stored.actor_node_id,
        stored.actor_login,
        stored.author_association,
        stored.permission_role,
        stored.mention_detected,
        stored.urgent,
        stored.wake_eligible,
    ) == (
        envelope.event_key,
        envelope.delivery_id,
        envelope.binding_id,
        envelope.event_name,
        envelope.action,
        envelope.object_node_id,
        envelope.surface_kind,
        envelope.surface_node_id,
        envelope.object_version,
        envelope.body_digest,
        envelope.canonical_url,
        envelope.actor_node_id,
        envelope.actor_login,
        envelope.author_association,
        envelope.permission_role,
        envelope.mention_detected,
        envelope.urgent,
        envelope.wake_eligible,
    )


def _validate_binding(binding: Binding) -> None:
    for name in (
        "binding_id",
        "repository_node_id",
        "repository_full_name",
        "issue_node_id",
        "issue_url",
        "thread_address",
        "agent_identity",
        "wrapper_identity",
        "trusted_permission",
        "instruction_digest",
    ):
        _require_text(getattr(binding, name), name)
    if binding.issue_number < 1:
        raise ValueError("issue_number must be positive")


def _validate_surface_route(route: SurfaceRoute) -> None:
    if route.surface_kind != SurfaceKind.PULL_REQUEST:
        raise ValueError("only pull_request aliases belong in surface_routes")
    for name in (
        "binding_id",
        "repository_node_id",
        "repository_full_name",
        "surface_node_id",
        "canonical_url",
        "association_version",
    ):
        _require_text(getattr(route, name), name)
    if route.surface_number < 1:
        raise ValueError("surface_number must be positive")


def _validate_event(envelope: EventEnvelope) -> None:
    for name in (
        "event_key",
        "binding_id",
        "event_name",
        "action",
        "object_node_id",
        "surface_kind",
        "surface_node_id",
        "object_version",
        "body_digest",
        "canonical_url",
    ):
        _require_text(getattr(envelope, name), name)
    if envelope.delivery_id is not None:
        _require_text(envelope.delivery_id, "delivery_id")
    for name in (
        "actor_node_id",
        "actor_login",
        "author_association",
        "permission_role",
    ):
        value = getattr(envelope, name)
        if value is not None:
            _require_text(value, name)
    if not math.isfinite(envelope.observed_at):
        raise ValueError("observed_at must be finite")


def _validate_outbox_intent(intent: OutboxIntent) -> None:
    for name in (
        "operation_key",
        "binding_id",
        "operation_kind",
        "target_node_id",
        "intended_digest",
    ):
        _require_text(getattr(intent, name), name)


def _require_text(value: str, name: str) -> None:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{name} must be a non-empty string")


def _require_positive(value: float, name: str) -> None:
    if (
        not isinstance(value, (int, float))
        or isinstance(value, bool)
        or not math.isfinite(value)
        or value <= 0
    ):
        raise ValueError(f"{name} must be positive")


def _compare_object_versions(left: str, right: str) -> int | None:
    """Compare provider timestamps; opaque fallback versions stay unordered."""

    try:
        left_value = datetime.fromisoformat(left.replace("Z", "+00:00"))
        right_value = datetime.fromisoformat(right.replace("Z", "+00:00"))
        return (left_value > right_value) - (left_value < right_value)
    except (TypeError, ValueError):
        return None
