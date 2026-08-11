"""Black-box contract probe for the installed Codex app-server binary."""

from __future__ import annotations

import asyncio
from dataclasses import asdict, dataclass
import hashlib
import json
from pathlib import Path
import tempfile
from typing import Any

from github_agent_bridge.app_server import (
    AppServerClient,
    AppServerProtocolError,
    AppServerRemoteError,
    ServerMessage,
    provider_environment,
)


PROBE_CLIENT_NAME = "braid-probe"
PROBE_CLIENT_VERSION = "0.1.0"
WRAPPER_TRANSPORT_INPUT = (
    "Wrapper transport notification: canonical GitHub event references are in "
    "application context. This is not a Human message or command. Use gh with "
    "the supplied URLs and IDs to observe canonical GitHub state before deciding "
    "whether to discuss, wait, plan, or act."
)
REQUIRED_CLIENT_METHODS = frozenset(
    {
        "initialize",
        "thread/archive",
        "thread/resume",
        "thread/start",
        "turn/interrupt",
        "turn/start",
        "turn/steer",
    }
)
REQUIRED_SERVER_METHODS = frozenset(
    {
        "item/agentMessage/delta",
        "item/completed",
        "item/reasoning/summaryTextDelta",
        "item/reasoning/textDelta",
        "item/started",
        "turn/completed",
        "turn/started",
    }
)


@dataclass(frozen=True, slots=True)
class TurnObservation:
    turn_id: str
    status: str
    final_answers: tuple[str, ...]
    commentary_messages: tuple[str, ...]
    item_types: tuple[str, ...]
    notification_methods: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class ProtocolProbeReport:
    app_server_version: str
    stable_schema_sha256: str
    experimental_schema_sha256: str
    initialize_user_agent: str
    thread_id: str
    start_turn: TurnObservation
    completed_turn_steer_rejection_code: int
    steer_turn: TurnObservation
    interrupt_turn: TurnObservation
    failed_turn: TurnObservation
    disconnect_turn_id: str
    disconnect_terminal_received_before_close: bool
    disconnect_resume_status: str
    resume_turn: TurnObservation
    application_context_turn: TurnObservation
    resumed_thread_matches: bool
    persisted_turn_ids: tuple[str, ...]
    archived: bool

    def to_json(self) -> str:
        return json.dumps(asdict(self), ensure_ascii=False, sort_keys=True)


@dataclass(frozen=True, slots=True)
class ProtocolIdentity:
    app_server_version: str
    stable_schema_sha256: str
    experimental_schema_sha256: str


async def inspect_protocol_identity(
    *, codex_executable: Path, timeout_seconds: float = 30.0
) -> ProtocolIdentity:
    """Generate the installed binary's schemas without starting a model turn."""

    executable = codex_executable.resolve(strict=True)
    if timeout_seconds <= 0:
        raise ValueError("timeout_seconds must be positive")
    version = await _capture_stdout(executable, "--version", timeout=timeout_seconds)
    with tempfile.TemporaryDirectory(
        prefix="braid-schema-"
    ) as directory:
        schema_root = Path(directory)
        stable_schema_directory = schema_root / "stable"
        experimental_schema_directory = schema_root / "experimental"
        await _run_process(
            executable,
            "app-server",
            "generate-json-schema",
            "--out",
            stable_schema_directory,
            timeout=timeout_seconds,
        )
        await _run_process(
            executable,
            "app-server",
            "generate-json-schema",
            "--experimental",
            "--out",
            experimental_schema_directory,
            timeout=timeout_seconds,
        )
        stable_schema_digest = _verify_and_digest_schema(stable_schema_directory)
        experimental_schema_digest = _verify_and_digest_schema(
            experimental_schema_directory
        )
        _verify_application_context_contract(experimental_schema_directory)
    return ProtocolIdentity(
        app_server_version=version,
        stable_schema_sha256=stable_schema_digest,
        experimental_schema_sha256=experimental_schema_digest,
    )


