"""Least-privilege GitHub App API boundary.

GitHub remains canonical.  Returned objects are bounded transport references;
secret tokens and private keys are never represented in operator-facing text.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping, Set
from dataclasses import dataclass, field
from datetime import datetime
import hashlib
import json
import time
from typing import Any
from urllib.parse import quote

from aiohttp import ClientSession
import jwt

from github_agent_bridge.store import TRUSTED_URGENT_PERMISSION_ROLES
from github_agent_bridge.github_webhook import (
    digest_comment_body,
    digest_issue_content,
    has_visible_agent_mention,
)


DEFAULT_REST_API = "https://api.github.com"
DEFAULT_GRAPHQL_API = "https://api.github.com/graphql"
DEFAULT_REST_API_VERSION = "2022-11-28"


class GitHubApiError(RuntimeError):
    """A safe failure at the GitHub authority boundary."""


class AssociationConflict(GitHubApiError):
    """More than one current native-associated PR prevents exact routing."""


class CanonicalStateUnavailable(GitHubApiError):
    """Canonical GitHub state cannot be represented within a bounded scan."""


@dataclass(frozen=True, slots=True)
class InstallationToken:
    installation_id: int
    expires_at: float
    token: str = field(repr=False)


@dataclass(frozen=True, slots=True)
class IssueReference:
    repository_node_id: str
    repository_full_name: str
    default_branch: str
    issue_node_id: str
    issue_number: int
    issue_url: str
    state: str
    updated_at: str
    content_digest: str


@dataclass(frozen=True, slots=True)
class PullRequestReference:
    repository_node_id: str
    repository_full_name: str
    pr_node_id: str
    pr_number: int
    pr_url: str
    state: str
    is_draft: bool


@dataclass(frozen=True, slots=True)
class RepositoryPermission:
    permission: str
    role_name: str

    @property
    def scheduling_role(self) -> str:
        if self.role_name in TRUSTED_URGENT_PERMISSION_ROLES:
            return self.role_name
        if self.permission in TRUSTED_URGENT_PERMISSION_ROLES:
            return self.permission
        return self.permission


@dataclass(frozen=True, slots=True)
class RemoteComment:
    database_id: int
    node_id: str
    url: str
    updated_at: str
    body_digest: str


@dataclass(frozen=True, slots=True)
class WebhookConfiguration:
    url: str
    content_type: str
    insecure_ssl: str


@dataclass(frozen=True, slots=True)
class IssueCommentSnapshot:
    object_node_id: str
    canonical_url: str
    object_version: str
    body_digest: str
    author_login: str | None
    author_association: str | None
    mention_detected: bool
    wake_eligible: bool
    is_minimized: bool
    minimized_reason: str | None


@dataclass(frozen=True, slots=True)
class PullRequestSnapshot:
    reference: PullRequestReference
    object_version: str
    body_digest: str
    head_ref_oid: str
    actor_node_id: str | None
    actor_login: str | None
    author_association: str | None
    mention_detected: bool
    wake_eligible: bool


@dataclass(frozen=True, slots=True)
class PullRequestCommentSnapshot:
    object_node_id: str
    canonical_url: str
    object_version: str
    body_digest: str
    actor_node_id: str | None
    actor_login: str | None
    author_association: str | None
    mention_detected: bool
    wake_eligible: bool
    is_minimized: bool
    minimized_reason: str | None


@dataclass(frozen=True, slots=True)
class PullRequestReviewSnapshot:
    object_node_id: str
    canonical_url: str
    object_version: str
    body_digest: str
    actor_node_id: str | None
    actor_login: str | None
    author_association: str | None
    mention_detected: bool
    wake_eligible: bool
    state: str
    is_minimized: bool
    minimized_reason: str | None


@dataclass(frozen=True, slots=True)
class PullRequestReviewThreadSnapshot:
    object_node_id: str
    canonical_url: str
    object_version: str
    body_digest: str
    is_resolved: bool


@dataclass(frozen=True, slots=True)
class PullRequestCanonicalState:
    pull_request: PullRequestSnapshot
    conversation_comments: tuple[PullRequestCommentSnapshot, ...]
    reviews: tuple[PullRequestReviewSnapshot, ...]
    review_comments: tuple[PullRequestCommentSnapshot, ...]
    review_threads: tuple[PullRequestReviewThreadSnapshot, ...]


class GitHubAppClient:
    """GitHub REST/GraphQL client authenticated as one App installation."""

    def __init__(
        self,
        session: ClientSession,
        *,
        app_id: int,
        private_key: bytes,
        rest_api: str = DEFAULT_REST_API,
        graphql_api: str = DEFAULT_GRAPHQL_API,
        rest_api_version: str = DEFAULT_REST_API_VERSION,
        clock: Callable[[], float] = time.time,
    ) -> None:
        if app_id < 1:
            raise ValueError("app_id must be positive")
        if not private_key:
            raise ValueError("private_key must not be empty")
        self._session = session
        self._app_id = app_id
        self._private_key = private_key
        self._rest_api = rest_api.rstrip("/")
        self._graphql_api = graphql_api
        self._rest_api_version = rest_api_version
        self._clock = clock
        self._tokens: dict[str, InstallationToken] = {}

    async def issue_reference(
        self, repository_full_name: str, issue_number: int
    ) -> IssueReference:
        owner, repository = _split_repository(repository_full_name)
        if issue_number < 1:
            raise ValueError("issue_number must be positive")
        data = await self._graphql(
            repository_full_name,
            query=_ISSUE_REFERENCE_QUERY,
            variables={"owner": owner, "name": repository, "number": issue_number},
            operation="read Issue reference",
        )
        repository_value = _object(data, "repository", "GraphQL data")
        issue = _object(repository_value, "issue", "repository")
        default_branch_ref = _object(
            repository_value, "defaultBranchRef", "repository"
        )
        title = _string(issue, "title", "issue")
        body = _nullable_string(issue, "body", "issue")
        state = _string(issue, "state", "issue")
        return IssueReference(
            repository_node_id=_string(repository_value, "id", "repository"),
            repository_full_name=_string(
                repository_value, "nameWithOwner", "repository"
            ),
            default_branch=_string(default_branch_ref, "name", "defaultBranchRef"),
            issue_node_id=_string(issue, "id", "issue"),
            issue_number=_positive_int(issue, "number", "issue"),
            issue_url=_string(issue, "url", "issue"),
            state=state,
            updated_at=_string(issue, "updatedAt", "issue"),
            content_digest=digest_issue_content(
                title=title, body=body, state=state.lower()
            ),
        )

    async def webhook_configuration(self) -> WebhookConfiguration:
        value = await self._request_json(
            "GET",
            f"{self._rest_api}/app/hook/config",
            headers=self._app_headers(self._app_jwt(self._clock())),
            operation="read App webhook configuration",
        )
        return _webhook_configuration(value)

    async def update_webhook_configuration(
        self, *, url: str, webhook_secret: bytes
    ) -> WebhookConfiguration:
        if not url.startswith("https://"):
            raise ValueError("webhook URL must use HTTPS")
        try:
            secret = webhook_secret.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ValueError("webhook secret must be UTF-8") from error
        if not secret:
            raise ValueError("webhook secret must not be empty")
        value = await self._request_json(
            "PATCH",
            f"{self._rest_api}/app/hook/config",
            headers=self._app_headers(self._app_jwt(self._clock())),
            json_body={
                "content_type": "json",
                "insecure_ssl": "0",
                "secret": secret,
                "url": url,
            },
            operation="update App webhook configuration",
        )
        return _webhook_configuration(value)

    async def associated_pull_requests(
        self, repository_full_name: str, issue_number: int
    ) -> tuple[PullRequestReference, ...]:
        owner, repository = _split_repository(repository_full_name)
        if issue_number < 1:
            raise ValueError("issue_number must be positive")
        references = []
        cursor: str | None = None
        for _ in range(100):
            data = await self._graphql(
                repository_full_name,
                query=_ASSOCIATED_PULL_REQUESTS_QUERY,
                variables={
                    "owner": owner,
                    "name": repository,
                    "number": issue_number,
                    "cursor": cursor,
                },
                operation="read native Issue association",
            )
            repository_value = _object(data, "repository", "GraphQL data")
            issue = _object(repository_value, "issue", "repository")
            connection = _object(
                issue, "closedByPullRequestsReferences", "issue"
            )
            nodes = connection.get("nodes")
            if not isinstance(nodes, list):
                raise GitHubApiError("GitHub returned malformed associated PR nodes")
            for value in nodes:
                if not isinstance(value, dict):
                    raise GitHubApiError(
                        "GitHub returned malformed associated PR node"
                    )
                pr_repository = _object(value, "repository", "pull request")
                references.append(
                    PullRequestReference(
                        repository_node_id=_string(
                            pr_repository, "id", "pull request repository"
                        ),
                        repository_full_name=_string(
                            pr_repository,
                            "nameWithOwner",
                            "pull request repository",
                        ),
                        pr_node_id=_string(value, "id", "pull request"),
                        pr_number=_positive_int(value, "number", "pull request"),
                        pr_url=_string(value, "url", "pull request"),
                        state=_string(value, "state", "pull request"),
                        is_draft=_boolean(value, "isDraft", "pull request"),
                    )
                )
            cursor = _next_cursor(connection, "associated PRs")
            if cursor is None:
                return tuple(references)
        raise GitHubApiError("GitHub associated PR pagination exceeded safety bound")

    async def current_associated_pull_request(
        self, repository_full_name: str, issue_number: int
    ) -> PullRequestReference | None:
        references = await self.associated_pull_requests(
            repository_full_name, issue_number
        )
        current = tuple(reference for reference in references if reference.state == "OPEN")
        if len(current) > 1:
            raise AssociationConflict(
                "bound Issue has more than one open native-associated PR"
            )
        if current:
            return current[0]
        # A single associated PR remains unambiguous after close/merge, which
        # lets reconciliation emit its terminal state before routing stops.
        # Multiple historical closed PRs have no mechanically current member.
        return references[0] if len(references) == 1 else None

    async def pull_request_state(
        self,
        reference: PullRequestReference,
        *,
        self_logins: Set[str] = frozenset(),
    ) -> PullRequestCanonicalState:
        """Read one bounded, canonical PR surface without retaining prose."""

        normalized_self_logins = {value.casefold() for value in self_logins}
        pull_request = await self._pull_request_object(
            reference,
            query=_PULL_REQUEST_STATE_QUERY,
            variables={},
            operation="reconcile pull request state",
        )
        state = _known_value(
            _string(pull_request, "state", "pull request"),
            frozenset({"OPEN", "CLOSED", "MERGED"}),
            "pull request state",
        )
        is_draft = _boolean(pull_request, "isDraft", "pull request")
        head_ref_oid = _string(pull_request, "headRefOid", "pull request")
        updated_at = _string(pull_request, "updatedAt", "pull request")
        body = _string(pull_request, "body", "pull request")
        title = _string(pull_request, "title", "pull request")
        actor_node_id, actor_login = _actor_reference(
            pull_request, "author", "pull request"
        )
        self_origin = _is_self_login(actor_login, normalized_self_logins)
        pr_reference = PullRequestReference(
            repository_node_id=reference.repository_node_id,
            repository_full_name=reference.repository_full_name,
            pr_node_id=reference.pr_node_id,
            pr_number=reference.pr_number,
            pr_url=reference.pr_url,
            state=state,
            is_draft=is_draft,
        )
        pr_snapshot = PullRequestSnapshot(
            reference=pr_reference,
            object_version=_pull_request_version(
                updated_at=updated_at,
                state=state,
                is_draft=is_draft,
                head_ref_oid=head_ref_oid,
            ),
            body_digest=_digest_json(
                {
                    "body": body,
                    "head_ref_oid": head_ref_oid,
                    "is_draft": is_draft,
                    "state": state,
                    "title": title,
                }
            ),
            head_ref_oid=head_ref_oid,
            actor_node_id=actor_node_id,
            actor_login=actor_login,
            author_association=_string(
                pull_request, "authorAssociation", "pull request"
            ),
            mention_detected=(
                state == "OPEN"
                and not self_origin
                and has_visible_agent_mention(body)
            ),
            wake_eligible=not self_origin,
        )

        conversation_nodes = await self._pull_request_connection_nodes(
            reference,
            query=_PULL_REQUEST_COMMENTS_QUERY,
            connection_name="comments",
            operation="reconcile pull request conversation comments",
        )
        conversation_comments = tuple(
            _comment_snapshot(
                value,
                owner="pull request conversation comment",
                normalized_self_logins=normalized_self_logins,
            )
            for value in conversation_nodes
        )

        review_nodes = await self._pull_request_connection_nodes(
            reference,
            query=_PULL_REQUEST_REVIEWS_QUERY,
            connection_name="reviews",
            operation="reconcile pull request reviews",
        )
        reviews = tuple(
            _review_snapshot(
                value,
                normalized_self_logins=normalized_self_logins,
            )
            for value in review_nodes
        )

        thread_nodes = await self._pull_request_connection_nodes(
            reference,
            query=_PULL_REQUEST_REVIEW_THREADS_QUERY,
            connection_name="reviewThreads",
            operation="reconcile pull request review threads",
        )
        review_comments: list[PullRequestCommentSnapshot] = []
        review_threads: list[PullRequestReviewThreadSnapshot] = []
        for thread in thread_nodes:
            comments = _object(thread, "comments", "pull request review thread")
            if _next_cursor(comments, "review thread comments") is not None:
                raise CanonicalStateUnavailable(
                    "GitHub review thread comments exceed nested pagination bound"
                )
            comment_nodes = _nodes(comments, "review thread comments")
            snapshots = tuple(
                _comment_snapshot(
                    value,
                    owner="pull request review comment",
                    normalized_self_logins=normalized_self_logins,
                )
                for value in comment_nodes
            )
            review_comments.extend(snapshots)
            is_resolved = _boolean(
                thread, "isResolved", "pull request review thread"
            )
            comment_node_ids = tuple(value.object_node_id for value in snapshots)
            digest = _digest_json(
                {
                    "comment_node_ids": comment_node_ids,
                    "is_resolved": is_resolved,
                }
            )
            review_threads.append(
                PullRequestReviewThreadSnapshot(
                    object_node_id=_string(
                        thread, "id", "pull request review thread"
                    ),
                    canonical_url=(
                        snapshots[0].canonical_url if snapshots else reference.pr_url
                    ),
                    object_version="canonical:" + digest.removeprefix("sha256:"),
                    body_digest=digest,
                    is_resolved=is_resolved,
                )
            )
        _require_unique_node_ids(review_comments, "pull request review comments")

        return PullRequestCanonicalState(
            pull_request=pr_snapshot,
            conversation_comments=conversation_comments,
            reviews=reviews,
            review_comments=tuple(review_comments),
            review_threads=tuple(review_threads),
        )

    async def _pull_request_connection_nodes(
        self,
        reference: PullRequestReference,
        *,
        query: str,
        connection_name: str,
        operation: str,
    ) -> tuple[dict[str, Any], ...]:
        nodes: list[dict[str, Any]] = []
        cursor: str | None = None
        for _ in range(100):
            pull_request = await self._pull_request_object(
                reference,
                query=query,
                variables={"cursor": cursor},
                operation=operation,
            )
            connection = _object(pull_request, connection_name, "pull request")
            nodes.extend(_nodes(connection, connection_name))
            cursor = _next_cursor(connection, connection_name)
            if cursor is None:
                _require_unique_mapping_node_ids(nodes, connection_name)
                return tuple(nodes)
        raise CanonicalStateUnavailable(
            f"GitHub {connection_name} pagination exceeded safety bound"
        )

    async def _pull_request_object(
        self,
        reference: PullRequestReference,
        *,
        query: str,
        variables: Mapping[str, object],
        operation: str,
    ) -> dict[str, Any]:
        owner, repository = _split_repository(reference.repository_full_name)
        data = await self._graphql(
            reference.repository_full_name,
            query=query,
            variables={
                "owner": owner,
                "name": repository,
                "number": reference.pr_number,
                **variables,
            },
            operation=operation,
        )
        repository_value = _object(data, "repository", "GraphQL data")
        pull_request = _object(repository_value, "pullRequest", "repository")
        identity = (
            _string(repository_value, "id", "repository"),
            _string(repository_value, "nameWithOwner", "repository"),
            _string(pull_request, "id", "pull request"),
            _positive_int(pull_request, "number", "pull request"),
            _string(pull_request, "url", "pull request"),
        )
        expected = (
            reference.repository_node_id,
            reference.repository_full_name,
            reference.pr_node_id,
            reference.pr_number,
            reference.pr_url,
        )
        if identity != expected:
            raise CanonicalStateUnavailable(
                "canonical pull request identity no longer matches association"
            )
        return pull_request

    async def repository_permission(
        self, repository_full_name: str, actor_login: str
    ) -> RepositoryPermission:
        if not actor_login:
            raise ValueError("actor_login must not be empty")
        owner, repository = _split_repository(repository_full_name)
        token = await self._installation_token(repository_full_name)
        path = (
            f"/repos/{quote(owner, safe='')}/{quote(repository, safe='')}"
            f"/collaborators/{quote(actor_login, safe='')}/permission"
        )
        payload = await self._rest_json(
            "GET",
            path,
            token=token.token,
            operation="read repository permission",
        )
        return RepositoryPermission(
            permission=_string(payload, "permission", "permission response"),
            role_name=_string(payload, "role_name", "permission response"),
        )

    async def issue_comments(
        self,
        repository_full_name: str,
        issue_number: int,
        *,
        self_logins: Set[str] = frozenset(),
    ) -> tuple[IssueCommentSnapshot, ...]:
        owner, repository = _split_repository(repository_full_name)
        if issue_number < 1:
            raise ValueError("issue_number must be positive")
        comments: list[IssueCommentSnapshot] = []
        cursor: str | None = None
        normalized_self_logins = {value.casefold() for value in self_logins}
        for _ in range(100):
            data = await self._graphql(
                repository_full_name,
                query=_ISSUE_COMMENTS_QUERY,
                variables={
                    "owner": owner,
                    "name": repository,
                    "number": issue_number,
                    "cursor": cursor,
                },
                operation="reconcile Issue comments",
            )
            repository_value = _object(data, "repository", "GraphQL data")
            issue = _object(repository_value, "issue", "repository")
            connection = _object(issue, "comments", "issue")
            nodes = connection.get("nodes")
            if not isinstance(nodes, list):
                raise GitHubApiError("GitHub returned malformed Issue comment nodes")
            for value in nodes:
                if not isinstance(value, dict):
                    raise GitHubApiError(
                        "GitHub returned malformed Issue comment node"
                    )
                author_value = value.get("author")
                if author_value is not None and not isinstance(author_value, dict):
                    raise GitHubApiError("GitHub returned malformed comment author")
                author_login = (
                    None
                    if author_value is None
                    else _nullable_string(author_value, "login", "comment author")
                )
                body = _string(value, "body", "Issue comment")
                is_minimized = _boolean(value, "isMinimized", "Issue comment")
                updated_at = _string(value, "updatedAt", "Issue comment")
                last_edited_at = _nullable_string(
                    value, "lastEditedAt", "Issue comment"
                )
                self_origin = (
                    author_login is not None
                    and author_login.casefold() in normalized_self_logins
                )
                comments.append(
                    IssueCommentSnapshot(
                        object_node_id=_string(value, "id", "Issue comment"),
                        canonical_url=_string(value, "url", "Issue comment"),
                        object_version=last_edited_at or updated_at,
                        body_digest=digest_comment_body(body),
                        author_login=author_login,
                        author_association=_nullable_string(
                            value, "authorAssociation", "Issue comment"
                        ),
                        mention_detected=(
                            not is_minimized
                            and not self_origin
                            and has_visible_agent_mention(body)
                        ),
                        wake_eligible=not self_origin,
                        is_minimized=is_minimized,
                        minimized_reason=_nullable_string(
                            value, "minimizedReason", "Issue comment"
                        ),
                    )
                )
            cursor = _next_cursor(connection, "Issue comments")
            if cursor is None:
                return tuple(comments)
        raise GitHubApiError("GitHub Issue comment pagination exceeded safety bound")

    async def create_issue_comment(
        self, repository_full_name: str, issue_number: int, body: str
    ) -> RemoteComment:
        owner, repository = _split_repository(repository_full_name)
        if issue_number < 1:
            raise ValueError("issue_number must be positive")
        if not body:
            raise ValueError("comment body must not be empty")
        token = await self._installation_token(repository_full_name)
        payload = await self._rest_json(
            "POST",
            (
                f"/repos/{quote(owner, safe='')}/{quote(repository, safe='')}"
                f"/issues/{issue_number}/comments"
            ),
            token=token.token,
            json_body={"body": body},
            operation="create Issue comment",
        )
        return _remote_comment(payload)

    async def update_issue_comment(
        self,
        repository_full_name: str,
        comment_database_id: int,
        body: str,
    ) -> RemoteComment:
        owner, repository = _split_repository(repository_full_name)
        if comment_database_id < 1:
            raise ValueError("comment_database_id must be positive")
        if not body:
            raise ValueError("comment body must not be empty")
        token = await self._installation_token(repository_full_name)
        payload = await self._rest_json(
            "PATCH",
            (
                f"/repos/{quote(owner, safe='')}/{quote(repository, safe='')}"
                f"/issues/comments/{comment_database_id}"
            ),
            token=token.token,
            json_body={"body": body},
            operation="update Issue comment",
        )
        return _remote_comment(payload)

    async def get_issue_comment(
        self, repository_full_name: str, comment_database_id: int
    ) -> RemoteComment:
        owner, repository = _split_repository(repository_full_name)
        if comment_database_id < 1:
            raise ValueError("comment_database_id must be positive")
        token = await self._installation_token(repository_full_name)
        payload = await self._rest_json(
            "GET",
            (
                f"/repos/{quote(owner, safe='')}/{quote(repository, safe='')}"
                f"/issues/comments/{comment_database_id}"
            ),
            token=token.token,
            operation="read Issue comment",
        )
        return _remote_comment(payload)

    async def find_issue_comments_by_marker(
        self,
        repository_full_name: str,
        issue_number: int,
        ownership_marker: str,
    ) -> tuple[RemoteComment, ...]:
        owner, repository = _split_repository(repository_full_name)
        if issue_number < 1:
            raise ValueError("issue_number must be positive")
        if not ownership_marker:
            raise ValueError("ownership_marker must not be empty")
        token = await self._installation_token(repository_full_name)
        matches: list[RemoteComment] = []
        for page in range(1, 101):
            value = await self._request_value(
                "GET",
                (
                    f"{self._rest_api}/repos/{quote(owner, safe='')}"
                    f"/{quote(repository, safe='')}/issues/{issue_number}/comments"
                    f"?per_page=100&page={page}"
                ),
                headers=self._installation_headers(token.token),
                operation="find owned Issue comment",
            )
            if not isinstance(value, list):
                raise GitHubApiError(
                    "GitHub returned malformed Issue comment collection"
                )
            for candidate in value:
                if not isinstance(candidate, dict):
                    raise GitHubApiError(
                        "GitHub returned malformed Issue comment candidate"
                    )
                body = _string(candidate, "body", "Issue comment candidate")
                if ownership_marker in body:
                    matches.append(_remote_comment(candidate))
            if len(value) < 100:
                return tuple(matches)
        raise GitHubApiError("GitHub Issue comment pagination exceeded safety bound")

    async def _graphql(
        self,
        repository_full_name: str,
        *,
        query: str,
        variables: Mapping[str, object],
        operation: str,
    ) -> dict[str, Any]:
        token = await self._installation_token(repository_full_name)
        payload = await self._request_json(
            "POST",
            self._graphql_api,
            headers=self._installation_headers(token.token),
            json_body={"query": query, "variables": dict(variables)},
            operation=operation,
        )
        errors = payload.get("errors")
        if errors:
            raise GitHubApiError(f"GitHub GraphQL failed to {operation}")
        return _object(payload, "data", "GraphQL response")

    async def _installation_token(
        self, repository_full_name: str
    ) -> InstallationToken:
        cached = self._tokens.get(repository_full_name)
        now = self._clock()
        if cached is not None and cached.expires_at - 60 > now:
            return cached
        owner, repository = _split_repository(repository_full_name)
        app_jwt = self._app_jwt(now)
        installation = await self._request_json(
            "GET",
            (
                f"{self._rest_api}/repos/{quote(owner, safe='')}"
                f"/{quote(repository, safe='')}/installation"
            ),
            headers=self._app_headers(app_jwt),
            operation="locate App installation",
        )
        installation_id = _positive_int(installation, "id", "installation")
        token_response = await self._request_json(
            "POST",
            f"{self._rest_api}/app/installations/{installation_id}/access_tokens",
            headers=self._app_headers(app_jwt),
            json_body={
                "permissions": {
                    "issues": "write",
                    "pull_requests": "read",
                    "metadata": "read",
                }
            },
            operation="create installation token",
        )
        token = _string(token_response, "token", "installation token response")
        expires_at = _parse_timestamp(
            _string(token_response, "expires_at", "installation token response")
        )
        resolved = InstallationToken(installation_id, expires_at, token)
        self._tokens[repository_full_name] = resolved
        return resolved

    def _app_jwt(self, now: float) -> str:
        try:
            encoded = jwt.encode(
                {
                    "iat": int(now) - 60,
                    "exp": int(now) + 540,
                    "iss": str(self._app_id),
                },
                self._private_key,
                algorithm="RS256",
            )
        except Exception as error:
            raise GitHubApiError("cannot sign GitHub App authentication") from error
        if not isinstance(encoded, str):
            raise GitHubApiError("GitHub App authentication returned invalid token")
        return encoded

    async def _rest_json(
        self,
        method: str,
        path: str,
        *,
        token: str,
        operation: str,
        json_body: object | None = None,
    ) -> dict[str, Any]:
        return await self._request_json(
            method,
            self._rest_api + path,
            headers=self._installation_headers(token),
            json_body=json_body,
            operation=operation,
        )

    async def _request_json(
        self,
        method: str,
        url: str,
        *,
        headers: Mapping[str, str],
        operation: str,
        json_body: object | None = None,
    ) -> dict[str, Any]:
        value = await self._request_value(
            method,
            url,
            headers=headers,
            operation=operation,
            json_body=json_body,
        )
        if not isinstance(value, dict):
            raise GitHubApiError(f"GitHub returned malformed data for {operation}")
        return value

    async def _request_value(
        self,
        method: str,
        url: str,
        *,
        headers: Mapping[str, str],
        operation: str,
        json_body: object | None = None,
    ) -> Any:
        try:
            async with self._session.request(
                method, url, headers=headers, json=json_body
            ) as response:
                if response.status < 200 or response.status >= 300:
                    await response.read()
                    raise GitHubApiError(
                        f"GitHub failed to {operation} (HTTP {response.status})"
                    )
                value = await response.json(content_type=None)
        except GitHubApiError:
            raise
        except Exception as error:
            raise GitHubApiError(f"GitHub transport failed to {operation}") from error
        return value

    def _app_headers(self, token: str) -> dict[str, str]:
        return {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": self._rest_api_version,
        }

    def _installation_headers(self, token: str) -> dict[str, str]:
        return {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": self._rest_api_version,
        }


def _remote_comment(value: dict[str, Any]) -> RemoteComment:
    body = _string(value, "body", "comment response")
    return RemoteComment(
        database_id=_positive_int(value, "id", "comment response"),
        node_id=_string(value, "node_id", "comment response"),
        url=_string(value, "html_url", "comment response"),
        updated_at=_string(value, "updated_at", "comment response"),
        body_digest="sha256:" + hashlib.sha256(body.encode("utf-8")).hexdigest(),
    )


def _webhook_configuration(value: dict[str, Any]) -> WebhookConfiguration:
    return WebhookConfiguration(
        url=_string(value, "url", "webhook configuration"),
        content_type=_string(value, "content_type", "webhook configuration"),
        insecure_ssl=_string(value, "insecure_ssl", "webhook configuration"),
    )


def _split_repository(repository_full_name: str) -> tuple[str, str]:
    parts = repository_full_name.split("/")
    if len(parts) != 2 or not all(parts):
        raise ValueError("repository_full_name must have owner/name form")
    return parts[0], parts[1]


def _parse_timestamp(value: str) -> float:
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
    except ValueError as error:
        raise GitHubApiError("GitHub returned malformed token expiration") from error


def _object(value: Mapping[str, Any], key: str, owner: str) -> dict[str, Any]:
    member = value.get(key)
    if not isinstance(member, dict):
        raise GitHubApiError(f"GitHub returned malformed {owner}.{key}")
    return member


def _string(value: Mapping[str, Any], key: str, owner: str) -> str:
    member = value.get(key)
    if not isinstance(member, str) or not member:
        raise GitHubApiError(f"GitHub returned malformed {owner}.{key}")
    return member


def _positive_int(value: Mapping[str, Any], key: str, owner: str) -> int:
    member = value.get(key)
    if not isinstance(member, int) or isinstance(member, bool) or member < 1:
        raise GitHubApiError(f"GitHub returned malformed {owner}.{key}")
    return member


def _boolean(value: Mapping[str, Any], key: str, owner: str) -> bool:
    member = value.get(key)
    if not isinstance(member, bool):
        raise GitHubApiError(f"GitHub returned malformed {owner}.{key}")
    return member


def _nullable_string(
    value: Mapping[str, Any], key: str, owner: str
) -> str | None:
    member = value.get(key)
    if member is None:
        return None
    if not isinstance(member, str):
        raise GitHubApiError(f"GitHub returned malformed {owner}.{key}")
    return member


def _next_cursor(connection: Mapping[str, Any], owner: str) -> str | None:
    page_info = _object(connection, "pageInfo", owner)
    has_next_page = _boolean(page_info, "hasNextPage", f"{owner}.pageInfo")
    end_cursor = _nullable_string(
        page_info, "endCursor", f"{owner}.pageInfo"
    )
    if has_next_page and end_cursor is None:
        raise GitHubApiError(f"GitHub returned missing cursor for {owner}")
    return end_cursor if has_next_page else None


def _nodes(
    connection: Mapping[str, Any], owner: str
) -> tuple[dict[str, Any], ...]:
    values = connection.get("nodes")
    if not isinstance(values, list):
        raise GitHubApiError(f"GitHub returned malformed {owner} nodes")
    nodes = []
    for value in values:
        if not isinstance(value, dict):
            raise GitHubApiError(f"GitHub returned malformed {owner} node")
        nodes.append(value)
    return tuple(nodes)


def _actor_reference(
    value: Mapping[str, Any], key: str, owner: str
) -> tuple[str | None, str | None]:
    actor = value.get(key)
    if actor is None:
        return None, None
    if not isinstance(actor, dict):
        raise GitHubApiError(f"GitHub returned malformed {owner}.{key}")
    return (
        _string(actor, "id", f"{owner}.{key}"),
        _string(actor, "login", f"{owner}.{key}"),
    )


def _is_self_login(actor_login: str | None, self_logins: set[str]) -> bool:
    return actor_login is not None and actor_login.casefold() in self_logins


def _known_value(value: str, known: frozenset[str], owner: str) -> str:
    if value not in known:
        raise CanonicalStateUnavailable(f"GitHub returned unknown {owner}")
    return value


def _pull_request_version(
    *, updated_at: str, state: str, is_draft: bool, head_ref_oid: str
) -> str:
    return json.dumps(
        {
            "draft": is_draft,
            "head": head_ref_oid,
            "state": state,
            "updated_at": updated_at,
        },
        ensure_ascii=True,
        separators=(",", ":"),
        sort_keys=True,
    )


def _digest_json(value: object) -> str:
    encoded = json.dumps(
        value,
        ensure_ascii=True,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    return "sha256:" + hashlib.sha256(encoded).hexdigest()


def _comment_snapshot(
    value: Mapping[str, Any],
    *,
    owner: str,
    normalized_self_logins: set[str],
) -> PullRequestCommentSnapshot:
    body = _string(value, "body", owner)
    updated_at = _string(value, "updatedAt", owner)
    last_edited_at = _nullable_string(value, "lastEditedAt", owner)
    is_minimized = _boolean(value, "isMinimized", owner)
    actor_node_id, actor_login = _actor_reference(value, "author", owner)
    self_origin = _is_self_login(actor_login, normalized_self_logins)
    return PullRequestCommentSnapshot(
        object_node_id=_string(value, "id", owner),
        canonical_url=_string(value, "url", owner),
        object_version=last_edited_at or updated_at,
        body_digest=digest_comment_body(body),
        actor_node_id=actor_node_id,
        actor_login=actor_login,
        author_association=_string(value, "authorAssociation", owner),
        mention_detected=(
            not is_minimized
            and not self_origin
            and has_visible_agent_mention(body)
        ),
        wake_eligible=not self_origin,
        is_minimized=is_minimized,
        minimized_reason=_nullable_string(value, "minimizedReason", owner),
    )


def _review_snapshot(
    value: Mapping[str, Any], *, normalized_self_logins: set[str]
) -> PullRequestReviewSnapshot:
    owner = "pull request review"
    body = _string(value, "body", owner)
    state = _known_value(
        _string(value, "state", owner),
        frozenset({"APPROVED", "CHANGES_REQUESTED", "COMMENTED", "DISMISSED"}),
        owner + " state",
    )
    updated_at = _string(value, "updatedAt", owner)
    last_edited_at = _nullable_string(value, "lastEditedAt", owner)
    is_minimized = _boolean(value, "isMinimized", owner)
    actor_node_id, actor_login = _actor_reference(value, "author", owner)
    self_origin = _is_self_login(actor_login, normalized_self_logins)
    return PullRequestReviewSnapshot(
        object_node_id=_string(value, "id", owner),
        canonical_url=_string(value, "url", owner),
        object_version=last_edited_at or updated_at,
        body_digest=digest_comment_body(body),
        actor_node_id=actor_node_id,
        actor_login=actor_login,
        author_association=_string(value, "authorAssociation", owner),
        mention_detected=(
            state != "DISMISSED"
            and not is_minimized
            and not self_origin
            and has_visible_agent_mention(body)
        ),
        wake_eligible=not self_origin,
        state=state,
        is_minimized=is_minimized,
        minimized_reason=_nullable_string(value, "minimizedReason", owner),
    )


def _require_unique_mapping_node_ids(
    values: list[dict[str, Any]], owner: str
) -> None:
    node_ids = tuple(_string(value, "id", owner) for value in values)
    if len(node_ids) != len(set(node_ids)):
        raise CanonicalStateUnavailable(
            f"GitHub returned duplicate {owner} node IDs"
        )


def _require_unique_node_ids(
    values: list[PullRequestCommentSnapshot], owner: str
) -> None:
    node_ids = tuple(value.object_node_id for value in values)
    if len(node_ids) != len(set(node_ids)):
        raise CanonicalStateUnavailable(
            f"GitHub returned duplicate {owner} node IDs"
        )


_ISSUE_REFERENCE_QUERY = """
query IssueReference($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    id
    nameWithOwner
    defaultBranchRef { name }
    issue(number: $number) {
      id
      number
      url
      state
      updatedAt
      title
      body
    }
  }
}
"""


_ASSOCIATED_PULL_REQUESTS_QUERY = """
query AssociatedPullRequests(
  $owner: String!,
  $name: String!,
  $number: Int!,
  $cursor: String
) {
  repository(owner: $owner, name: $name) {
    issue(number: $number) {
      closedByPullRequestsReferences(
        first: 100,
        after: $cursor,
        includeClosedPrs: true,
        userLinkedOnly: false
      ) {
        nodes {
          id
          number
          url
          state
          isDraft
          repository { id nameWithOwner }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"""


_ISSUE_COMMENTS_QUERY = """
query IssueComments(
  $owner: String!,
  $name: String!,
  $number: Int!,
  $cursor: String
) {
  repository(owner: $owner, name: $name) {
    issue(number: $number) {
      comments(first: 100, after: $cursor) {
        nodes {
          id
          url
          body
          updatedAt
          lastEditedAt
          author { login }
          authorAssociation
          isMinimized
          minimizedReason
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"""


_PULL_REQUEST_STATE_QUERY = """
query PullRequestCanonicalState(
  $owner: String!,
  $name: String!,
  $number: Int!
) {
  repository(owner: $owner, name: $name) {
    id
    nameWithOwner
    pullRequest(number: $number) {
      id
      number
      url
      title
      body
      state
      isDraft
      headRefOid
      updatedAt
      author { id login }
      authorAssociation
    }
  }
}
"""


_PULL_REQUEST_COMMENTS_QUERY = """
query PullRequestConversationComments(
  $owner: String!,
  $name: String!,
  $number: Int!,
  $cursor: String
) {
  repository(owner: $owner, name: $name) {
    id
    nameWithOwner
    pullRequest(number: $number) {
      id
      number
      url
      comments(first: 100, after: $cursor) {
        nodes {
          id
          url
          body
          updatedAt
          lastEditedAt
          author { id login }
          authorAssociation
          isMinimized
          minimizedReason
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"""


_PULL_REQUEST_REVIEWS_QUERY = """
query PullRequestReviews(
  $owner: String!,
  $name: String!,
  $number: Int!,
  $cursor: String
) {
  repository(owner: $owner, name: $name) {
    id
    nameWithOwner
    pullRequest(number: $number) {
      id
      number
      url
      reviews(first: 100, after: $cursor) {
        nodes {
          id
          url
          body
          state
          updatedAt
          lastEditedAt
          author { id login }
          authorAssociation
          isMinimized
          minimizedReason
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"""


_PULL_REQUEST_REVIEW_THREADS_QUERY = """
query PullRequestReviewThreads(
  $owner: String!,
  $name: String!,
  $number: Int!,
  $cursor: String
) {
  repository(owner: $owner, name: $name) {
    id
    nameWithOwner
    pullRequest(number: $number) {
      id
      number
      url
      reviewThreads(first: 100, after: $cursor) {
        nodes {
          id
          isResolved
          comments(first: 100) {
            nodes {
              id
              url
              body
              updatedAt
              lastEditedAt
              author { id login }
              authorAssociation
              isMinimized
              minimizedReason
            }
            pageInfo { hasNextPage endCursor }
          }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"""
