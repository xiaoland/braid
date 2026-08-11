from __future__ import annotations

from datetime import datetime, timezone
import json
import unittest

from aiohttp import ClientSession, web
from aiohttp.test_utils import TestServer
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa
import jwt

from github_agent_bridge.github_api import (
    AssociationConflict,
    CanonicalStateUnavailable,
    GitHubApiError,
    GitHubAppClient,
)


class GitHubApiTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        key = rsa.generate_private_key(public_exponent=65_537, key_size=2_048)
        self.private_key = key.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        )
        self.public_key = key.public_key().public_bytes(
            serialization.Encoding.PEM,
            serialization.PublicFormat.SubjectPublicKeyInfo,
        )
        self.installation_lookups = 0
        self.token_creations = 0
        self.comment_bodies: list[str] = []
        self.nested_review_comment_page = False
        self.unknown_review_state = False

        app = web.Application()
        app.router.add_get(
            "/repos/{owner}/{repository}/installation", self._installation
        )
        app.router.add_post(
            "/app/installations/{installation_id}/access_tokens", self._token
        )
        app.router.add_post("/graphql", self._graphql)
        app.router.add_get(
            "/repos/{owner}/{repository}/collaborators/{actor}/permission",
            self._permission,
        )
        app.router.add_post(
            "/repos/{owner}/{repository}/issues/{number}/comments",
            self._create_comment,
        )
        app.router.add_get(
            "/repos/{owner}/{repository}/issues/{number}/comments",
            self._list_comments,
        )
        app.router.add_patch(
            "/repos/{owner}/{repository}/issues/comments/{comment_id}",
            self._update_comment,
        )
        app.router.add_get(
            "/repos/{owner}/{repository}/issues/comments/{comment_id}",
            self._get_comment,
        )
        app.router.add_get("/app/hook/config", self._get_webhook_config)
        app.router.add_patch("/app/hook/config", self._update_webhook_config)
        self.server = TestServer(app)
        await self.server.start_server()
        self.session = ClientSession()
        base_url = str(self.server.make_url("")).rstrip("/")
        self.client = GitHubAppClient(
            self.session,
            app_id=12_345,
            private_key=self.private_key,
            rest_api=base_url,
            graphql_api=base_url + "/graphql",
            clock=lambda: 2_000_000_000.0,
        )

    async def asyncTearDown(self) -> None:
        await self.session.close()
        await self.server.close()

    def _assert_app_authorization(self, request: web.Request) -> None:
        authorization = request.headers["Authorization"]
        self.assertTrue(authorization.startswith("Bearer "))
        payload = jwt.decode(
            authorization.removeprefix("Bearer "),
            self.public_key,
            algorithms=["RS256"],
            options={"verify_exp": False, "verify_iat": False},
        )
        self.assertEqual(payload["iss"], "12345")
        self.assertEqual(payload["iat"], 1_999_999_940)
        self.assertEqual(payload["exp"], 2_000_000_540)
        self.assertEqual(request.headers["X-GitHub-Api-Version"], "2022-11-28")

    def _assert_installation_authorization(self, request: web.Request) -> None:
        self.assertEqual(
            request.headers["Authorization"], "Bearer installation-token"
        )
        self.assertEqual(request.headers["X-GitHub-Api-Version"], "2022-11-28")

    async def _installation(self, request: web.Request) -> web.Response:
        self._assert_app_authorization(request)
        self.installation_lookups += 1
        return web.json_response({"id": 987})

    async def _token(self, request: web.Request) -> web.Response:
        self._assert_app_authorization(request)
        self.token_creations += 1
        self.assertEqual(request.match_info["installation_id"], "987")
        self.assertEqual(
            await request.json(),
            {
                "permissions": {
                    "issues": "write",
                    "pull_requests": "read",
                    "metadata": "read",
                }
            },
        )
        expires_at = datetime.fromtimestamp(
            2_000_003_600, timezone.utc
        ).isoformat().replace("+00:00", "Z")
        return web.json_response(
            {"token": "installation-token", "expires_at": expires_at}
        )

    async def _graphql(self, request: web.Request) -> web.Response:
        self._assert_installation_authorization(request)
        payload = await request.json()
        variables = payload["variables"]
        self.assertEqual(variables["owner"], "owner")
        self.assertEqual(variables["name"], "repository")
        query = payload["query"]
        if "IssueReference" in query:
            return web.json_response(
                {
                    "data": {
                        "repository": {
                            "id": "R_repository",
                            "nameWithOwner": "owner/repository",
                            "defaultBranchRef": {"name": "main"},
                            "issue": {
                                "id": "I_issue",
                                "number": variables["number"],
                                "url": "https://github.example/owner/repository/issues/17",
                                "state": "OPEN",
                                "updatedAt": "2026-08-10T12:00:00Z",
                                "title": "A bounded task",
                                "body": "Discuss the task before implementation.",
                            },
                        }
                    }
                }
            )
        if "AssociatedPullRequests" in query:
            paginated = variables["number"] == 100
            second_page = variables.get("cursor") == "page-2"
            count = 2 if variables["number"] == 99 else 1
            nodes = [
                {
                    "id": f"PR_{index + int(second_page)}",
                    "number": 40 + index + int(second_page),
                    "url": (
                        "https://github.example/owner/repository/pull/"
                        f"{40 + index + int(second_page)}"
                    ),
                    "state": "OPEN",
                    "isDraft": True,
                    "repository": {
                        "id": "R_repository",
                        "nameWithOwner": "owner/repository",
                    },
                }
                for index in range(count)
            ]
            return web.json_response(
                {
                    "data": {
                        "repository": {
                            "issue": {
                                "closedByPullRequestsReferences": {
                                    "nodes": nodes,
                                    "pageInfo": {
                                        "hasNextPage": paginated and not second_page,
                                        "endCursor": (
                                            "page-2"
                                            if paginated and not second_page
                                            else None
                                        ),
                                    },
                                }
                            }
                        }
                    }
                }
            )
        if "IssueComments" in query:
            second_page = variables.get("cursor") == "comment-page-2"
            body = (
                "@agent please review"
                if second_page
                else "ordinary Human discussion"
            )
            node = {
                "id": "IC_second" if second_page else "IC_first",
                "url": (
                    "https://github.example/owner/repository/issues/17#"
                    + ("issuecomment-2" if second_page else "issuecomment-1")
                ),
                "body": body,
                "updatedAt": "2026-08-10T12:00:00Z",
                "lastEditedAt": (
                    "2026-08-10T12:01:00Z" if second_page else None
                ),
                "author": {"login": "human"},
                "authorAssociation": "MEMBER",
                "isMinimized": second_page,
                "minimizedReason": "OUTDATED" if second_page else None,
            }
            return web.json_response(
                {
                    "data": {
                        "repository": {
                            "issue": {
                                "comments": {
                                    "nodes": [node],
                                    "pageInfo": {
                                        "hasNextPage": not second_page,
                                        "endCursor": (
                                            "comment-page-2"
                                            if not second_page
                                            else None
                                        ),
                                    },
                                }
                            }
                        }
                    }
                }
            )
        if "PullRequestCanonicalState" in query:
            return web.json_response(
                {
                    "data": {
                        "repository": {
                            "id": "R_repository",
                            "nameWithOwner": "owner/repository",
                            "pullRequest": {
                                "id": "PR_0",
                                "number": variables["number"],
                                "url": "https://github.example/owner/repository/pull/40",
                                "title": "Candidate implementation",
                                "body": "private PR body @agent",
                                "state": "OPEN",
                                "isDraft": True,
                                "headRefOid": "a" * 40,
                                "updatedAt": "2026-08-10T12:10:00Z",
                                "author": {"id": "U_author", "login": "author"},
                                "authorAssociation": "MEMBER",
                            },
                        }
                    }
                }
            )
        if "PullRequestConversationComments" in query:
            second_page = variables.get("cursor") == "pr-comment-page-2"
            node = self._graphql_comment(
                "IC_pr_second" if second_page else "IC_pr_first",
                "second private conversation" if second_page else "first private conversation",
                minimized=second_page,
            )
            return web.json_response(
                self._pull_request_connection_response(
                    variables,
                    "comments",
                    [node],
                    next_cursor=None if second_page else "pr-comment-page-2",
                )
            )
        if "PullRequestReviews" in query:
            review = {
                **self._graphql_comment(
                    "PRR_review", "private review body @agent"
                ),
                "state": (
                    "FUTURE_STATE"
                    if self.unknown_review_state
                    else "CHANGES_REQUESTED"
                ),
            }
            return web.json_response(
                self._pull_request_connection_response(
                    variables, "reviews", [review]
                )
            )
        if "PullRequestReviewThreads" in query:
            second_page = variables.get("cursor") == "thread-page-2"
            thread_number = 2 if second_page else 1
            thread = {
                "id": f"PRRT_{thread_number}",
                "isResolved": second_page,
                "comments": {
                    "nodes": [
                        self._graphql_comment(
                            f"PRRC_{thread_number}",
                            f"private diff comment {thread_number}",
                        )
                    ],
                    "pageInfo": {
                        "hasNextPage": self.nested_review_comment_page,
                        "endCursor": (
                            "nested-page-2"
                            if self.nested_review_comment_page
                            else None
                        ),
                    },
                },
            }
            return web.json_response(
                self._pull_request_connection_response(
                    variables,
                    "reviewThreads",
                    [thread],
                    next_cursor=None if second_page else "thread-page-2",
                )
            )
        return web.json_response({"errors": [{"message": "unknown query"}]})

    @staticmethod
    def _graphql_comment(
        node_id: str, body: str, *, minimized: bool = False
    ) -> dict[str, object]:
        return {
            "id": node_id,
            "url": f"https://github.example/comment/{node_id}",
            "body": body,
            "updatedAt": "2026-08-10T12:11:00Z",
            "lastEditedAt": None,
            "author": {"id": "U_reviewer", "login": "reviewer"},
            "authorAssociation": "MEMBER",
            "isMinimized": minimized,
            "minimizedReason": "OUTDATED" if minimized else None,
        }

    @staticmethod
    def _pull_request_connection_response(
        variables: dict[str, object],
        connection_name: str,
        nodes: list[dict[str, object]],
        *,
        next_cursor: str | None = None,
    ) -> dict[str, object]:
        return {
            "data": {
                "repository": {
                    "id": "R_repository",
                    "nameWithOwner": "owner/repository",
                    "pullRequest": {
                        "id": "PR_0",
                        "number": variables["number"],
                        "url": "https://github.example/owner/repository/pull/40",
                        connection_name: {
                            "nodes": nodes,
                            "pageInfo": {
                                "hasNextPage": next_cursor is not None,
                                "endCursor": next_cursor,
                            },
                        },
                    },
                }
            }
        }

    async def _permission(self, request: web.Request) -> web.Response:
        self._assert_installation_authorization(request)
        actor = request.match_info["actor"]
        if actor == "denied":
            return web.json_response(
                {"message": "private response body must not escape"}, status=403
            )
        if actor == "maintainer":
            return web.json_response(
                {"permission": "write", "role_name": "maintain"}
            )
        return web.json_response({"permission": "read", "role_name": "read"})

    async def _create_comment(self, request: web.Request) -> web.Response:
        self._assert_installation_authorization(request)
        body = (await request.json())["body"]
        self.comment_bodies.append(body)
        return web.json_response(self._comment_response(51, body), status=201)

    async def _update_comment(self, request: web.Request) -> web.Response:
        self._assert_installation_authorization(request)
        body = (await request.json())["body"]
        self.comment_bodies.append(body)
        return web.json_response(
            self._comment_response(int(request.match_info["comment_id"]), body)
        )

    async def _get_comment(self, request: web.Request) -> web.Response:
        self._assert_installation_authorization(request)
        comment_id = int(request.match_info["comment_id"])
        return web.json_response(
            self._comment_response(comment_id, "owned marker-51 body")
        )

    async def _list_comments(self, request: web.Request) -> web.Response:
        self._assert_installation_authorization(request)
        return web.json_response(
            [
                self._comment_response(50, "unrelated"),
                self._comment_response(51, "owned marker-51 body"),
            ]
        )

    async def _get_webhook_config(self, request: web.Request) -> web.Response:
        self._assert_app_authorization(request)
        return web.json_response(
            {
                "url": "https://old.example/webhooks/github",
                "content_type": "json",
                "insecure_ssl": "0",
            }
        )

    async def _update_webhook_config(self, request: web.Request) -> web.Response:
        self._assert_app_authorization(request)
        payload = await request.json()
        self.assertEqual(payload["secret"], "test-webhook-secret")
        return web.json_response(
            {
                "url": payload["url"],
                "content_type": payload["content_type"],
                "insecure_ssl": payload["insecure_ssl"],
            }
        )

    @staticmethod
    def _comment_response(comment_id: int, body: str) -> dict[str, object]:
        return {
            "id": comment_id,
            "node_id": f"IC_{comment_id}",
            "html_url": f"https://github.example/comment/{comment_id}",
            "user": {"login": "wrapper-app[bot]"},
            "created_at": "2026-08-10T12:00:00Z",
            "updated_at": "2026-08-10T12:00:00Z",
            "body": body,
        }

    async def test_app_jwt_installation_token_and_issue_reference(self) -> None:
        issue = await self.client.issue_reference("owner/repository", 17)
        permission = await self.client.repository_permission(
            "owner/repository", "maintainer"
        )

        self.assertEqual(issue.issue_node_id, "I_issue")
        self.assertEqual(issue.repository_node_id, "R_repository")
        self.assertEqual(issue.default_branch, "main")
        self.assertEqual(permission.scheduling_role, "maintain")
        self.assertEqual(self.installation_lookups, 1)
        self.assertEqual(self.token_creations, 1)

    async def test_native_association_is_exact_and_fails_on_multiple_open_prs(self) -> None:
        reference = await self.client.current_associated_pull_request(
            "owner/repository", 17
        )
        assert reference is not None
        self.assertEqual(reference.pr_node_id, "PR_0")
        self.assertTrue(reference.is_draft)

        with self.assertRaises(AssociationConflict):
            await self.client.current_associated_pull_request(
                "owner/repository", 99
            )
        with self.assertRaises(AssociationConflict):
            await self.client.current_associated_pull_request(
                "owner/repository", 100
            )

    async def test_comment_reconciliation_paginates_and_drops_raw_body(self) -> None:
        comments = await self.client.issue_comments("owner/repository", 17)

        self.assertEqual([value.object_node_id for value in comments], ["IC_first", "IC_second"])
        self.assertFalse(comments[0].mention_detected)
        self.assertFalse(comments[1].mention_detected)
        self.assertTrue(comments[1].is_minimized)
        self.assertEqual(comments[1].object_version, "2026-08-10T12:01:00Z")
        self.assertNotIn("@agent please review", repr(comments))

    async def test_pull_request_canonical_state_paginates_all_top_level_connections(self) -> None:
        reference = await self.client.current_associated_pull_request(
            "owner/repository", 17
        )
        assert reference is not None

        state = await self.client.pull_request_state(
            reference, self_logins=frozenset({"wrapper-bot"})
        )

        self.assertEqual(state.pull_request.head_ref_oid, "a" * 40)
        self.assertTrue(state.pull_request.mention_detected)
        self.assertEqual(
            [value.object_node_id for value in state.conversation_comments],
            ["IC_pr_first", "IC_pr_second"],
        )
        self.assertTrue(state.conversation_comments[1].is_minimized)
        self.assertEqual(state.reviews[0].state, "CHANGES_REQUESTED")
        self.assertTrue(state.reviews[0].mention_detected)
        self.assertEqual(
            [value.object_node_id for value in state.review_comments],
            ["PRRC_1", "PRRC_2"],
        )
        self.assertEqual(
            [value.is_resolved for value in state.review_threads],
            [False, True],
        )
        projection = repr(state)
        for private_text in (
            "private PR body",
            "first private conversation",
            "private review body",
            "private diff comment",
        ):
            self.assertNotIn(private_text, projection)
        self.assertIn("sha256:", projection)

    async def test_pull_request_unknown_state_and_nested_pagination_are_unavailable(self) -> None:
        reference = await self.client.current_associated_pull_request(
            "owner/repository", 17
        )
        assert reference is not None

        self.unknown_review_state = True
        with self.assertRaisesRegex(CanonicalStateUnavailable, "unknown"):
            await self.client.pull_request_state(reference)

        self.unknown_review_state = False
        self.nested_review_comment_page = True
        with self.assertRaisesRegex(CanonicalStateUnavailable, "nested"):
            await self.client.pull_request_state(reference)

    async def test_comment_projection_returns_digest_not_body(self) -> None:
        private_body = "agent final that should only appear in the API request"
        created = await self.client.create_issue_comment(
            "owner/repository", 17, private_body
        )
        updated = await self.client.update_issue_comment(
            "owner/repository", created.database_id, "updated final"
        )

        self.assertEqual(created.database_id, 51)
        self.assertTrue(created.body_digest.startswith("sha256:"))
        self.assertNotIn(private_body, repr(created))
        self.assertEqual(updated.database_id, 51)
        self.assertEqual(self.comment_bodies, [private_body, "updated final"])
        read_back = await self.client.get_issue_comment("owner/repository", 51)
        self.assertEqual(read_back.database_id, 51)
        found = await self.client.find_issue_comments_by_evidence(
            "owner/repository",
            17,
            author_login="wrapper-app",
            body_digest=read_back.body_digest,
            created_after=1_786_363_100.0,
            created_before=1_786_363_300.0,
        )
        self.assertEqual([comment.database_id for comment in found], [51])

    async def test_http_error_does_not_echo_remote_body_or_credentials(self) -> None:
        with self.assertRaises(GitHubApiError) as raised:
            await self.client.repository_permission("owner/repository", "denied")
        message = str(raised.exception)
        self.assertNotIn("private response body", message)
        self.assertNotIn("installation-token", message)

    async def test_app_webhook_configuration_is_explicit_and_secret_free(self) -> None:
        previous = await self.client.webhook_configuration()
        self.assertEqual(previous.url, "https://old.example/webhooks/github")
        updated = await self.client.update_webhook_configuration(
            url="https://random.trycloudflare.com/webhooks/github",
            webhook_secret=b"test-webhook-secret",
        )
        self.assertEqual(
            updated.url,
            "https://random.trycloudflare.com/webhooks/github",
        )
        self.assertNotIn("test-webhook-secret", repr(updated))


if __name__ == "__main__":
    unittest.main()
