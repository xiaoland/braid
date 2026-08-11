from __future__ import annotations

from dataclasses import asdict
import hashlib
import hmac
import json
import unittest

from github_agent_bridge.github_webhook import (
    HeaderError,
    NormalizedGitHubEvent,
    ObjectLifecycle,
    PayloadError,
    SignatureError,
    UnsupportedEvent,
    WebhookPing,
    has_visible_agent_mention,
    parse_verified_webhook,
)


SECRET = b"test-only-webhook-secret"


def issue_comment_payload(
    *,
    action: str = "created",
    body: str | None = "Please take a look.",
    sender_login: str | None = "human",
    pull_request_surface: bool = False,
) -> dict[str, object]:
    issue: dict[str, object] = {
        "node_id": "I_issue",
        "number": 17,
        "html_url": "https://github.example/owner/repository/issues/17",
    }
    if pull_request_surface:
        issue["pull_request"] = {
            "url": "https://api.github.example/repos/owner/repository/pulls/9"
        }
    sender: dict[str, object] | None
    if sender_login is None:
        sender = None
    else:
        sender = {"node_id": "U_actor", "login": sender_login}
    return {
        "action": action,
        "repository": {
            "node_id": "R_repository",
            "full_name": "owner/repository",
        },
        "issue": issue,
        "comment": {
            "node_id": "IC_comment",
            "html_url": (
                "https://github.example/owner/repository/issues/17"
                "#issuecomment-42"
            ),
            "body": body,
            "created_at": "2026-08-10T12:00:00Z",
            "updated_at": "2026-08-10T12:01:00Z",
            "author_association": "MEMBER",
        },
        "sender": sender,
    }


def issue_payload(*, action: str = "opened", body: str | None = None) -> dict[str, object]:
    return {
        "action": action,
        "repository": {
            "node_id": "R_repository",
            "full_name": "owner/repository",
        },
        "issue": {
            "node_id": "I_issue",
            "number": 17,
            "html_url": "https://github.example/owner/repository/issues/17",
            "title": "A bounded task",
            "body": body,
            "state": "open" if action != "closed" else "closed",
            "updated_at": "2026-08-10T12:01:00Z",
            "author_association": "OWNER",
        },
        "sender": {"node_id": "U_actor", "login": "human"},
    }


def encoded_delivery(
    payload: dict[str, object],
    *,
    event_name: str = "issue_comment",
    delivery_id: str = "delivery-guid-1",
) -> tuple[bytes, dict[str, str]]:
    raw_body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    signature = hmac.new(SECRET, raw_body, hashlib.sha256).hexdigest()
    return raw_body, {
        "X-GitHub-Delivery": delivery_id,
        "X-GitHub-Event": event_name,
        "X-Hub-Signature-256": f"sha256={signature}",
    }


