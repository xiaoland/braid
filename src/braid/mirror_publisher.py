"""Durable create/edit projection of one Agent turn onto GitHub comments."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
from typing import Protocol

from braid.github_api import GitHubApiError, RemoteComment
from braid.mirror_render import (
    RenderedMirrorChunk,
    render_mirror_chunks,
)
from braid.store import (
    Binding,
    LeaseToken,
    MirrorChunkIntent,
    OutboxIntent,
    OutboxState,
    StoredMirrorChunk,
    StoredOutbox,
    TransportStore,
)
from braid.turn_projection import TurnProjectionSnapshot


class MirrorConflict(RuntimeError):
    """A remote comment no longer matches Wrapper ownership evidence."""


class GitHubCommentAuthority(Protocol):
    async def create_issue_comment(
        self, repository_full_name: str, issue_number: int, body: str
    ) -> RemoteComment: ...

    async def update_issue_comment(
        self,
        repository_full_name: str,
        comment_database_id: int,
        body: str,
    ) -> RemoteComment: ...

    async def get_issue_comment(
        self, repository_full_name: str, comment_database_id: int
    ) -> RemoteComment: ...

    async def find_issue_comments_by_evidence(
        self,
        repository_full_name: str,
        issue_number: int,
        *,
        author_login: str,
        body_digest: str,
        created_after: float,
        created_before: float,
    ) -> tuple[RemoteComment, ...]: ...


@dataclass(frozen=True, slots=True)
class MirrorTarget:
    surface_node_id: str
    surface_number: int


class TurnMirrorPublisher:
    def __init__(
        self,
        authority: GitHubCommentAuthority,
        store: TransportStore,
        owner_token: LeaseToken,
        *,
        max_comment_bytes: int = 60_000,
        create_recovery_window_seconds: float = 300.0,
    ) -> None:
        if max_comment_bytes < 1_024:
            raise ValueError("max_comment_bytes is too small for a safe mirror")
        if create_recovery_window_seconds <= 0:
            raise ValueError("create recovery window must be positive")
        self._authority = authority
        self._store = store
        self._owner_token = owner_token
        self._max_comment_bytes = max_comment_bytes
        self._create_recovery_window_seconds = create_recovery_window_seconds

    async def publish(
        self,
        *,
        binding: Binding,
        target: MirrorTarget,
        snapshot: TurnProjectionSnapshot,
        revision: int,
    ) -> tuple[StoredMirrorChunk, ...]:
        await self._recover_existing_chunks(binding, target, snapshot.turn_id)
        rendered = render_mirror_chunks(
            snapshot,
            revision=revision,
            max_comment_bytes=self._max_comment_bytes,
        )
        aggregate_digest = _aggregate_digest(rendered)
        stored = await self._store.prepare_mirror_revision(
            self._owner_token,
            turn_id=snapshot.turn_id,
            binding_id=binding.binding_id,
            target_node_id=target.surface_node_id,
            terminal_state=snapshot.terminal_status,
            revision=revision,
            aggregate_digest=aggregate_digest,
            chunks=tuple(
                MirrorChunkIntent(
                    chunk_index=chunk.index,
                    body_digest=chunk.body_digest,
                    ownership_marker=chunk.ownership_marker,
                )
                for chunk in rendered
            ),
        )
        for chunk, record in zip(rendered, stored, strict=True):
            await self._publish_chunk(binding, target, snapshot.turn_id, chunk, record)
        return await self._store.mirror_chunks(snapshot.turn_id)

    async def _recover_existing_chunks(
        self,
        binding: Binding,
        target: MirrorTarget,
        turn_id: str,
    ) -> None:
        """Reconcile a prior revision before it can be overwritten locally."""

        records = await self._store.mirror_chunks(turn_id)
        for record in records:
            operation_kind = (
                "comment-create" if record.remote_id is None else "comment-update"
            )
            operation_target = record.remote_id or target.surface_node_id
            operation_key = (
                f"mirror:{turn_id}:chunk:{record.chunk_index}:"
                f"revision:{record.revision}:{operation_kind}"
            )
            outbox, _ = await self._store.enqueue_outbox(
                self._owner_token,
                OutboxIntent(
                    operation_key=operation_key,
                    binding_id=binding.binding_id,
                    operation_kind=operation_kind,
                    target_node_id=operation_target,
                    intended_digest=record.body_digest,
                ),
            )
            chunk = RenderedMirrorChunk(
                index=record.chunk_index,
                count=len(records),
                body="",
                body_digest=record.body_digest,
                ownership_marker=record.ownership_marker,
            )
            if outbox.state == OutboxState.ACKED:
                if outbox.remote_id is None:
                    raise MirrorConflict("acked mirror outbox has no remote id")
                remote = await self._authority.get_issue_comment(
                    binding.repository_full_name,
                    _database_id(outbox.remote_id),
                )
                if remote.body_digest != record.body_digest:
                    await self._conflict(turn_id)
                await self._record_remote(turn_id, chunk, remote)
                continue
            if outbox.state == OutboxState.SENDING:
                outbox = await self._store.recover_sending_outbox(
                    self._owner_token, operation_key
                )
            if outbox.state == OutboxState.UNCERTAIN:
                await self._reconcile_uncertain(
                    binding,
                    target,
                    turn_id,
                    chunk,
                    record,
                    outbox,
                )

    async def publish_fyi(
        self,
        *,
        binding: Binding,
        turn_id: str,
        target: MirrorTarget,
        canonical_comment_url: str,
    ) -> RemoteComment | None:
        if not canonical_comment_url:
            raise ValueError("canonical_comment_url must not be empty")
        body = (
            "FYI：本 turn 同时涉及多个 GitHub surface，无法确定唯一回复位置；"
            f"canonical 回复见 {canonical_comment_url}。"
        )
        digest = "sha256:" + hashlib.sha256(body.encode("utf-8")).hexdigest()
        operation_key = f"mirror-fyi:{turn_id}:target:{target.surface_node_id}"
        outbox, _ = await self._store.enqueue_outbox(
            self._owner_token,
            OutboxIntent(
                operation_key=operation_key,
                binding_id=binding.binding_id,
                operation_kind="comment-fyi-create",
                target_node_id=target.surface_node_id,
                intended_digest=digest,
            ),
        )
        if outbox.state == OutboxState.ACKED:
            return None
        if outbox.state == OutboxState.SENDING:
            outbox = await self._store.recover_sending_outbox(
                self._owner_token, operation_key
            )
        if outbox.state == OutboxState.UNCERTAIN:
            matches = await self._authority.find_issue_comments_by_evidence(
                binding.repository_full_name,
                target.surface_number,
                author_login=binding.wrapper_identity,
                body_digest=digest,
                created_after=outbox.created_at - 60.0,
                created_before=(
                    outbox.created_at + self._create_recovery_window_seconds
                ),
            )
            if len(matches) == 1:
                remote = matches[0]
                await self._store.acknowledge_outbox(
                    self._owner_token,
                    operation_key,
                    remote_id=str(remote.database_id),
                    remote_digest=remote.body_digest,
                )
                return remote
            raise MirrorConflict(
                "uncertain FYI create has no unique canonical recovery match"
            )
        await self._store.start_outbox_send(self._owner_token, operation_key)
        try:
            remote = await self._authority.create_issue_comment(
                binding.repository_full_name, target.surface_number, body
            )
        except GitHubApiError:
            await self._store.mark_outbox_uncertain(
                self._owner_token, operation_key
            )
            raise
        if remote.body_digest != digest:
            await self._store.mark_outbox_uncertain(
                self._owner_token, operation_key
            )
            raise MirrorConflict("GitHub acknowledged a different FYI body")
        await self._store.acknowledge_outbox(
            self._owner_token,
            operation_key,
            remote_id=str(remote.database_id),
            remote_digest=remote.body_digest,
        )
        return remote

    async def _publish_chunk(
        self,
        binding: Binding,
        target: MirrorTarget,
        turn_id: str,
        chunk: RenderedMirrorChunk,
        record: StoredMirrorChunk,
    ) -> None:
        if record.remote_id is not None:
            current = await self._read_owned_remote(binding, turn_id, record)
            operation_kind = "comment-update"
            operation_target = record.remote_id
        else:
            current = None
            operation_kind = "comment-create"
            operation_target = target.surface_node_id

        operation_key = (
            f"mirror:{turn_id}:chunk:{chunk.index}:revision:{record.revision}:"
            f"{operation_kind}"
        )
        outbox, _ = await self._store.enqueue_outbox(
            self._owner_token,
            OutboxIntent(
                operation_key=operation_key,
                binding_id=binding.binding_id,
                operation_kind=operation_kind,
                target_node_id=operation_target,
                intended_digest=chunk.body_digest,
            ),
        )
        if current is not None and current.body_digest == chunk.body_digest:
            if outbox.state == OutboxState.PENDING:
                outbox = await self._store.start_outbox_send(
                    self._owner_token, operation_key
                )
            await self._store.acknowledge_outbox(
                self._owner_token,
                operation_key,
                remote_id=str(current.database_id),
                remote_digest=current.body_digest,
            )
            await self._record_remote(turn_id, chunk, current)
            return
        if outbox.state == OutboxState.ACKED:
            if outbox.remote_id is None:
                raise MirrorConflict("acked mirror outbox has no remote id")
            remote = await self._authority.get_issue_comment(
                binding.repository_full_name, _database_id(outbox.remote_id)
            )
            if remote.body_digest != chunk.body_digest:
                await self._conflict(turn_id)
            await self._record_remote(turn_id, chunk, remote)
            return
        if outbox.state == OutboxState.SENDING:
            outbox = await self._store.recover_sending_outbox(
                self._owner_token, operation_key
            )
        if outbox.state == OutboxState.UNCERTAIN:
            recovered = await self._reconcile_uncertain(
                binding, target, turn_id, chunk, record, outbox
            )
            if recovered:
                return

        await self._store.start_outbox_send(
            self._owner_token, operation_key
        )
        try:
            if operation_kind == "comment-create":
                remote = await self._authority.create_issue_comment(
                    binding.repository_full_name,
                    target.surface_number,
                    chunk.body,
                )
            else:
                assert record.remote_id is not None
                remote = await self._authority.update_issue_comment(
                    binding.repository_full_name,
                    _database_id(record.remote_id),
                    chunk.body,
                )
        except GitHubApiError:
            await self._store.mark_outbox_uncertain(
                self._owner_token, operation_key
            )
            raise
        if remote.body_digest != chunk.body_digest:
            await self._store.mark_outbox_uncertain(
                self._owner_token, operation_key
            )
            raise MirrorConflict("GitHub acknowledged a different mirror body")
        await self._store.acknowledge_outbox(
            self._owner_token,
            operation_key,
            remote_id=str(remote.database_id),
            remote_digest=remote.body_digest,
        )
        await self._record_remote(turn_id, chunk, remote)

    async def _read_owned_remote(
        self,
        binding: Binding,
        turn_id: str,
        record: StoredMirrorChunk,
    ) -> RemoteComment:
        assert record.remote_id is not None
        remote = await self._authority.get_issue_comment(
            binding.repository_full_name, _database_id(record.remote_id)
        )
        if (
            record.remote_digest is not None
            and remote.body_digest not in {record.remote_digest, record.body_digest}
        ):
            await self._conflict(turn_id)
        return remote

    async def _reconcile_uncertain(
        self,
        binding: Binding,
        target: MirrorTarget,
        turn_id: str,
        chunk: RenderedMirrorChunk,
        record: StoredMirrorChunk,
        outbox: StoredOutbox,
    ) -> bool:
        if record.remote_id is None:
            matches = await self._authority.find_issue_comments_by_evidence(
                binding.repository_full_name,
                target.surface_number,
                author_login=binding.wrapper_identity,
                body_digest=chunk.body_digest,
                created_after=outbox.created_at - 60.0,
                created_before=(
                    outbox.created_at + self._create_recovery_window_seconds
                ),
            )
            if len(matches) == 1:
                remote = matches[0]
                await self._store.acknowledge_outbox(
                    self._owner_token,
                    outbox.operation_key,
                    remote_id=str(remote.database_id),
                    remote_digest=remote.body_digest,
                )
                await self._record_remote(turn_id, chunk, remote)
                return True
            raise MirrorConflict(
                "uncertain mirror create has no unique canonical recovery match"
            )
        else:
            remote = await self._authority.get_issue_comment(
                binding.repository_full_name, _database_id(record.remote_id)
            )
            if remote.body_digest == chunk.body_digest:
                await self._store.acknowledge_outbox(
                    self._owner_token,
                    outbox.operation_key,
                    remote_id=str(remote.database_id),
                    remote_digest=remote.body_digest,
                )
                await self._record_remote(turn_id, chunk, remote)
                return True
            if record.remote_digest is not None and (
                remote.body_digest != record.remote_digest
            ):
                await self._conflict(turn_id)
        await self._store.reconcile_outbox_absent(
            self._owner_token, outbox.operation_key
        )
        return False

    async def _record_remote(
        self,
        turn_id: str,
        chunk: RenderedMirrorChunk,
        remote: RemoteComment,
    ) -> None:
        await self._store.record_mirror_chunk_remote(
            self._owner_token,
            turn_id=turn_id,
            chunk_index=chunk.index,
            expected_body_digest=chunk.body_digest,
            remote_id=str(remote.database_id),
            remote_url=remote.url,
            remote_digest=remote.body_digest,
        )

    async def _conflict(self, turn_id: str) -> None:
        await self._store.mark_mirror_conflict(self._owner_token, turn_id)
        raise MirrorConflict("remote mirror ownership changed")


def _aggregate_digest(chunks: tuple[RenderedMirrorChunk, ...]) -> str:
    joined = "\n".join(chunk.body_digest for chunk in chunks).encode("ascii")
    return "sha256:" + hashlib.sha256(joined).hexdigest()


def _database_id(value: str) -> int:
    try:
        result = int(value)
    except ValueError as error:
        raise MirrorConflict("remote comment id is not a database integer") from error
    if result < 1:
        raise MirrorConflict("remote comment id is not positive")
    return result
