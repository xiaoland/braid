#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target=${BRAID_BUILD_TARGET:-aarch64-apple-darwin}
output_root=${BRAID_DIST_DIR:-"$repository_root/dist"}

cd "$repository_root"
cargo build --release --locked --target "$target"

binary="$repository_root/target/$target/release/braid"
version=$("$binary" --version | awk '{print $2}')
artifact="braid-v${version}-${target}"
archive="$output_root/$artifact.tar.gz"
mkdir -p "$output_root"
stage_root=$(mktemp -d "$output_root/.braid-package.XXXXXX")
trap 'rm -rf "$stage_root"' EXIT HUP INT TERM
stage="$stage_root/$artifact"

mkdir -p "$stage/bin" "$stage/deploy"
cp "$binary" "$stage/bin/braid"
cp LICENSE README.md CHANGELOG.md config.example.toml "$stage/"
cp deploy/otel-collector.example.yaml "$stage/deploy/"

tar -C "$stage_root" -czf "$archive" "$artifact"
(cd "$output_root" && shasum -a 256 "$(basename "$archive")" > "$(basename "$archive").sha256")

printf '%s\n' "$archive"