async def run_protocol_probe(
    *,
    codex_executable: Path,
    workspace: Path,
    timeout_seconds: float = 120.0,
) -> ProtocolProbeReport:
    """Exercise schema, lifecycle, steering, interruption, and resume for real."""

    executable = codex_executable.resolve(strict=True)
    probe_workspace = workspace.resolve(strict=True)
    if not probe_workspace.is_dir():
        raise ValueError("probe workspace must be a directory")

    identity = await inspect_protocol_identity(
        codex_executable=executable, timeout_seconds=timeout_seconds
    )

    command = (str(executable), "app-server", "--stdio")
    environment = provider_environment()
    first_client = await AppServerClient.start(command, environment=environment)
    first_client_closed = False
    try:
        initialized = await first_client.initialize(
            client_name=PROBE_CLIENT_NAME,
            client_version=PROBE_CLIENT_VERSION,
            experimental_api=True,
            timeout=timeout_seconds,
        )
        user_agent = _required_string(initialized, "userAgent", "initialize")
        started = _required_object(
            await first_client.request(
                "thread/start",
                {
                    "approvalPolicy": "never",
                    "baseInstructions": (
                        "This is an automated protocol probe. Do not call tools. "
                        "Follow exact response-token instructions."
                    ),
                    "cwd": str(probe_workspace),
                    "ephemeral": False,
                    "sandbox": "read-only",
                },
                timeout=timeout_seconds,
            ),
            "thread/start",
        )
        thread = _required_object(started.get("thread"), "thread/start.thread")
        thread_id = _required_string(thread, "id", "thread/start.thread")

        start_turn_id = await _start_turn(
            first_client,
            thread_id,
            "Reply with exactly PROVIDER_PROBE_START.",
            timeout_seconds,
        )
        start_turn = await _wait_for_terminal(
            first_client, thread_id, start_turn_id, timeout_seconds
        )
        _require_latest_final(start_turn, "PROVIDER_PROBE_START")
        completed_turn_steer_rejection_code = await _require_steer_rejected(
            first_client,
            thread_id,
            start_turn_id,
            timeout_seconds,
        )

        steer_turn_id = await _start_turn(
            first_client,
            thread_id,
            (
                "Prepare a detailed draft, but your eventual final response must be "
                "exactly PROVIDER_PROBE_BEFORE_STEER. Do not call tools."
            ),
            timeout_seconds,
        )
        pre_steer = await _wait_for_turn_started(
            first_client, thread_id, steer_turn_id, timeout_seconds
        )
        steer_result = _required_object(
            await first_client.request(
                "turn/steer",
                {
                    "additionalContext": {
                        "wrapper-event:steer-probe": {
                            "kind": "application",
                            "value": (
                                "A Wrapper-origin protocol event was observed. For this "
                                "probe only, replace the final response with exactly "
                                "PROVIDER_PROBE_AFTER_STEER."
                            ),
                        }
                    },
                    "expectedTurnId": steer_turn_id,
                    "input": [{"type": "text", "text": WRAPPER_TRANSPORT_INPUT}],
                    "threadId": thread_id,
                },
                timeout=timeout_seconds,
            ),
            "turn/steer",
        )
        if steer_result.get("turnId") != steer_turn_id:
            raise AppServerProtocolError("turn/steer did not echo the active turn id")
        steer_turn = await _wait_for_terminal(
            first_client,
            thread_id,
            steer_turn_id,
            timeout_seconds,
            preceding=pre_steer,
        )
        _require_latest_final(steer_turn, "PROVIDER_PROBE_AFTER_STEER")

        interrupt_turn_id = await _start_turn(
            first_client,
            thread_id,
            (
                "Prepare a long private analysis before answering, do not call tools, "
                "and eventually reply PROVIDER_PROBE_NOT_EXPECTED."
            ),
            timeout_seconds,
        )
        pre_interrupt = await _wait_for_turn_started(
            first_client, thread_id, interrupt_turn_id, timeout_seconds
        )
        interrupt_result = await first_client.request(
            "turn/interrupt",
            {"threadId": thread_id, "turnId": interrupt_turn_id},
            timeout=timeout_seconds,
        )
        if interrupt_result != {}:
            raise AppServerProtocolError("turn/interrupt result is not an empty object")
        interrupt_turn = await _wait_for_terminal(
            first_client,
            thread_id,
            interrupt_turn_id,
            timeout_seconds,
            preceding=pre_interrupt,
        )
        if interrupt_turn.status != "interrupted":
            raise AppServerProtocolError(
                f"interrupted probe turn ended as {interrupt_turn.status!r}"
            )

        failed_turn = await _probe_failed_turn(
            first_client, probe_workspace, timeout_seconds
        )

        disconnect_turn_id = await _start_turn(
            first_client,
            thread_id,
            (
                "Prepare a response of at least 3000 words before a final answer. "
                "Do not call tools."
            ),
            timeout_seconds,
        )
        pre_disconnect = await _wait_for_turn_started(
            first_client, thread_id, disconnect_turn_id, timeout_seconds
        )
        disconnect_terminal_received_before_close = await _terminal_arrived_during(
            first_client,
            thread_id,
            disconnect_turn_id,
            preceding=pre_disconnect,
            observation_seconds=0.05,
        )
        if disconnect_terminal_received_before_close:
            raise AppServerProtocolError(
                "disconnect probe turn completed before the transport could close"
            )
        await first_client.close(timeout=0.01)
        first_client_closed = True
    finally:
        if not first_client_closed:
            await first_client.close()

    second_client = await AppServerClient.start(command, environment=environment)
    archived = False
    try:
        await second_client.initialize(
            client_name=PROBE_CLIENT_NAME,
            client_version=PROBE_CLIENT_VERSION,
            experimental_api=True,
            timeout=timeout_seconds,
        )
        resumed = _required_object(
            await second_client.request(
                "thread/resume",
                {
                    "approvalPolicy": "never",
                    "cwd": str(probe_workspace),
                    "sandbox": "read-only",
                    "threadId": thread_id,
                },
                timeout=timeout_seconds,
            ),
            "thread/resume",
        )
        resumed_thread = _required_object(resumed.get("thread"), "thread/resume.thread")
        resumed_thread_id = _required_string(
            resumed_thread, "id", "thread/resume.thread"
        )
        persisted_turn_ids = tuple(
            turn["id"]
            for turn in resumed_thread.get("turns", [])
            if isinstance(turn, dict) and isinstance(turn.get("id"), str)
        )
        if start_turn_id not in persisted_turn_ids or disconnect_turn_id not in persisted_turn_ids:
            raise AppServerProtocolError(
                "thread/resume omitted a materialized probe turn"
            )
        disconnect_resume_status = _persisted_turn_status(
            resumed_thread, disconnect_turn_id
        )
        if disconnect_resume_status != "interrupted":
            raise AppServerProtocolError(
                "transport disconnect did not persist an interrupted provider turn"
            )

        resume_turn_id = await _start_turn(
            second_client,
            thread_id,
            "Reply with exactly PROVIDER_PROBE_RESUMED.",
            timeout_seconds,
        )
        resume_turn = await _wait_for_terminal(
            second_client, thread_id, resume_turn_id, timeout_seconds
        )
        _require_latest_final(resume_turn, "PROVIDER_PROBE_RESUMED")

        application_context_turn_id = await _start_application_context_turn(
            second_client, thread_id, timeout_seconds
        )
        application_context_turn = await _wait_for_terminal(
            second_client,
            thread_id,
            application_context_turn_id,
            timeout_seconds,
        )
        _require_latest_final(
            application_context_turn, "PROVIDER_PROBE_APPLICATION_CONTEXT"
        )
        archive_result = await second_client.request(
            "thread/archive", {"threadId": thread_id}, timeout=timeout_seconds
        )
        if archive_result != {}:
            raise AppServerProtocolError("thread/archive result is not an empty object")
        archived = True
    finally:
        await second_client.close()

    return ProtocolProbeReport(
        app_server_version=identity.app_server_version,
        stable_schema_sha256=identity.stable_schema_sha256,
        experimental_schema_sha256=identity.experimental_schema_sha256,
        initialize_user_agent=user_agent,
        thread_id=thread_id,
        start_turn=start_turn,
        completed_turn_steer_rejection_code=completed_turn_steer_rejection_code,
        steer_turn=steer_turn,
        interrupt_turn=interrupt_turn,
        failed_turn=failed_turn,
        disconnect_turn_id=disconnect_turn_id,
        disconnect_terminal_received_before_close=(
            disconnect_terminal_received_before_close
        ),
        disconnect_resume_status=disconnect_resume_status,
        resume_turn=resume_turn,
        application_context_turn=application_context_turn,
        resumed_thread_matches=resumed_thread_id == thread_id,
        persisted_turn_ids=persisted_turn_ids,
        archived=archived,
    )


