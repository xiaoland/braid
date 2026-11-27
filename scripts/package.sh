#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target=${BRAID_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}
output_root=${BRAID_DIST_DIR:-"$repository_root/dist"}

cd "$repository_root"
cargo build --release --locked --target "$target"

target_directory=${CARGO_TARGET_DIR:-"$repository_root/target"}
binary="$target_directory/$target/release/braid"
version=$("$binary" --version | awk '{print $2}')
if [ -n "${BRAID_RELEASE_TAG:-}" ] && [ "$BRAID_RELEASE_TAG" != "v$version" ]; then
  printf 'Release tag %s does not match binary version v%s\n' "$BRAID_RELEASE_TAG" "$version" >&2
  exit 1
fi
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
