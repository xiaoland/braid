"""Operator command-line boundary."""

from __future__ import annotations

import argparse
import asyncio
from dataclasses import asdict
import json
import sys
from collections.abc import Awaitable, Callable, Sequence
from pathlib import Path
from typing import NoReturn, TextIO

from braid.app_server import AppServerError
from braid.config import ConfigLoadError, SecretLoadError, load_config
from braid.github_api import GitHubApiError
from braid.protocol_probe import ProtocolProbeReport, run_protocol_probe
from braid.quick_tunnel import QuickTunnelError
from braid.runtime import BridgeRuntimeError, RuntimeResult, serve_bridge
from braid.store import StoreError

CONFIGURATION_ERROR = 2
PROBE_ERROR = 3
RUNTIME_ERROR = 4

AppServerProbe = Callable[..., Awaitable[ProtocolProbeReport]]
BridgeRunner = Callable[..., Awaitable[RuntimeResult]]


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="braid")
    subparsers = parser.add_subparsers(dest="command", required=True)

    config_check = subparsers.add_parser(
        "config-check", help="validate a JSON configuration file"
    )
    config_check.add_argument("--config", type=Path, required=True)

    provider_probe = subparsers.add_parser(
        "probe-app-server",
        help="run a real read-only Codex app-server protocol probe",
    )
    provider_probe.add_argument("--codex", type=Path, required=True)
    provider_probe.add_argument("--workspace", type=Path, required=True)
    provider_probe.add_argument("--timeout-seconds", type=float, default=120.0)

    serve = subparsers.add_parser(
        "serve", help="run one Issue-bound transport bridge"
    )
    serve.add_argument("--config", type=Path, required=True)
    serve.add_argument("--repository", required=True, metavar="OWNER/REPOSITORY")
    serve.add_argument("--issue-number", type=int, required=True)
    serve.add_argument(
        "--wrangler",
        type=Path,
        help="absolute Wrangler executable path; omit for loopback-only ingress",
    )

    return parser


def main(
    argv: Sequence[str] | None = None,
    *,
    stdout: TextIO | None = None,
    stderr: TextIO | None = None,
    app_server_probe: AppServerProbe = run_protocol_probe,
    bridge_runner: BridgeRunner = serve_bridge,
) -> int:
    output = stdout if stdout is not None else sys.stdout
    errors = stderr if stderr is not None else sys.stderr
    args = build_parser().parse_args(argv)

    if args.command == "config-check":
        try:
            load_config(args.config)
        except ConfigLoadError as error:
            print(f"configuration invalid: {error}", file=errors)
            return CONFIGURATION_ERROR
        print("configuration valid", file=output)
        return 0

    if args.command == "probe-app-server":
        if args.timeout_seconds <= 0:
            print("provider probe failed: timeout must be positive", file=errors)
            return PROBE_ERROR
        try:
            report = asyncio.run(
                app_server_probe(
                    codex_executable=args.codex,
                    workspace=args.workspace,
                    timeout_seconds=args.timeout_seconds,
                )
            )
        except (AppServerError, OSError, TimeoutError, ValueError) as error:
            print(f"provider probe failed: {error}", file=errors)
            return PROBE_ERROR
        print(report.to_json(), file=output)
        return 0

    if args.command == "serve":
        try:
            config = load_config(args.config)
            if args.issue_number < 1:
                raise ValueError("issue number must be positive")
            if args.repository.count("/") != 1 or any(
                not part for part in args.repository.split("/")
            ):
                raise ValueError("repository must use OWNER/REPOSITORY")

            def report_started(result: RuntimeResult) -> None:
                print(
                    json.dumps(
                        {"status": "running", **asdict(result)},
                        ensure_ascii=False,
                        separators=(",", ":"),
                    ),
                    file=output,
                    flush=True,
                )

            result = asyncio.run(
                bridge_runner(
                    config=config,
                    repository_full_name=args.repository,
                    issue_number=args.issue_number,
                    wrangler_executable=args.wrangler,
                    on_started=report_started,
                )
            )
        except KeyboardInterrupt:
            return 130
        except (
            AppServerError,
            BridgeRuntimeError,
            ConfigLoadError,
            GitHubApiError,
            OSError,
            QuickTunnelError,
            SecretLoadError,
            StoreError,
            ValueError,
        ) as error:
            print(f"runtime failed: {error}", file=errors)
            return RUNTIME_ERROR
        print(
            json.dumps(
                {"status": "stopped", **asdict(result)},
                ensure_ascii=False,
                separators=(",", ":"),
            ),
            file=output,
        )
        return 0

    raise AssertionError(f"unhandled command: {args.command}")


def entrypoint() -> NoReturn:
    raise SystemExit(main())