async def _require_steer_rejected(
    client: AppServerClient,
    thread_id: str,
    completed_turn_id: str,
    timeout: float,
) -> int:
    try:
        await client.request(
            "turn/steer",
            {
                "expectedTurnId": completed_turn_id,
                "input": [{"type": "text", "text": "must be rejected"}],
                "threadId": thread_id,
            },
            timeout=timeout,
        )
    except AppServerRemoteError as error:
        if error.code != -32600:
            raise AppServerProtocolError(
                f"completed-turn steer failed with unexpected code {error.code}"
            ) from error
        return error.code
    raise AppServerProtocolError("app-server accepted steer for a completed turn")


async def _probe_failed_turn(
    client: AppServerClient,
    workspace: Path,
    timeout: float,
) -> TurnObservation:
    started = _required_object(
        await client.request(
            "thread/start",
            {
                "allowProviderModelFallback": False,
                "approvalPolicy": "never",
                "baseInstructions": "Do not call tools.",
                "cwd": str(workspace),
                "ephemeral": True,
                "model": "braid-deliberately-invalid-model",
                "sandbox": "read-only",
            },
            timeout=timeout,
        ),
        "failed-thread/start",
    )
    thread = _required_object(started.get("thread"), "failed-thread/start.thread")
    thread_id = _required_string(thread, "id", "failed-thread/start.thread")
    turn_id = await _start_turn(
        client,
        thread_id,
        "This deliberately invalid-model probe must reach a failed terminal state.",
        timeout,
    )
    observation = await _wait_for_terminal(client, thread_id, turn_id, timeout)
    if observation.status != "failed":
        raise AppServerProtocolError(
            f"invalid-model probe turn ended as {observation.status!r}"
        )
    return observation


