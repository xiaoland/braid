from __future__ import annotations

import asyncio
import sys
import unittest

from github_agent_bridge.app_server import (
    AppServerClient,
    AppServerExited,
    AppServerProtocolError,
    provider_environment,
)


class AppServerWireTests(unittest.TestCase):
    def test_exact_jsonl_handshake_and_notification(self) -> None:
        script = r'''
import json
import sys

initialize = json.loads(sys.stdin.readline())
assert "jsonrpc" not in initialize
assert initialize["method"] == "initialize"
print(json.dumps({"id": initialize["id"], "result": {"userAgent": "fake"}}), flush=True)
initialized = json.loads(sys.stdin.readline())
assert initialized == {"method": "initialized"}
request = json.loads(sys.stdin.readline())
print(json.dumps({"method": "probe/notice", "params": {"sequence": 1}}), flush=True)
print(json.dumps({"id": request["id"], "result": {"pong": True}}), flush=True)
sys.stdin.read()
'''

        async def scenario() -> None:
            client = await AppServerClient.start(
                (sys.executable, "-u", "-c", script),
                environment=provider_environment(),
            )
            try:
                initialized = await client.initialize(
                    client_name="wire-test",
                    client_version="1",
                    timeout=2,
                )
                self.assertEqual(initialized, {"userAgent": "fake"})
                result = await client.request("probe/ping", {}, timeout=2)
                self.assertEqual(result, {"pong": True})
                message = await client.next_message(timeout=2)
                self.assertEqual(message.method, "probe/notice")
                self.assertEqual(message.params, {"sequence": 1})
            finally:
                await client.close()

        asyncio.run(scenario())

    def test_invalid_ndjson_fails_pending_request(self) -> None:
        script = r'''
import sys
sys.stdin.readline()
print("{invalid", flush=True)
'''

        async def scenario() -> None:
            client = await AppServerClient.start(
                (sys.executable, "-u", "-c", script),
                environment=provider_environment(),
            )
            try:
                with self.assertRaises(AppServerProtocolError):
                    await client.initialize(
                        client_name="wire-test",
                        client_version="1",
                        timeout=2,
                    )
            finally:
                await client.close()

        asyncio.run(scenario())

    def test_eof_fails_pending_request(self) -> None:
        script = "import sys; sys.stdin.readline()"

        async def scenario() -> None:
            client = await AppServerClient.start(
                (sys.executable, "-u", "-c", script),
                environment=provider_environment(),
            )
            try:
                with self.assertRaises(AppServerExited):
                    await client.initialize(
                        client_name="wire-test",
                        client_version="1",
                        timeout=2,
                    )
            finally:
                await client.close()

        asyncio.run(scenario())


if __name__ == "__main__":
    unittest.main()
