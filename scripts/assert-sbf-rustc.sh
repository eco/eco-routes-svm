#!/usr/bin/env bash
#
# Assert which rustc compiled the on-chain programs.
#
# `anchor build` does not use the `cargo-build-sbf` default toolchain. It resolves its own
# platform-tools, downloads them under ~/.cache/solana, and links them as the `solana` rustup
# toolchain. That resolution depends on the installed Solana CLI and on whatever is already
# cached, so a cache eviction or an unpinned installer can silently change the compiler that
# produces deployed bytecode -- with a green build either way.
#
# This makes that change loud instead of silent. Run it AFTER a build has run, since the
# `solana` toolchain link is created during the build.
#
# Usage: scripts/assert-sbf-rustc.sh <expected-version>   e.g. 1.79.0-dev

set -euo pipefail

expected="${1:?usage: assert-sbf-rustc.sh <expected-version>}"

if ! rustup toolchain list | grep -q '^solana'; then
    echo "ERROR: no 'solana' rustup toolchain found." >&2
    echo "This script must run after a step that builds the on-chain programs" >&2
    echo "(anchor build / cargo build-sbf), which is what creates the link." >&2
    exit 1
fi

# e.g. "rustc 1.79.0-dev" -> "1.79.0-dev"
actual="$(rustup run solana rustc --version | awk '{print $2}')"

echo "on-chain (SBF) rustc: ${actual}"

if [[ "${actual}" != "${expected}" ]]; then
    cat >&2 <<EOF

ERROR: the on-chain compiler changed.

  expected: ${expected}
  actual:   ${actual}

The rustc that compiles the deployed programs is not the one this repo pins. That changes
the bytecode toolchain, so it must be a deliberate, reviewed decision -- not a side effect
of a cache miss or an installer serving a new default.

If the change is intended, update SBF_RUSTC_VERSION in .github/workflows/ and say why in
the PR. If it is not, check SOLANA_VERSION and the Anchor version.
EOF
    exit 1
fi