def _persisted_turn_status(thread: JsonObject, turn_id: str) -> str:
    turns = thread.get("turns")
    if not isinstance(turns, list):
        raise AppServerProtocolError("resumed thread turns is not an array")
    for turn in turns:
        if not isinstance(turn, dict) or turn.get("id") != turn_id:
            continue
        status = turn.get("status")
        if status not in {"completed", "failed", "inProgress", "interrupted"}:
            raise AppServerProtocolError("resumed turn has an unknown status")
        return status
    raise AppServerProtocolError("resumed thread omitted the disconnected turn")


async def _start_turn(
    client: AppServerClient,
    thread_id: str,
    text: str,
    timeout: float,
) -> str:
    result = _required_object(
        await client.request(
            "turn/start",
            {
                "approvalPolicy": "never",
                "input": [{"type": "text", "text": text}],
                "sandboxPolicy": {"networkAccess": False, "type": "readOnly"},
                "summary": "concise",
                "threadId": thread_id,
            },
            timeout=timeout,
        ),
        "turn/start",
    )
    turn = _required_object(result.get("turn"), "turn/start.turn")
    turn_id = _required_string(turn, "id", "turn/start.turn")
    if turn.get("status") != "inProgress":
        raise AppServerProtocolError("turn/start did not return an in-progress turn")
    return turn_id


async def _start_application_context_turn(
    client: AppServerClient,
    thread_id: str,
    timeout: float,
) -> str:
    result = _required_object(
        await client.request(
            "turn/start",
            {
                "additionalContext": {
                    "wrapper-event:application-context-probe": {
                        "kind": "application",
                        "value": (
                            "A Wrapper-origin protocol event was observed. For this "
                            "probe only, reply with exactly "
                            "PROVIDER_PROBE_APPLICATION_CONTEXT."
                        ),
                    }
                },
                "approvalPolicy": "never",
                "input": [{"type": "text", "text": WRAPPER_TRANSPORT_INPUT}],
                "sandboxPolicy": {"networkAccess": False, "type": "readOnly"},
                "summary": "concise",
                "threadId": thread_id,
            },
            timeout=timeout,
        ),
        "application-context turn/start",
    )
    turn = _required_object(result.get("turn"), "application-context turn/start.turn")
    turn_id = _required_string(turn, "id", "application-context turn/start.turn")
    if turn.get("status") != "inProgress":
        raise AppServerProtocolError(
            "application-context turn/start did not return an in-progress turn"
        )
    return turn_id


