#!/bin/sh
# Check a native release archive as its consumer, away from the checkout.
set -eu

if [ "$#" -ne 3 ]; then
  echo 'Usage: verify-package.sh DIST_DIRECTORY RELEASE_TAG RUST_TARGET' >&2
  exit 2
fi
dist=$(CDPATH= cd -- "$1" && pwd)
artifact="braid-$2-$3"
archive="$artifact.tar.gz"
cd "$dist"
if ! verified=$(shasum -a 256 -c "$archive.sha256"); then
  printf 'SHA-256 verification failed for %s: %s\n' "$archive" "$verified" >&2
  exit 1
fi
if [ "$verified" != "$archive: OK" ]; then
  printf 'Checksum must identify %s (got %s)\n' "$archive" "$verified" >&2
  exit 1
fi
install_root=$(mktemp -d)
trap 'rm -rf "$install_root"' EXIT
tar -xzf "$archive" -C "$install_root"
cd "$install_root/$artifact"
for entry in LICENSE README.md CHANGELOG.md deploy/otel-collector.example.yaml; do
  if [ ! -s "$entry" ]; then
    printf 'Required release file is missing or empty: %s\n' "$entry" >&2
    exit 1
  fi
done
# The packaged executable must run without a checkout, Rust toolchain or user config.
braid() { env -i HOME="$install_root" PATH=/usr/bin:/bin ./bin/braid "$@"; }
version=$(braid --version)
if [ "$version" != "braid ${2#v}" ]; then
  printf 'Expected braid %s, installed binary reported %s\n' "${2#v}" "$version" >&2
  exit 1
fi
config="$PWD/config.example.toml"
braid config check --config "$config"
applied=$(braid migrate apply --config "$config")
printf '%s\n' "$applied"
if ! printf '%s\n' "$applied" | grep -Eq '^schema: 0 -> [1-9][0-9]*$'; then
  echo 'Clean installation must migrate a fresh database from schema 0' >&2
  exit 1
fi
plan=$(braid migrate plan --config "$config")
printf '%s\n' "$plan" | grep -Fx 'pending: none'
