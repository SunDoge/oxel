#!/usr/bin/env bash
set -euo pipefail

output_dir=${1:-target/preview}
mkdir -p -- "$output_dir"
output_dir=$(cd -- "$output_dir" && pwd)
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
repo=$PWD
target=x86_64-unknown-linux-gnu

host=$(rustc -vV | awk '/^host: / { print $2 }')
if [[ "$host" != "$target" ]]; then
    printf 'The CLI bundle requires a %s Rust host. Found %s.\n' "$target" "$host" >&2
    exit 1
fi

metadata=$(cargo metadata --locked --no-deps --format-version 1)
version=$(jq -er '.packages[] | select(.name == "oxel-cli") | .version' <<< "$metadata")
target_directory=$(jq -er '.target_directory' <<< "$metadata")
cargo build --release --locked -p oxel-cli --target "$target"

bundle="oxel-$version-$target"
stage=$(mktemp -d)
trap 'rm -r -- "$stage"' EXIT
mkdir "$stage/$bundle" "$stage/extracted" "$stage/smoke"
install -m 755 "$target_directory/$target/release/oxel" "$stage/$bundle/oxel"
install -m 644 LICENSE README.md CHANGELOG.md "$stage/$bundle/"
tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="$(git show -s --format=%cI HEAD)" -cf - "$bundle" |
    gzip -n > "$stage/$bundle.tar.gz"

cd -- "$stage"
sha256sum "$bundle.tar.gz" > SHA256SUMS
sha256sum -c SHA256SUMS
tar -xzf "$bundle.tar.gz" -C extracted
cli="$stage/extracted/$bundle/oxel"
cp "$repo/examples/hello.oxel" "$repo/examples/unit_tests.oxel" smoke/
cd smoke
test "$("$cli" --version)" = "oxel $version"
PATH="" "$cli" check hello.oxel
"$cli" fmt --check hello.oxel
test "$("$cli" run hello.oxel)" = 30
"$cli" test unit_tests.oxel

install -m 644 "$stage/$bundle.tar.gz" "$stage/SHA256SUMS" "$output_dir/"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf 'tag=v%s\nbundle=%s.tar.gz\n' "$version" "$bundle" >> "$GITHUB_OUTPUT"
fi
printf '%s/%s.tar.gz\n' "$output_dir" "$bundle"
