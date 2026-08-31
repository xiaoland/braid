#!/usr/bin/env python3
"""Small, dependency-free source audit for production Rust.

The audit intentionally avoids pretending to parse Rust fully. Compiler and Clippy
remain authoritative. Warnings become failures only with --strict.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import sys
from typing import Iterable

IGNORED_DIRS = {".git", "target", "vendor", "node_modules"}
TEST_DIRS = {"tests", "benches", "examples"}


@dataclass(frozen=True)
class Finding:
    severity: str
    path: Path
    line: int
    code: str
    message: str


def iter_rs_files(paths: Iterable[Path], include_tests: bool) -> Iterable[Path]:
    seen: set[Path] = set()
    for path in paths:
        if path.is_file() and path.suffix == ".rs":
            if not include_tests and any(part in TEST_DIRS for part in path.parts):
                continue
            resolved = path.resolve()
            if resolved not in seen:
                seen.add(resolved)
                yield path
        elif path.is_dir():
            for child in path.rglob("*.rs"):
                if any(part in IGNORED_DIRS for part in child.parts):
                    continue
                if not include_tests and any(part in TEST_DIRS for part in child.parts):
                    continue
                resolved = child.resolve()
                if resolved not in seen:
                    seen.add(resolved)
                    yield child


def has_safety_comment(lines: list[str], index: int) -> bool:
    for previous in range(index - 1, max(-1, index - 5), -1):
        text = lines[previous].strip()
        if not text:
            continue
        if "SAFETY:" in text.upper():
            return True
        if not text.startswith(("//", "///", "#[")):
            break
    return False


def audit(path: Path) -> list[Finding]:
    try:
        source = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        return [Finding("ERROR", path, 1, "IO001", f"cannot read file: {exc}")]

    lines = source.splitlines()
    findings: list[Finding] = []

    async_depth: int | None = None
    pending_async = False
    brace_depth = 0

    for idx, line in enumerate(lines):
        line_no = idx + 1
        stripped = line.strip()

        # Track a simple async-fn lexical region. This is intentionally conservative.
        if async_depth is None and not pending_async and re.search(r"\b(?:unsafe\s+)?async\s+fn\b|\basync\s+unsafe\s+fn\b", line):
            pending_async = True

        if pending_async and "{" in line:
            async_depth = brace_depth + line.count("{") - line.count("}")
            pending_async = False

        if re.search(r"\bstd::thread::sleep\s*\(", line) and async_depth is not None:
            findings.append(Finding("ERROR", path, line_no, "ASYNC001", "std::thread::sleep inside an async function blocks the executor; use the runtime timer"))

        if re.search(r"\b(?:todo|unimplemented)!\s*\(", line):
            findings.append(Finding("WARN", path, line_no, "DEBT001", "todo!/unimplemented! remains in scanned production source; verify the path cannot execute or finish the implementation"))

        if re.search(r"\bdbg!\s*\(", line):
            findings.append(Finding("WARN", path, line_no, "DBG001", "dbg! macro left in source"))

        if re.search(r"\.unwrap\s*\(\s*\)", line):
            findings.append(Finding("WARN", path, line_no, "ERR001", "unwrap() in production source; verify failure is a documented programmer invariant rather than a recoverable error"))

        if re.search(r"#\s*!?\s*\[\s*allow\s*\(\s*(?:warnings|clippy::all|clippy::restriction)\s*\)\s*\]", line):
            findings.append(Finding("WARN", path, line_no, "LINT001", "broad lint suppression; prefer narrow justified allows and do not enable/suppress the whole restriction group"))

        if re.search(r"\bunsafe\s*\{", line) and not has_safety_comment(lines, idx):
            findings.append(Finding("WARN", path, line_no, "UNSAFE001", "unsafe block has no nearby SAFETY explanation; document why its preconditions hold"))

        if re.search(r"\bpub\s+unsafe\s+fn\b", line):
            window_start = max(0, idx - 12)
            docs = "\n".join(lines[window_start:idx]).lower()
            if "# safety" not in docs:
                findings.append(Finding("WARN", path, line_no, "UNSAFE002", "public unsafe fn has no nearby '# Safety' documentation section"))

        brace_depth += line.count("{") - line.count("}")
        if async_depth is not None and brace_depth < async_depth:
            async_depth = None

    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="+", type=Path, help="Rust files or directories to scan")
    parser.add_argument("--strict", action="store_true", help="treat WARN findings as failures")
    parser.add_argument("--include-tests", action="store_true", help="also scan tests/examples/benches directories")
    args = parser.parse_args()

    findings: list[Finding] = []
    for path in iter_rs_files(args.paths, args.include_tests):
        findings.extend(audit(path))

    findings.sort(key=lambda f: (str(f.path), f.line, f.code))
    for finding in findings:
        print(f"{finding.path}:{finding.line}: {finding.severity} {finding.code} {finding.message}")

    errors = sum(f.severity == "ERROR" for f in findings)
    warnings = sum(f.severity == "WARN" for f in findings)
    print(f"audit: {errors} error(s), {warnings} warning(s)", file=sys.stderr)
    return 1 if errors or (args.strict and warnings) else 0


if __name__ == "__main__":
    raise SystemExit(main())
