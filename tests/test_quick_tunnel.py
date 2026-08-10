from __future__ import annotations

import unittest

from github_agent_bridge.quick_tunnel import (
    extract_quick_tunnel_url,
    validate_quick_tunnel_origin,
)


class QuickTunnelTests(unittest.TestCase):
    def test_extracts_only_free_trycloudflare_https_url(self) -> None:
        self.assertEqual(
            extract_quick_tunnel_url(
                "ready at https://random-words.trycloudflare.com"
            ),
            "https://random-words.trycloudflare.com",
        )
        self.assertIsNone(extract_quick_tunnel_url("http://localhost:8080"))
        self.assertIsNone(extract_quick_tunnel_url("https://example.com"))

    def test_origin_must_be_explicit_loopback_http(self) -> None:
        for valid in ("http://127.0.0.1:8080", "http://[::1]:8080"):
            with self.subTest(valid=valid):
                validate_quick_tunnel_origin(valid)
        for invalid in (
            "https://127.0.0.1:8080",
            "http://0.0.0.0:8080",
            "http://localhost:8080",
            "http://127.0.0.1",
        ):
            with self.subTest(invalid=invalid):
                with self.assertRaises(ValueError):
                    validate_quick_tunnel_origin(invalid)


if __name__ == "__main__":
    unittest.main()
