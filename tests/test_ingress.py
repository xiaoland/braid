from __future__ import annotations

import hashlib
import hmac
import json
from pathlib import Path
import tempfile
import unittest

from aiohttp.test_utils import TestClient, TestServer

from github_agent_bridge.ingress import (
    INGRESS_DEPENDENCIES_KEY,
    IngressDependencies,
    create_ingress_app,
)
from github_agent_bridge.store import (
    Binding,
    BindingLifecycle,
    LeaseToken,
    SurfaceKind,
    SurfaceRoute,
    TransportStore,
)


SECRET = b"ingress-test-secret"


def binding(*, lifecycle: BindingLifecycle = BindingLifecycle.ACTIVE) -> Binding:
    return Binding(
        binding_id="binding-1",
        repository_node_id="R_repository",
        repository_full_name="owner/repository",
        issue_node_id="I_issue",
        issue_number=17,
        issue_url="https://github.example/owner/repository/issues/17",
        thread_address="opaque-provider-thread-1",
        agent_identity="agent-bot",
        wrapper_identity="wrapper-bot",
        trusted_permission="triage",
        instruction_digest="sha256:instructions",
        lifecycle=lifecycle,
    )


def comment_payload(*, body: str = "Human message", issue_node_id: str = "I_issue") -> dict[str, object]:
    return {
        "action": "created",
        "repository": {
            "node_id": "R_repository",
            "full_name": "owner/repository",
        },
        "issue": {
            "node_id": issue_node_id,
            "number": 17,
            "html_url": "https://github.example/owner/repository/issues/17",
        },
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
        "sender": {"node_id": "U_actor", "login": "human"},
    }


def delivery(
    payload: dict[str, object],
    *,
    delivery_id: str = "delivery-guid-1",
    event_name: str = "issue_comment",
) -> tuple[bytes, dict[str, str]]:
    raw_body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    digest = hmac.new(SECRET, raw_body, hashlib.sha256).hexdigest()
    return raw_body, {
        "X-GitHub-Delivery": delivery_id,
        "X-GitHub-Event": event_name,
        "X-Hub-Signature-256": f"sha256={digest}",
        "Content-Type": "application/json",
    }


class IngressTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        database = Path(self.temporary_directory.name) / "state.sqlite3"
        self.store = await TransportStore.open(database)
        self.owner = await self.store.acquire_owner("test-owner", 60)
        await self.store.put_binding(self.owner, binding())
        self.observed_at = 2_000.0
        self.client = await self._start_client(self.owner)

    async def asyncTearDown(self) -> None:
        await self.client.close()
        await self.store.close()
        self.temporary_directory.cleanup()

    async def _start_client(
        self,
        owner: LeaseToken,
        *,
        max_body_bytes: int = 2 * 1024 * 1024,
    ) -> TestClient:
        app = create_ingress_app(
            IngressDependencies(
                store=self.store,
                owner_token=owner,
                webhook_secret=SECRET,
                self_logins=frozenset({"agent-bot", "wrapper-bot"}),
                clock=lambda: self.observed_at,
            ),
            max_body_bytes=max_body_bytes,
        )
        client = TestClient(TestServer(app))
        await client.start_server()
        return client

    async def post(
        self, raw_body: bytes, headers: dict[str, str]
    ):
        return await self.client.post(
            "/webhooks/github", data=raw_body, headers=headers
        )

    async def test_acknowledges_only_after_bound_event_is_durable(self) -> None:
        raw_body, headers = delivery(comment_payload(body="please @agent"))
        response = await self.post(raw_body, headers)

        self.assertEqual(response.status, 202)
        self.assertEqual(await response.json(), {"outcome": "accepted"})
        pending = await self.store.pending_events(self.owner, "binding-1")
        self.assertEqual(len(pending), 1)
        self.assertEqual(pending[0].delivery_id, "delivery-guid-1")
        self.assertEqual(pending[0].body_digest.startswith("sha256:"), True)
        self.assertTrue(pending[0].mention_detected)
        self.assertFalse(pending[0].urgent)
        scheduler = await self.store.scheduler_snapshot("binding-1")
        self.assertEqual(scheduler.quiet_deadline, 2_030.0)
        self.assertEqual(scheduler.transport_status, "pending")

    async def test_redelivery_is_acknowledged_without_duplicate_event(self) -> None:
        raw_body, headers = delivery(comment_payload())
        first = await self.post(raw_body, headers)
        self.observed_at += 30
        second = await self.post(raw_body, headers)

        self.assertEqual(first.status, 202)
        self.assertEqual(second.status, 202)
        self.assertEqual(await second.json(), {"outcome": "duplicate"})
        pending = await self.store.pending_events(self.owner, "binding-1")
        self.assertEqual(len(pending), 1)

    async def test_reused_delivery_with_different_facts_is_a_conflict(self) -> None:
        first_body, first_headers = delivery(comment_payload(body="first"))
        second_body, second_headers = delivery(comment_payload(body="second"))
        self.assertEqual((await self.post(first_body, first_headers)).status, 202)

        conflict = await self.post(second_body, second_headers)
        self.assertEqual(conflict.status, 409)
        self.assertEqual(await conflict.json(), {"outcome": "conflict"})

    async def test_invalid_signature_and_payload_are_not_enqueued(self) -> None:
        raw_body, headers = delivery(comment_payload())
        headers["X-Hub-Signature-256"] = "sha256=" + "0" * 64
        response = await self.post(raw_body, headers)
        self.assertEqual(response.status, 401)

        invalid_body = b"not-json"
        digest = hmac.new(SECRET, invalid_body, hashlib.sha256).hexdigest()
        headers["X-Hub-Signature-256"] = f"sha256={digest}"
        invalid = await self.post(invalid_body, headers)
        self.assertEqual(invalid.status, 400)
        self.assertEqual(
            await self.store.pending_events(self.owner, "binding-1"), ()
        )

    async def test_verified_unbound_and_out_of_scope_deliveries_are_terminally_ignored(self) -> None:
        unbound_body, unbound_headers = delivery(
            comment_payload(issue_node_id="I_unbound")
        )
        unbound = await self.post(unbound_body, unbound_headers)
        self.assertEqual(unbound.status, 202)
        self.assertEqual(await unbound.json(), {"outcome": "ignored"})

        event_body, event_headers = delivery(
            comment_payload(),
            delivery_id="delivery-guid-2",
            event_name="pull_request",
        )
        unsupported = await self.post(event_body, event_headers)
        self.assertEqual(unsupported.status, 202)
        self.assertEqual(await unsupported.json(), {"outcome": "ignored"})
        self.assertEqual(
            await self.store.pending_events(self.owner, "binding-1"), ()
        )

    async def test_native_associated_pr_conversation_routes_to_the_issue_binding(self) -> None:
        payload = comment_payload(issue_node_id="PR_candidate")
        issue = payload["issue"]
        assert isinstance(issue, dict)
        issue["pull_request"] = {
            "url": "https://api.github.example/owner/repository/pulls/23"
        }
        issue["number"] = 23
        issue["html_url"] = "https://github.example/owner/repository/pull/23"
        raw_body, headers = delivery(payload)

        unassociated = await self.post(raw_body, headers)
        self.assertEqual(await unassociated.json(), {"outcome": "ignored"})

        await self.store.replace_current_pr_route(
            self.owner,
            "binding-1",
            SurfaceRoute(
                binding_id="binding-1",
                surface_kind=SurfaceKind.PULL_REQUEST,
                repository_node_id="R_repository",
                repository_full_name="owner/repository",
                surface_node_id="PR_candidate",
                surface_number=23,
                canonical_url="https://github.example/owner/repository/pull/23",
                association_version="sha256:association",
            ),
        )
        raw_body, headers = delivery(payload, delivery_id="delivery-guid-2")
        associated = await self.post(raw_body, headers)
        self.assertEqual(await associated.json(), {"outcome": "accepted"})
        pending = await self.store.pending_events(self.owner, "binding-1")
        self.assertEqual(len(pending), 1)
        self.assertEqual(pending[0].surface_kind, "pull_request")
        self.assertEqual(pending[0].surface_node_id, "PR_candidate")

    async def test_revoked_binding_does_not_accept_new_events(self) -> None:
        await self.store.put_binding(
            self.owner, binding(lifecycle=BindingLifecycle.REVOKED)
        )
        raw_body, headers = delivery(comment_payload())
        response = await self.post(raw_body, headers)
        self.assertEqual(response.status, 202)
        self.assertEqual(await response.json(), {"outcome": "ignored"})
        self.assertEqual(
            await self.store.pending_events(self.owner, "binding-1"), ()
        )

    async def test_authenticated_ping_does_not_create_domain_event(self) -> None:
        raw_body, headers = delivery(
            {"hook_id": 42, "zen": "Keep it logically awesome."},
            event_name="ping",
        )
        response = await self.post(raw_body, headers)
        self.assertEqual(response.status, 202)
        self.assertEqual(await response.json(), {"outcome": "verified"})
        self.assertEqual(
            await self.store.pending_events(self.owner, "binding-1"), ()
        )

    async def test_self_origin_is_recorded_without_waking_the_binding(self) -> None:
        raw_body, headers = delivery(
            {
                **comment_payload(body="wrapper projection @agent"),
                "sender": {"node_id": "U_wrapper", "login": "wrapper-bot"},
            }
        )
        response = await self.post(raw_body, headers)
        self.assertEqual(response.status, 202)
        self.assertEqual(await response.json(), {"outcome": "accepted"})
        self.assertEqual(
            await self.store.pending_events(self.owner, "binding-1"), ()
        )
        latest = await self.store.latest_event_for_object(
            self.owner, "binding-1", "IC_comment"
        )
        assert latest is not None
        self.assertFalse(latest.wake_eligible)
        self.assertEqual(latest.state.value, "superseded")

    async def test_body_limit_is_enforced_before_normalization(self) -> None:
        await self.client.close()
        self.client = await self._start_client(self.owner, max_body_bytes=32)
        raw_body, headers = delivery(comment_payload())
        response = await self.post(raw_body, headers)
        self.assertEqual(response.status, 413)
        self.assertEqual(
            await self.store.pending_events(self.owner, "binding-1"), ()
        )

    async def test_missing_runtime_dependencies_have_a_stable_error_surface(self) -> None:
        await self.client.close()
        app = create_ingress_app(
            IngressDependencies(
                store=self.store,
                owner_token=self.owner,
                webhook_secret=SECRET,
            )
        )
        del app[INGRESS_DEPENDENCIES_KEY]
        self.client = TestClient(TestServer(app))
        await self.client.start_server()

        raw_body, headers = delivery(comment_payload())
        response = await self.post(raw_body, headers)
        self.assertEqual(response.status, 503)
        self.assertEqual(await response.json(), {"outcome": "misconfigured"})


if __name__ == "__main__":
    unittest.main()