class GitHubWebhookTests(unittest.TestCase):
    def parse(
        self,
        payload: dict[str, object],
        *,
        event_name: str = "issue_comment",
        permission_role: str | None = None,
        self_logins: frozenset[str] = frozenset(),
    ) -> NormalizedGitHubEvent | WebhookPing:
        raw_body, headers = encoded_delivery(payload, event_name=event_name)
        return parse_verified_webhook(
            raw_body,
            headers,
            webhook_secret=SECRET,
            permission_role=permission_role,
            self_logins=self_logins,
        )

    def test_comment_normalization_keeps_references_and_digests_only(self) -> None:
        secret_phrase = "internal content that must not be persisted verbatim"
        result = self.parse(issue_comment_payload(body=secret_phrase))

        self.assertIsInstance(result, NormalizedGitHubEvent)
        assert isinstance(result, NormalizedGitHubEvent)
        self.assertEqual(result.event_key, "github-delivery:delivery-guid-1")
        self.assertEqual(result.event_name, "issue_comment")
        self.assertEqual(result.issue_node_id, "I_issue")
        self.assertEqual(result.object_node_id, "IC_comment")
        self.assertEqual(result.object_version, "2026-08-10T12:01:00Z")
        self.assertEqual(result.lifecycle, ObjectLifecycle.ACTIVE)
        self.assertEqual(result.actor_login, "human")
        self.assertEqual(result.author_association, "MEMBER")
        self.assertTrue(result.body_digest.startswith("sha256:"))
        self.assertNotIn(secret_phrase, repr(result))
        self.assertNotIn(secret_phrase, json.dumps(asdict(result)))

        envelope = result.to_event_envelope(
            binding_id="binding-1", observed_at=1_234.0
        )
        self.assertEqual(envelope.binding_id, "binding-1")
        self.assertEqual(envelope.surface_kind, "issue")
        self.assertEqual(envelope.surface_node_id, "I_issue")
        self.assertEqual(envelope.delivery_id, "delivery-guid-1")

    def test_signature_is_verified_before_json_is_parsed(self) -> None:
        raw_body = b"not valid json"
        headers = {
            "X-GitHub-Delivery": "delivery-guid-1",
            "X-GitHub-Event": "issue_comment",
            "X-Hub-Signature-256": "sha256=" + "0" * 64,
        }
        with self.assertRaises(SignatureError):
            parse_verified_webhook(raw_body, headers, webhook_secret=SECRET)

        valid_signature = hmac.new(SECRET, raw_body, hashlib.sha256).hexdigest()
        headers["X-Hub-Signature-256"] = f"sha256={valid_signature}"
        with self.assertRaises(PayloadError):
            parse_verified_webhook(raw_body, headers, webhook_secret=SECRET)

    def test_missing_malformed_and_changed_signatures_are_rejected(self) -> None:
        raw_body, headers = encoded_delivery(issue_comment_payload())
        missing = dict(headers)
        del missing["X-Hub-Signature-256"]
        with self.assertRaises(HeaderError):
            parse_verified_webhook(raw_body, missing, webhook_secret=SECRET)

        malformed = dict(headers)
        malformed["X-Hub-Signature-256"] = "sha1=wrong"
        with self.assertRaises(SignatureError):
            parse_verified_webhook(raw_body, malformed, webhook_secret=SECRET)

        with self.assertRaises(SignatureError):
            parse_verified_webhook(raw_body + b" ", headers, webhook_secret=SECRET)

        with self.assertRaises(SignatureError):
            parse_verified_webhook(raw_body, headers, webhook_secret=b"")

    def test_header_names_are_case_insensitive(self) -> None:
        raw_body, headers = encoded_delivery(issue_comment_payload())
        lower_headers = {key.lower(): value for key, value in headers.items()}
        result = parse_verified_webhook(
            raw_body, lower_headers, webhook_secret=SECRET
        )
        self.assertIsInstance(result, NormalizedGitHubEvent)

    def test_issue_lifecycle_and_comment_tombstone_are_explicit(self) -> None:
        edited = self.parse(
            issue_payload(action="edited", body="updated"), event_name="issues"
        )
        closed = self.parse(issue_payload(action="closed"), event_name="issues")
        reopened = self.parse(issue_payload(action="reopened"), event_name="issues")
        deleted = self.parse(issue_comment_payload(action="deleted", body="@agent"))

        assert isinstance(edited, NormalizedGitHubEvent)
        assert isinstance(closed, NormalizedGitHubEvent)
        assert isinstance(reopened, NormalizedGitHubEvent)
        assert isinstance(deleted, NormalizedGitHubEvent)
        self.assertEqual(edited.lifecycle, ObjectLifecycle.EDITED)
        self.assertEqual(closed.lifecycle, ObjectLifecycle.CLOSED)
        self.assertEqual(reopened.lifecycle, ObjectLifecycle.REOPENED)
        self.assertEqual(deleted.lifecycle, ObjectLifecycle.DELETED)
        self.assertFalse(deleted.mention_detected)
        self.assertFalse(deleted.urgent)

    def test_ghost_actor_is_preserved_as_unknown(self) -> None:
        result = self.parse(issue_comment_payload(sender_login=None))
        assert isinstance(result, NormalizedGitHubEvent)
        self.assertIsNone(result.actor_node_id)
        self.assertIsNone(result.actor_login)

    def test_unsupported_events_actions_and_pr_surface_fail_closed(self) -> None:
        with self.assertRaises(UnsupportedEvent):
            self.parse(issue_comment_payload(), event_name="check_run")
        with self.assertRaises(UnsupportedEvent):
            self.parse(issue_comment_payload(action="minimized"))

        pr_comment = self.parse(
            issue_comment_payload(pull_request_surface=True)
        )
        assert isinstance(pr_comment, NormalizedGitHubEvent)
        self.assertEqual(pr_comment.surface_kind, "pull_request")
        self.assertEqual(pr_comment.surface_node_id, "I_issue")

        malformed = issue_comment_payload()
        repository = malformed["repository"]
        assert isinstance(repository, dict)
        del repository["node_id"]
        with self.assertRaises(PayloadError):
            self.parse(malformed)

    def test_pr_review_and_thread_events_are_transport_refs(self) -> None:
        repository = {
            "node_id": "R_repository",
            "full_name": "owner/repository",
        }
        pull_request = {
            "node_id": "PR_candidate",
            "number": 23,
            "html_url": "https://github.example/owner/repository/pull/23",
            "title": "Candidate",
            "body": "Implementation notes",
            "state": "open",
            "draft": True,
            "updated_at": "2026-08-10T12:00:00Z",
            "author_association": "MEMBER",
        }
        sender = {"node_id": "U_actor", "login": "human"}

        pr_result = self.parse(
            {
                "action": "synchronize",
                "repository": repository,
                "pull_request": pull_request,
                "sender": sender,
            },
            event_name="pull_request",
        )
        review_result = self.parse(
            {
                "action": "submitted",
                "repository": repository,
                "pull_request": pull_request,
                "review": {
                    "node_id": "PRR_review",
                    "html_url": "https://github.example/review/1",
                    "body": "Please address this @agent",
                    "submitted_at": "2026-08-10T12:01:00Z",
                    "author_association": "MEMBER",
                },
                "sender": sender,
            },
            event_name="pull_request_review",
            permission_role="write",
        )
        comment_result = self.parse(
            {
                "action": "edited",
                "repository": repository,
                "pull_request": pull_request,
                "comment": {
                    "node_id": "PRRC_comment",
                    "html_url": "https://github.example/review-comment/1",
                    "body": "Updated review comment",
                    "updated_at": "2026-08-10T12:02:00Z",
                    "author_association": "MEMBER",
                },
                "sender": sender,
            },
            event_name="pull_request_review_comment",
        )
        thread_result = self.parse(
            {
                "action": "resolved",
                "repository": repository,
                "pull_request": pull_request,
                "thread": {
                    "node_id": "PRRT_thread",
                    "updated_at": "2026-08-10T12:03:00Z",
                    "comments": [{"node_id": "PRRC_comment"}],
                },
                "sender": sender,
            },
            event_name="pull_request_review_thread",
        )

        for result in (pr_result, review_result, comment_result, thread_result):
            assert isinstance(result, NormalizedGitHubEvent)
            self.assertEqual(result.surface_kind, "pull_request")
            self.assertEqual(result.surface_node_id, "PR_candidate")
        assert isinstance(review_result, NormalizedGitHubEvent)
        assert isinstance(thread_result, NormalizedGitHubEvent)
        self.assertTrue(review_result.urgent)
        self.assertEqual(thread_result.lifecycle, ObjectLifecycle.RESOLVED)

    def test_mention_only_becomes_urgent_for_trusted_non_self_actor(self) -> None:
        ordinary = self.parse(issue_comment_payload(body="@agent"))
        read_only = self.parse(
            issue_comment_payload(body="@agent"), permission_role="read"
        )
        trusted = self.parse(
            issue_comment_payload(body="@agent"), permission_role="write"
        )
        self_origin = self.parse(
            issue_comment_payload(body="@agent", sender_login="wrapper-bot"),
            permission_role="admin",
            self_logins=frozenset({"WRAPPER-BOT"}),
        )

        assert isinstance(ordinary, NormalizedGitHubEvent)
        assert isinstance(read_only, NormalizedGitHubEvent)
        assert isinstance(trusted, NormalizedGitHubEvent)
        assert isinstance(self_origin, NormalizedGitHubEvent)
        self.assertTrue(ordinary.mention_detected)
        self.assertFalse(ordinary.urgent)
        self.assertTrue(read_only.mention_detected)
        self.assertFalse(read_only.urgent)
        self.assertTrue(trusted.mention_detected)
        self.assertTrue(trusted.urgent)
        self.assertFalse(self_origin.mention_detected)
        self.assertFalse(self_origin.urgent)
        self.assertFalse(self_origin.wake_eligible)

        graphql_spelling = self.parse(
            issue_comment_payload(body="@agent", sender_login="wrapper-bot"),
            permission_role="admin",
            self_logins=frozenset({"wrapper-bot[bot]"}),
        )
        assert isinstance(graphql_spelling, NormalizedGitHubEvent)
        self.assertFalse(graphql_spelling.mention_detected)
        self.assertFalse(graphql_spelling.wake_eligible)

    def test_visible_agent_mention_grammar(self) -> None:
        visible = (
            "please @agent now",
            "(@agent), continue",
            "[@agent](https://example.test/request)",
            "> @agent from a quoted Human comment",
        )
        hidden_or_non_exact = (
            "@Agent",
            "@agent-bot",
            "foo@agent.com",
            "`@agent`",
            "``use @agent here``",
            "```text\n@agent\n```",
            "~~~text\n@agent\n~~~",
            "> ```text\n> @agent\n> ```",
            "    @agent in indented code",
            "\t@agent in tab-indented code",
            ">     @agent in quoted indented code",
            "<!-- @agent -->",
            "prefix <!-- @agent without a closing marker",
            "[link](https://example.test/@agent)",
            "https://example.test/@agent",
            "<https://example.test/@agent>",
            '<a href="https://example.test/@agent">link</a>',
            "<code>@agent</code>",
            "<pre><code>@agent</code></pre>",
        )
        for body in visible:
            with self.subTest(body=body):
                self.assertTrue(has_visible_agent_mention(body))
        for body in hidden_or_non_exact:
            with self.subTest(body=body):
                self.assertFalse(has_visible_agent_mention(body))

    def test_ping_is_authenticated_and_does_not_retain_zen(self) -> None:
        zen = "Keep it logically awesome."
        result = self.parse(
            {"hook_id": 42, "zen": zen}, event_name="ping"
        )
        self.assertIsInstance(result, WebhookPing)
        assert isinstance(result, WebhookPing)
        self.assertEqual(result.delivery_id, "delivery-guid-1")
        self.assertEqual(result.hook_id, 42)
        self.assertTrue(result.zen_digest.startswith("sha256:"))
        self.assertNotIn(zen, repr(result))


if __name__ == "__main__":
    unittest.main()
