"""Small, typed-enough transport for the Codex app-server stdio protocol.

The app-server owns thread state.  This module only keeps request futures and
live notifications for the lifetime of one transport connection.
"""

from __future__ import annotations

import asyncio
from collections import deque
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import json
import os
from pathlib import Path
from typing import Any


JsonObject = dict[str, Any]


class AppServerError(RuntimeError):
    """Base error for the local app-server transport."""


class AppServerProtocolError(AppServerError):
    """The peer emitted data that does not satisfy the NDJSON/RPC contract."""


class AppServerExited(AppServerError):
    """The app-server transport ended before an operation settled."""


@dataclass(frozen=True, slots=True)
class AppServerRemoteError(AppServerError):
    """An error response returned by app-server."""

    code: int
    message: str
    data: Any = None

    def __str__(self) -> str:
        return f"app-server error {self.code}: {self.message}"


@dataclass(frozen=True, slots=True)
class ServerMessage:
    """One protocol notification or server-initiated request."""

    method: str
    params: JsonObject
    request_id: int | str | None = None
    emitted_at_ms: int | None = None


# Wrapper credentials are intentionally absent.  Provider credentials should
# normally come from the provider's own durable auth store under HOME/CODEX_HOME.
DEFAULT_PROVIDER_ENVIRONMENT_NAMES = frozenset(
    {
        "CODEX_HOME",
        "HOME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "LOGNAME",
        "PATH",
        "SHELL",
        "SSL_CERT_DIR",
        "SSL_CERT_FILE",
        "TERM",
        "TERM_PROGRAM",
        "TMPDIR",
        "USER",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
    }
)


def provider_environment(
    source: Mapping[str, str] | None = None,
    *,
    names: frozenset[str] = DEFAULT_PROVIDER_ENVIRONMENT_NAMES,
) -> dict[str, str]:
    """Project an explicit, secret-minimizing environment for app-server."""

    source = os.environ if source is None else source
    projected = {
        name: value
        for name, value in source.items()
        if name in names
    }
    projected.setdefault("PATH", os.defpath)
    return projected