async def _wait_for_turn_started(
    client: AppServerClient,
    thread_id: str,
    turn_id: str,
    timeout: float,
) -> list[ServerMessage]:
    observed: list[ServerMessage] = []
    async with asyncio.timeout(timeout):
        while True:
            message = await client.next_message(timeout=timeout)
            observed.append(message)
            if (
                message.method == "turn/started"
                and message.params.get("threadId") == thread_id
                and _nested_turn_id(message.params) == turn_id
            ):
                return observed


async def _terminal_arrived_during(
    client: AppServerClient,
    thread_id: str,
    turn_id: str,
    *,
    preceding: list[ServerMessage],
    observation_seconds: float,
) -> bool:
    def is_terminal(message: ServerMessage) -> bool:
        return (
            message.method == "turn/completed"
            and message.params.get("threadId") == thread_id
            and _nested_turn_id(message.params) == turn_id
        )

    if any(is_terminal(message) for message in preceding):
        return True
    deadline = asyncio.get_running_loop().time() + observation_seconds
    while (remaining := deadline - asyncio.get_running_loop().time()) > 0:
        try:
            message = await client.next_message(timeout=remaining)
        except TimeoutError:
            return False
        if is_terminal(message):
            return True
    return False


async def _wait_for_terminal(
    client: AppServerClient,
    thread_id: str,
    turn_id: str,
    timeout: float,
    *,
    preceding: list[ServerMessage] | None = None,
) -> TurnObservation:
    observed = list(preceding or [])
    terminal: JsonObject | None = None
    async with asyncio.timeout(timeout):
        while terminal is None:
            message = await client.next_message(timeout=timeout)
            observed.append(message)
            if (
                message.method == "turn/completed"
                and message.params.get("threadId") == thread_id
                and _nested_turn_id(message.params) == turn_id
            ):
                terminal = _required_object(
                    message.params.get("turn"), "turn/completed.turn"
                )

    finals: list[str] = []
    commentary: list[str] = []
    item_types: set[str] = set()
    methods: set[str] = set()
    for message in observed:
        methods.add(message.method)
        if message.params.get("threadId") != thread_id:
            continue
        message_turn_id = message.params.get("turnId") or _nested_turn_id(
            message.params
        )
        if message_turn_id != turn_id:
            continue
        item = message.params.get("item")
        if not isinstance(item, dict):
            continue
        item_type = item.get("type")
        if isinstance(item_type, str):
            item_types.add(item_type)
        if message.method != "item/completed" or item_type != "agentMessage":
            continue
        text = item.get("text")
        phase = item.get("phase")
        if not isinstance(item.get("id"), str) or not isinstance(text, str):
            continue
        if phase == "final_answer":
            finals.append(text)
        elif phase == "commentary":
            commentary.append(text)

    status = terminal.get("status")
    if status not in {"completed", "failed", "inProgress", "interrupted"}:
        raise AppServerProtocolError("turn/completed returned an unknown status")
    return TurnObservation(
        turn_id=turn_id,
        status=status,
        final_answers=tuple(finals),
        commentary_messages=tuple(commentary),
        item_types=tuple(sorted(item_types)),
        notification_methods=tuple(sorted(methods)),
    )


def _require_latest_final(observation: TurnObservation, expected: str) -> None:
    if observation.status != "completed":
        raise AppServerProtocolError(
            f"probe turn {observation.turn_id} ended as {observation.status!r}"
        )
    if latest_final_answer(observation) != expected:
        raise AppServerProtocolError(
            f"probe turn did not end with the expected explicit final_answer: {expected}"
        )


def latest_final_answer(observation: TurnObservation) -> str | None:
    """Return the final answer that remains authoritative at turn completion."""

    if not observation.final_answers:
        return None
    return observation.final_answers[-1]


