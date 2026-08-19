#!/usr/bin/env bash
#
# Assert which rustc compiled the on-chain programs.
#
# `anchor build` does not use the `cargo-build-sbf` default toolchain. It resolves its own
# platform-tools and downloads them under ~/.cache/solana/<version>/platform-tools/rust/bin/rustc.
# That resolution depends on the installed Solana CLI and on whatever is already cached, so a
# cache eviction or an unpinned installer can silently change the compiler that produces deployed
# bytecode -- with a green build either way.
#
# This makes that change loud instead of silent. Run it AFTER a build has run, since that is what
# populates the cache directory this script reads.
#
# Anchor versions before 1.x additionally linked the resolved compiler as a `solana` rustup
# toolchain; 1.1.2 no longer does, so this reads the cache directory directly instead of going
# through rustup.
#
# Usage: scripts/assert-sbf-rustc.sh <expected-version>   e.g. 1.79.0-dev

set -euo pipefail

expected="${1:?usage: assert-sbf-rustc.sh <expected-version>}"
cache_dir="${HOME}/.cache/solana"

candidates=()
while IFS= read -r rustc; do
    candidates+=("${rustc}")
done < <(find "${cache_dir}" -type f -path '*/platform-tools/rust/bin/rustc' 2>/dev/null | sort)

if [[ ${#candidates[@]} -eq 0 ]]; then
    echo "ERROR: no platform-tools rustc found under ${cache_dir}." >&2
    echo "This script must run after a step that builds the on-chain programs" >&2
    echo "(anchor build / cargo build-sbf), which is what populates the cache." >&2
    exit 1
fi

# e.g. "rustc 1.79.0-dev" -> "1.79.0-dev"
versions=()
for rustc in "${candidates[@]}"; do
    versions+=("$("${rustc}" --version | awk '{print $2}')")
done

for version in "${versions[@]}"; do
    if [[ "${version}" == "${expected}" ]]; then
        echo "on-chain (SBF) rustc: ${version}"
        exit 0
    fi
done

echo "on-chain (SBF) rustc candidates found, none matched:" >&2
for i in "${!candidates[@]}"; do
    echo "  ${versions[i]}  (${candidates[i]})" >&2
done

cat >&2 <<EOF

ERROR: the on-chain compiler changed.

  expected: ${expected}
  found:    ${versions[*]}

The rustc that compiles the deployed programs is not the one this repo pins. That changes
the bytecode toolchain, so it must be a deliberate, reviewed decision -- not a side effect
of a cache miss or an installer serving a new default.

If the change is intended, update SBF_RUSTC_VERSION in .github/workflows/ and say why in
the PR. If it is not, check SOLANA_VERSION and the Anchor version.
EOF
exit 1
