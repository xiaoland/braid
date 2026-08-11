"""Loopback-only GitHub webhook ingress.

The ingress authenticates and durably records transport facts before it
acknowledges a supported, bound delivery.  It deliberately does not call the
Agent provider or interpret GitHub prose.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping, Set
from dataclasses import dataclass
import time

from aiohttp import web

from braid.github_webhook import (
    HeaderError,
    PayloadError,
    SignatureError,
    UnsupportedEvent,
    UnsupportedSurface,
    WebhookPing,
    parse_verified_webhook,
)
from braid.store import (
    BindingLifecycle,
    LeaseToken,
    StaleLease,
    StateConflict,
    StoreError,
    TransportStore,
)


DEFAULT_MAX_BODY_BYTES = 2 * 1024 * 1024


@dataclass(frozen=True, slots=True)
class IngressDependencies:
    store: TransportStore
    owner_token: LeaseToken
    webhook_secret: bytes
    self_logins: Set[str] = frozenset()
    clock: Callable[[], float] = time.time
    quiet_window_seconds: float = 30.0
    health_snapshot: Callable[[], Mapping[str, object]] | None = None


def create_ingress_app(
    dependencies: IngressDependencies,
    *,
    max_body_bytes: int = DEFAULT_MAX_BODY_BYTES,
    expose_health: bool = True,
) -> web.Application:
    if max_body_bytes < 1:
        raise ValueError("max_body_bytes must be positive")
    if not dependencies.webhook_secret:
        raise ValueError("webhook_secret must not be empty")
    if dependencies.quiet_window_seconds <= 0:
        raise ValueError("quiet_window_seconds must be positive")

    application = web.Application(client_max_size=max_body_bytes)
    application[INGRESS_DEPENDENCIES_KEY] = dependencies
    application.router.add_post("/webhooks/github", _receive_github_webhook)
    if expose_health:
        application.router.add_get("/healthz", _health)
    return application


def create_health_app(
    health_snapshot: Callable[[], Mapping[str, object]],
) -> web.Application:
    async def health(_request: web.Request) -> web.Response:
        return web.json_response(dict(health_snapshot()))

    application = web.Application()
    application.router.add_get("/healthz", health)
    return application


async def _health(request: web.Request) -> web.Response:
    dependencies = request.app.get(INGRESS_DEPENDENCIES_KEY)
    if not isinstance(dependencies, IngressDependencies):
        return _response("misconfigured", status=503)
    snapshot = (
        {"status": "ok"}
        if dependencies.health_snapshot is None
        else dict(dependencies.health_snapshot())
    )
    return web.json_response(snapshot)


async def _receive_github_webhook(request: web.Request) -> web.Response:
    dependencies = request.app.get(INGRESS_DEPENDENCIES_KEY)
    if not isinstance(dependencies, IngressDependencies):
        return _response("misconfigured", status=503)
    raw_body = await request.read()
    try:
        normalized = parse_verified_webhook(
            raw_body,
            request.headers,
            webhook_secret=dependencies.webhook_secret,
            self_logins=dependencies.self_logins,
        )
    except SignatureError:
        return _response("rejected", status=401)
    except (HeaderError, PayloadError):
        return _response("rejected", status=400)
    except (UnsupportedEvent, UnsupportedSurface):
        # A verified but out-of-scope delivery is terminally ignored. Returning
        # a 2xx avoids turning the App's broader event subscription into a
        # permanent failed-delivery queue.
        return _response("ignored", status=202)

    if isinstance(normalized, WebhookPing):
        return _response("verified", status=202)

    binding = await dependencies.store.binding_for_surface(
        normalized.surface_node_id
    )
    if binding is None or binding.lifecycle == BindingLifecycle.REVOKED:
        return _response("ignored", status=202)

    try:
        stored, created = await dependencies.store.ingest_event(
            dependencies.owner_token,
            normalized.to_event_envelope(
                binding_id=binding.binding_id,
                observed_at=dependencies.clock(),
            ),
        )
        await dependencies.store.schedule_event(
            dependencies.owner_token,
            stored.event_id,
            quiet_window_seconds=dependencies.quiet_window_seconds,
            received_at=stored.observed_at,
        )
    except StateConflict:
        return _response("conflict", status=409)
    except StaleLease:
        return _response("lease-stale", status=503)
    except StoreError:
        return _response("unavailable", status=503)
    return _response("accepted" if created else "duplicate", status=202)


def _response(outcome: str, *, status: int) -> web.Response:
    return web.json_response({"outcome": outcome}, status=status)


INGRESS_DEPENDENCIES_KEY = web.AppKey(
    "ingress_dependencies", IngressDependencies
)
