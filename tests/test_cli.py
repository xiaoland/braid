from __future__ import annotations

import io
import json
import tempfile
import unittest
from pathlib import Path
from typing import Any

from dataclasses import dataclass

from github_agent_bridge.cli import (
    CONFIGURATION_ERROR,
    PROBE_ERROR,
    RUNTIME_ERROR,
    main,
)
from github_agent_bridge.runtime import RuntimeResult
from test_config import valid_config


class CliTests(unittest.TestCase):
    def write_config(self, directory: str, value: dict[str, Any]) -> Path:
        path = Path(directory) / "config.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def test_config_check_accepts_valid_configuration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = self.write_config(directory, valid_config())
            stdout = io.StringIO()
            stderr = io.StringIO()

            exit_code = main(
                ["config-check", "--config", str(path)],
                stdout=stdout,
                stderr=stderr,
            )

        self.assertEqual(exit_code, 0)
        self.assertEqual(stdout.getvalue(), "configuration valid\n")
        self.assertEqual(stderr.getvalue(), "")

    def test_config_check_rejects_invalid_configuration_without_value_leak(self) -> None:
        value = valid_config()
        value["github"]["webhook_secret"] = {"value": "plaintext-secret"}

        with tempfile.TemporaryDirectory() as directory:
            path = self.write_config(directory, value)
            stdout = io.StringIO()
            stderr = io.StringIO()

            exit_code = main(
                ["config-check", "--config", str(path)],
                stdout=stdout,
                stderr=stderr,
            )

        self.assertEqual(exit_code, CONFIGURATION_ERROR)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("configuration invalid", stderr.getvalue())
        self.assertNotIn("plaintext-secret", stderr.getvalue())

    def test_probe_command_prints_only_the_settled_report(self) -> None:
        @dataclass
        class Report:
            def to_json(self) -> str:
                return '{"probe":"passed"}'

        async def probe(**_arguments: object) -> Any:
            return Report()

        stdout = io.StringIO()
        stderr = io.StringIO()
        exit_code = main(
            [
                "probe-app-server",
                "--codex",
                "/usr/local/bin/codex",
                "--workspace",
                "/tmp/probe",
            ],
            stdout=stdout,
            stderr=stderr,
            app_server_probe=probe,
        )

        self.assertEqual(exit_code, 0)
        self.assertEqual(stdout.getvalue(), '{"probe":"passed"}\n')
        self.assertEqual(stderr.getvalue(), "")

    def test_probe_command_reports_failure_without_success(self) -> None:
        async def probe(**_arguments: object) -> Any:
            raise ValueError("bad workspace")

        stdout = io.StringIO()
        stderr = io.StringIO()
        exit_code = main(
            [
                "probe-app-server",
                "--codex",
                "/usr/local/bin/codex",
                "--workspace",
                "/tmp/probe",
            ],
            stdout=stdout,
            stderr=stderr,
            app_server_probe=probe,
        )

        self.assertEqual(exit_code, PROBE_ERROR)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("provider probe failed", stderr.getvalue())

    def test_serve_reports_running_then_stopped_without_secret_values(self) -> None:
        async def run_bridge(**arguments: object) -> RuntimeResult:
            started = arguments["on_started"]
            result = RuntimeResult(
                binding_id="binding-1",
                thread_address="thread-1",
                public_webhook_url="https://example.trycloudflare.com/webhooks/github",
            )
            started(result)  # type: ignore[operator]
            return result

        with tempfile.TemporaryDirectory() as directory:
            path = self.write_config(directory, valid_config())
            stdout = io.StringIO()
            stderr = io.StringIO()
            exit_code = main(
                [
                    "serve",
                    "--config",
                    str(path),
                    "--repository",
                    "owner/repository",
                    "--issue-number",
                    "17",
                    "--wrangler",
                    "/usr/local/bin/wrangler",
                ],
                stdout=stdout,
                stderr=stderr,
                bridge_runner=run_bridge,
            )

        lines = [json.loads(line) for line in stdout.getvalue().splitlines()]
        self.assertEqual([line["status"] for line in lines], ["running", "stopped"])
        self.assertEqual(lines[0]["binding_id"], "binding-1")
        self.assertEqual(stderr.getvalue(), "")

    def test_serve_rejects_ambiguous_repository_before_runtime(self) -> None:
        async def run_bridge(**_arguments: object) -> RuntimeResult:
            raise AssertionError("runtime must not start")

        with tempfile.TemporaryDirectory() as directory:
            path = self.write_config(directory, valid_config())
            stderr = io.StringIO()
            exit_code = main(
                [
                    "serve",
                    "--config",
                    str(path),
                    "--repository",
                    "not-a-repository",
                    "--issue-number",
                    "17",
                ],
                stdout=io.StringIO(),
                stderr=stderr,
                bridge_runner=run_bridge,
            )

        self.assertEqual(exit_code, RUNTIME_ERROR)
        self.assertIn("OWNER/REPOSITORY", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
