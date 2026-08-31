# rust-code

Agent Skill for production Rust applications and libraries.

It separates two kinds of engineering standards:

1. semantic rules in `SKILL.md` and `references/` for ownership, APIs, errors, concurrency, unsafe, and operational behavior;
2. a deliberately small `scripts/audit.py` for simple source patterns, while Cargo/rustc/Clippy remain authoritative.

This follows the same general philosophy as CMGS/asl: automate only rules that can be checked with acceptable noise; retain exception-heavy architecture and design rules in a semantic walkthrough.

## Install

Copy the entire `rust-code/` directory into the skills directory used by your Agent Skills-compatible client. Keep the directory name unchanged so it matches the manifest `name`.

## Scope

Cargo-managed Rust applications/libraries. The skill preserves the repository's edition, MSRV, async runtime, feature matrix, dependencies, and CI conventions.

## Optional audit

```bash
python scripts/audit.py src
```

Use `--strict` only after reviewing/adopting advisory warnings.
