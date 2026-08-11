"""Supervise a free Cloudflare Wrangler Quick Tunnel process."""

from __future__ import annotations

import asyncio
from collections import deque
from collections.abc import Mapping
from dataclasses import dataclass
from ipaddress import ip_address
from pathlib import Path
import re
from urllib.parse import urlparse


QUICK_TUNNEL_URL = re.compile(
    r"https://[a-z0-9-]+\.trycloudflare\.com(?:/[^\s]*)?", re.IGNORECASE
)


class QuickTunnelError(RuntimeError):
    """The free tunnel could not establish or remain supervised."""


@dataclass(frozen=True, slots=True)
class QuickTunnelStatus:
    public_url: str
    origin_url: str
    process_id: int


class WranglerQuickTunnel:
    def __init__(
        self,
        process: asyncio.subprocess.Process,
        *,
        public_url: str,
        origin_url: str,
        reader_tasks: tuple[asyncio.Task[None], ...],
        output_tail: deque[str],
    ) -> None:
        self._process = process
        self.public_url = public_url.rstrip("/")
        self.origin_url = origin_url.rstrip("/")
        self._reader_tasks = reader_tasks
        self._output_tail = output_tail

    @classmethod
    async def start(
        cls,
        *,
        wrangler_executable: Path,
        origin_url: str,
        environment: Mapping[str, str] | None = None,
        timeout_seconds: float = 30.0,
    ) -> WranglerQuickTunnel:
        executable = wrangler_executable.resolve(strict=True)
        validate_quick_tunnel_origin(origin_url)
        if timeout_seconds <= 0:
            raise ValueError("timeout_seconds must be positive")
        process = await asyncio.create_subprocess_exec(
            str(executable),
            "tunnel",
            "quick-start",
            origin_url,
            env=None if environment is None else dict(environment),
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        assert process.stdout is not None
        assert process.stderr is not None
        queue: asyncio.Queue[str] = asyncio.Queue()
        tail: deque[str] = deque(maxlen=40)
        tasks = (
            asyncio.create_task(_read_lines(process.stdout, queue, tail)),
            asyncio.create_task(_read_lines(process.stderr, queue, tail)),
        )
        try:
            async with asyncio.timeout(timeout_seconds):
                while True:
                    if process.returncode is not None:
                        raise QuickTunnelError(
                            "Wrangler exited before publishing a Quick Tunnel URL"
                        )
                    line = await queue.get()
                    public_url = extract_quick_tunnel_url(line)
                    if public_url is not None:
                        return cls(
                            process,
                            public_url=public_url,
                            origin_url=origin_url,
                            reader_tasks=tasks,
                            output_tail=tail,
                        )
        except BaseException:
            if process.returncode is None:
                process.terminate()
                await process.wait()
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
            raise

    @property
    def status(self) -> QuickTunnelStatus:
        return QuickTunnelStatus(
            public_url=self.public_url,
            origin_url=self.origin_url,
            process_id=self._process.pid,
        )

    @property
    def output_tail(self) -> tuple[str, ...]:
        return tuple(self._output_tail)

    async def wait_terminated(self) -> int:
        """Wait for the supervised tunnel process to exit."""

        return await self._process.wait()

    async def close(self, *, timeout_seconds: float = 5.0) -> None:
        if self._process.returncode is None:
            self._process.terminate()
            try:
                await asyncio.wait_for(self._process.wait(), timeout_seconds)
            except TimeoutError:
                self._process.kill()
                await self._process.wait()
        for task in self._reader_tasks:
            if not task.done():
                task.cancel()
        await asyncio.gather(*self._reader_tasks, return_exceptions=True)

    async def __aenter__(self) -> WranglerQuickTunnel:
        return self

    async def __aexit__(self, *_: object) -> None:
        await self.close()


async def _read_lines(
    stream: asyncio.StreamReader,
    queue: asyncio.Queue[str],
    tail: deque[str],
) -> None:
    try:
        while line := await stream.readline():
            value = line.decode("utf-8", errors="replace").rstrip()
            tail.append(value)
            await queue.put(value)
    except asyncio.CancelledError:
        raise


def extract_quick_tunnel_url(line: str) -> str | None:
    match = QUICK_TUNNEL_URL.search(line)
    return None if match is None else match.group(0)


def validate_quick_tunnel_origin(origin_url: str) -> None:
    parsed = urlparse(origin_url)
    if parsed.scheme != "http" or parsed.hostname is None or parsed.port is None:
        raise ValueError("Quick Tunnel origin must be an explicit HTTP loopback URL")
    try:
        address = ip_address(parsed.hostname)
    except ValueError as error:
        raise ValueError("Quick Tunnel origin hostname must be an IP address") from error
    if not address.is_loopback:
        raise ValueError("Quick Tunnel origin must be loopback-only")
