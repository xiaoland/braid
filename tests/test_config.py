from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from typing import Any

from pydantic import ValidationError

from github_agent_bridge.config import (
    BridgeConfig,
    ConfigLoadError,
    SecretLoadError,
    SecretReference,
    load_config,
    load_secret,
)


def valid_config() -> dict[str, Any]:
    return {
        "github": {
            "app_id": 12345,
            "private_key": {"file": "/run/secrets/github-app.pem"},
            "webhook_secret": {"environment": "WRAPPER_WEBHOOK_SECRET"},
            "agent_login": "coding-agent-bot",
            "wrapper_login": "wrapper-app[bot]",
        },
        "ingress": {"host": "127.0.0.1", "port": 8080, "health_port": 8081},
        "timing": {
            "quiet_window_seconds": 30.0,
            "mirror_interval_seconds": 5.0,
            "reconciliation_interval_seconds": 60.0,
            "mirror_comment_bytes": 60000,
        },
        "paths": {
            "state_database": "/var/lib/github-agent-bridge/state.sqlite3",
            "provider_cwd": "/worktrees/issue-123",
            "collaboration_instructions": "/workspace/agent-handoff/AGENTS.md",
        },
        "app_server": {
            "executable": "/usr/local/bin/codex",
            "version": "codex-cli 0.147.0-alpha.6.5",
            "stable_schema_sha256": (
                "7d79fe309dd7520843459070f3884ecf0e39cee2620c1c49aad6efb4eca76ecb"
            ),
            "experimental_schema_sha256": (
                "a14d4878fe7b8cdd31059dbca11d7167d8cfd06effa2f7991b5364439063a5c8"
            ),
            "environment_allowlist": ["HOME", "PATH", "GH_TOKEN"],
        },
    }


class BridgeConfigTests(unittest.TestCase):
    def parse(self, value: dict[str, Any]) -> BridgeConfig:
        return BridgeConfig.model_validate_json(json.dumps(value))

    def test_accepts_strict_reference_only_configuration(self) -> None:
        config = self.parse(valid_config())

        self.assertTrue(config.ingress.host.is_loopback)
        self.assertEqual(config.github.private_key.file, Path("/run/secrets/github-app.pem"))
        self.assertEqual(config.paths.provider_writable_roots, ())
        self.assertEqual(config.github.wrapper_login, "wrapper-app[bot]")

    def test_rejects_unknown_fields_in_nested_models(self) -> None:
        value = valid_config()
        value["github"]["webhook_secret"] = {
            "environment": "WRAPPER_WEBHOOK_SECRET",
            "value": "must-not-be-accepted",
        }

        with self.assertRaises(ValidationError):
            self.parse(value)

    def test_rejects_non_loopback_ingress(self) -> None:
        value = valid_config()
        value["ingress"] = {
            "host": "0.0.0.0",
            "port": 8080,
            "health_port": 8081,
        }

        with self.assertRaisesRegex(ValidationError, "loopback"):
            self.parse(value)

    def test_rejects_shared_webhook_and_health_port(self) -> None:
        value = valid_config()
        value["ingress"]["health_port"] = 8080

        with self.assertRaisesRegex(ValidationError, "must be distinct"):
            self.parse(value)

    def test_rejects_non_positive_and_non_finite_timing(self) -> None:
        for invalid_value in (0.0, -1.0, float("inf"), float("nan")):
            with self.subTest(invalid_value=invalid_value):
                value = valid_config()
                value["timing"]["quiet_window_seconds"] = invalid_value

                with self.assertRaises(ValidationError):
                    self.parse(value)

    def test_secret_reference_requires_exactly_one_source(self) -> None:
        for reference in (
            {},
            {
                "environment": "WRAPPER_PRIVATE_KEY",
                "file": "/run/secrets/github-app.pem",
            },
        ):
            with self.subTest(reference=reference):
                value = valid_config()
                value["github"]["private_key"] = reference

                with self.assertRaisesRegex(ValidationError, "exactly one"):
                    self.parse(value)

    def test_rejects_secret_reference_in_provider_environment(self) -> None:
        value = valid_config()
        value["app_server"]["environment_allowlist"] = [
            "PATH",
            "WRAPPER_WEBHOOK_SECRET",
        ]

        with self.assertRaisesRegex(ValidationError, "exposes Wrapper secret"):
            self.parse(value)

    def test_rejects_string_coercion_for_numeric_values(self) -> None:
        value = valid_config()
        value["github"]["app_id"] = "12345"

        with self.assertRaises(ValidationError):
            self.parse(value)

    def test_load_error_does_not_echo_rejected_values(self) -> None:
        value = valid_config()
        value["github"]["webhook_secret"] = {"value": "plaintext-secret"}

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "config.json"
            path.write_text(json.dumps(value), encoding="utf-8")

            with self.assertRaises(ConfigLoadError) as raised:
                load_config(path)

        self.assertNotIn("plaintext-secret", str(raised.exception))

    def test_secret_loading_preserves_exact_bytes_without_echoing_them(self) -> None:
        environment_secret = "webhook-secret-with-whitespace\n"
        loaded = load_secret(
            SecretReference(environment="WRAPPER_WEBHOOK_SECRET"),
            environment={"WRAPPER_WEBHOOK_SECRET": environment_secret},
        )
        self.assertEqual(loaded, environment_secret.encode("utf-8"))

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "app.pem"
            file_secret = b"private-key-material\n"
            path.write_bytes(file_secret)
            self.assertEqual(load_secret(SecretReference(file=path)), file_secret)

            path.write_bytes(b"")
            with self.assertRaises(SecretLoadError) as raised:
                load_secret(SecretReference(file=path))
            self.assertNotIn("private-key-material", str(raised.exception))

        with self.assertRaisesRegex(SecretLoadError, "missing"):
            load_secret(
                SecretReference(environment="WRAPPER_WEBHOOK_SECRET"),
                environment={},
            )


if __name__ == "__main__":
    unittest.main()