def _nested_turn_id(params: JsonObject) -> str | None:
    turn = params.get("turn")
    if not isinstance(turn, dict):
        return None
    turn_id = turn.get("id")
    return turn_id if isinstance(turn_id, str) else None


def _required_object(value: Any, owner: str) -> JsonObject:
    if not isinstance(value, dict):
        raise AppServerProtocolError(f"{owner} is not an object")
    return value


def _required_string(value: JsonObject, key: str, owner: str) -> str:
    member = value.get(key)
    if not isinstance(member, str) or not member:
        raise AppServerProtocolError(f"{owner}.{key} is not a non-empty string")
    return member


def _verify_and_digest_schema(directory: Path) -> str:
    client_schema = directory / "ClientRequest.json"
    server_schema = directory / "ServerNotification.json"
    bundle = directory / "codex_app_server_protocol.v2.schemas.json"
    for path in (client_schema, server_schema, bundle):
        if not path.is_file():
            raise AppServerProtocolError(f"schema generator omitted {path.name}")
    client_methods = _declared_methods(json.loads(client_schema.read_bytes()))
    server_methods = _declared_methods(json.loads(server_schema.read_bytes()))
    missing_client = sorted(
        method for method in REQUIRED_CLIENT_METHODS if method not in client_methods
    )
    missing_server = sorted(
        method for method in REQUIRED_SERVER_METHODS if method not in server_methods
    )
    if missing_client or missing_server:
        raise AppServerProtocolError(
            "installed app-server schema lacks required methods: "
            f"client={missing_client}, server={missing_server}"
        )
    return hashlib.sha256(bundle.read_bytes()).hexdigest()


def _verify_application_context_contract(directory: Path) -> None:
    for filename in ("TurnStartParams.json", "TurnSteerParams.json"):
        path = directory / "v2" / filename
        if not path.is_file():
            raise AppServerProtocolError(
                f"experimental schema generator omitted v2/{filename}"
            )
        schema = json.loads(path.read_bytes())
        properties = schema.get("properties")
        if not isinstance(properties, dict) or "additionalContext" not in properties:
            raise AppServerProtocolError(
                f"experimental v2/{filename} lacks additionalContext"
            )
        required = schema.get("required")
        if not isinstance(required, list) or "input" not in required:
            raise AppServerProtocolError(
                f"experimental v2/{filename} no longer requires input"
            )
        definitions = schema.get("definitions")
        kinds = None
        if isinstance(definitions, dict):
            kind = definitions.get("AdditionalContextKind")
            if isinstance(kind, dict):
                kinds = kind.get("enum")
        if not isinstance(kinds, list) or "application" not in kinds:
            raise AppServerProtocolError(
                f"experimental v2/{filename} lacks application context kind"
            )


def _declared_methods(value: Any) -> frozenset[str]:
    methods: set[str] = set()

    def visit(node: Any) -> None:
        if isinstance(node, dict):
            properties = node.get("properties")
            if isinstance(properties, dict):
                method = properties.get("method")
                if isinstance(method, dict):
                    enum = method.get("enum")
                    if isinstance(enum, list):
                        methods.update(item for item in enum if isinstance(item, str))
            for child in node.values():
                visit(child)
        elif isinstance(node, list):
            for child in node:
                visit(child)

    visit(value)
    return frozenset(methods)


async def _capture_stdout(
    executable: Path, *arguments: str, timeout: float
) -> str:
    stdout = await _run_process(executable, *arguments, timeout=timeout)
    value = stdout.decode("utf-8", errors="strict").strip()
    if not value:
        raise AppServerProtocolError("version command returned empty stdout")
    return value


async def _run_process(
    executable: Path,
    *arguments: str | Path,
    timeout: float,
) -> bytes:
    process = await asyncio.create_subprocess_exec(
        str(executable),
        *(str(argument) for argument in arguments),
        env=provider_environment(),
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    try:
        stdout, stderr = await asyncio.wait_for(process.communicate(), timeout)
    except TimeoutError:
        process.kill()
        await process.wait()
        raise
    if process.returncode != 0:
        detail = stderr.decode("utf-8", errors="replace").strip()
        raise AppServerProtocolError(
            f"app-server helper exited {process.returncode}: {detail}"
        )
    return stdout
