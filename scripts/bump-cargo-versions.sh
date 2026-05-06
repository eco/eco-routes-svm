#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-}"

if [[ -z "$VERSION" ]]; then
    echo "Usage: $0 <version>" >&2
    exit 1
fi

released_crates=(
    programs/flash-fulfiller/Cargo.toml
    programs/hyper-prover/Cargo.toml
    programs/local-prover/Cargo.toml
    programs/portal/Cargo.toml
    programs/proof-helper/Cargo.toml
    packages/eco-svm-std/Cargo.toml
)

for manifest in "${released_crates[@]}"; do
    awk -v ver="$VERSION" '
        /^\[/ { in_package = ($0 == "[package]") }
        in_package && /^version = / { sub(/".*"/, "\"" ver "\"") }
        { print }
    ' "$manifest" > "$manifest.tmp" && mv "$manifest.tmp" "$manifest"
done

cargo update --workspace
