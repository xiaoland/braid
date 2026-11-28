# Braid Tunnel

`braid serve --tunnel` exposes the local webhook ingress through a free,
ephemeral Cloudflare Quick Tunnel so GitHub can deliver webhooks without any
network configuration on your side.

## What it does

When you run:

```shell
braid serve --instance <KEY> --tunnel
```

Braid performs these steps automatically:

1. Starts the local webhook ingress on the loopback address configured in
   `server.ingress` (default `127.0.0.1:18080`).
2. Starts a Wrangler Quick Tunnel with:
   ```shell
   wrangler tunnel quick-start http://127.0.0.1:18080 --log-level info
   ```
3. Parses the tunnel output to obtain a public HTTPS URL such as
   `https://<random>.trycloudflare.com`.
4. Appends `/webhook` to form the public webhook endpoint.
5. Sends a signed `ping` through that endpoint to verify it reaches Braid's
   ingress and passes signature verification.
6. Once reachable, updates the GitHub App's webhook URL to the public endpoint.
7. Runs the runtime normally.
8. On clean shutdown, restores the GitHub App's previous webhook URL.

## No Cloudflare account required

Wrangler Quick Tunnel uses Cloudflare's `trycloudflare.com` service, which is
free and does not require a Cloudflare account. You only need the `wrangler`
CLI installed, which `braid doctor` checks.

## No manual webhook URL

`braid setup` pre-fills the GitHub App Manifest with a placeholder webhook URL
because GitHub requires one, but the real public URL is set by
`braid serve --tunnel` at runtime. You do not need to copy a tunnel URL into
GitHub settings yourself.

## Multiple instances

Each instance has its own `serve` invocation and `BRAID_INSTANCE` value. You
can run:

```shell
BRAID_INSTANCE=inkcre braid serve --tunnel
```

in one terminal, and another instance in another terminal. Each gets its own
Quick Tunnel and its own GitHub App webhook URL. The `~/.braid/registry.toml`
maps instance keys to their home directories, so `--instance` (or
`BRAID_INSTANCE`) is the only selector you normally need.

## Troubleshooting

- Braid tries at most five tunnel candidates. Each candidate has up to 45
  seconds to publish its URL and register the connection. Braid then allows
  time for DNS propagation before probing the public endpoint. If all
  candidates fail, Braid reports the last error.
- `braid tunnel probe --instance <KEY> --url <public-url>` can verify a
  specific URL manually, but normal `serve --tunnel` does this automatically.