class AppServerClient:
    """One supervised NDJSON connection to a local Codex app-server process."""

    def __init__(self, process: asyncio.subprocess.Process) -> None:
        if process.stdin is None or process.stdout is None or process.stderr is None:
            raise ValueError("app-server subprocess must use piped stdio")
        self._process = process
        self._next_request_id = 1
        self._pending: dict[int, asyncio.Future[Any]] = {}
        self._messages: asyncio.Queue[ServerMessage | BaseException] = asyncio.Queue()
        self._stderr_tail: deque[str] = deque(maxlen=20)
        self._closed = False
        self._stdout_task = asyncio.create_task(self._read_stdout())
        self._stderr_task = asyncio.create_task(self._read_stderr())

    @classmethod
    async def start(
        cls,
        command: Sequence[str] = ("codex", "app-server", "--stdio"),
        *,
        cwd: Path | None = None,
        environment: Mapping[str, str] | None = None,
    ) -> AppServerClient:
        if not command:
            raise ValueError("app-server command must not be empty")
        process = await asyncio.create_subprocess_exec(
            *command,
            cwd=None if cwd is None else os.fspath(cwd),
            env=None if environment is None else dict(environment),
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        return cls(process)

    @property
    def returncode(self) -> int | None:
        return self._process.returncode

    @property
    def stderr_tail(self) -> tuple[str, ...]:
        return tuple(self._stderr_tail)

    async def initialize(
        self,
        *,
        client_name: str,
        client_version: str,
        experimental_api: bool = False,
        timeout: float,
    ) -> JsonObject:
        params: JsonObject = {
            "clientInfo": {"name": client_name, "version": client_version}
        }
        if experimental_api:
            params["capabilities"] = {"experimentalApi": True}
        result = await self.request(
            "initialize",
            params,
            timeout=timeout,
        )
        if not isinstance(result, dict):
            raise AppServerProtocolError("initialize result is not an object")
        await self.notify("initialized")
        return result

    async def request(
        self,
        method: str,
        params: JsonObject,
        *,
        timeout: float,
    ) -> Any:
        if self._closed:
            raise AppServerExited("app-server connection is closed")
        request_id = self._next_request_id
        self._next_request_id += 1
        future = asyncio.get_running_loop().create_future()
        self._pending[request_id] = future
        try:
            await self._write(
                {
                    "id": request_id,
                    "method": method,
                    "params": params,
                }
            )
            return await asyncio.wait_for(future, timeout)
        finally:
            self._pending.pop(request_id, None)

    async def notify(self, method: str, params: JsonObject | None = None) -> None:
        if self._closed:
            raise AppServerExited("app-server connection is closed")
        payload: JsonObject = {"method": method}
        if params is not None:
            payload["params"] = params
        await self._write(payload)

    async def next_message(self, *, timeout: float) -> ServerMessage:
        message = await asyncio.wait_for(self._messages.get(), timeout)
        if isinstance(message, BaseException):
            raise message
        return message

    async def close(self, *, timeout: float = 5.0) -> None:
        if self._closed:
            return
        self._closed = True
        if self._process.stdin is not None and not self._process.stdin.is_closing():
            self._process.stdin.close()
            try:
                await self._process.stdin.wait_closed()
            except (BrokenPipeError, ConnectionResetError):
                pass
        try:
            await asyncio.wait_for(self._process.wait(), timeout)
        except TimeoutError:
            self._process.terminate()
            try:
                await asyncio.wait_for(self._process.wait(), timeout)
            except TimeoutError:
                self._process.kill()
                await self._process.wait()
        for task in (self._stdout_task, self._stderr_task):
            if not task.done():
                task.cancel()
        await asyncio.gather(self._stdout_task, self._stderr_task, return_exceptions=True)

    async def __aenter__(self) -> AppServerClient:
        return self

    async def __aexit__(self, *_: object) -> None:
        await self.close()

    async def _write(self, payload: JsonObject) -> None:
        stdin = self._process.stdin
        if stdin is None or stdin.is_closing():
            raise AppServerExited("app-server stdin is unavailable")
        encoded = json.dumps(payload, separators=(",", ":"), ensure_ascii=False)
        stdin.write(encoded.encode("utf-8") + b"\n")
        try:
            await stdin.drain()
        except (BrokenPipeError, ConnectionResetError) as error:
            raise AppServerExited("app-server stdin closed") from error

    async def _read_stdout(self) -> None:
        stdout = self._process.stdout
        assert stdout is not None
        failure: BaseException | None = None
        try:
            while line := await stdout.readline():
                try:
                    payload = json.loads(line)
                except (UnicodeDecodeError, json.JSONDecodeError) as error:
                    raise AppServerProtocolError(
                        "app-server emitted invalid NDJSON on stdout"
                    ) from error
                if not isinstance(payload, dict):
                    raise AppServerProtocolError(
                        "app-server protocol message is not an object"
                    )
                if "id" in payload and ("result" in payload or "error" in payload):
                    self._settle_response(payload)
                    continue
                method = payload.get("method")
                if not isinstance(method, str):
                    raise AppServerProtocolError(
                        "app-server message has neither a response nor method"
                    )
                params = payload.get("params", {})
                if not isinstance(params, dict):
                    raise AppServerProtocolError("app-server message params is not an object")
                request_id = payload.get("id")
                if request_id is not None and not isinstance(request_id, (int, str)):
                    raise AppServerProtocolError("server request id has an invalid type")
                emitted_at_ms = payload.get("emittedAtMs")
                await self._messages.put(
                    ServerMessage(
                        method=method,
                        params=params,
                        request_id=request_id,
                        emitted_at_ms=(
                            emitted_at_ms if isinstance(emitted_at_ms, int) else None
                        ),
                    )
                )
        except asyncio.CancelledError:
            raise
        except BaseException as error:
            failure = error
        finally:
            if failure is None and not self._closed:
                failure = AppServerExited("app-server stdout reached EOF")
            if failure is not None:
                for future in tuple(self._pending.values()):
                    if not future.done():
                        future.set_exception(failure)
                await self._messages.put(failure)

    def _settle_response(self, payload: JsonObject) -> None:
        request_id = payload.get("id")
        if not isinstance(request_id, int):
            raise AppServerProtocolError("response id is not an integer")
        future = self._pending.get(request_id)
        if future is None or future.done():
            return
        error = payload.get("error")
        if isinstance(error, dict):
            future.set_exception(
                AppServerRemoteError(
                    code=int(error.get("code", -1)),
                    message=str(error.get("message", "unknown remote error")),
                    data=error.get("data"),
                )
            )
            return
        future.set_result(payload.get("result"))

    async def _read_stderr(self) -> None:
        stderr = self._process.stderr
        assert stderr is not None
        try:
            while line := await stderr.readline():
                self._stderr_tail.append(line.decode("utf-8", errors="replace").rstrip())
        except asyncio.CancelledError:
            raise
