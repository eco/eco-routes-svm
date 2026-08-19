#!/usr/bin/env bash
#
# Assert which rustc compiled the on-chain programs.
#
# Kept even though anchor now hard-codes its tools-version: that constant lives in a dependency's
# source, so nothing in this repo -- not Anchor.toml, not the lockfile -- moves when it changes.
# This is what makes "which compiler produced the deployed bytecode" an enforced fact rather than
# folklore, and what turns an ANCHOR_VERSION bump's bytecode consequence into a visible one.
# Limit: -dev builds report no commit hash, so a platform-tools respin keeping the same rustc base
# would pass. It is a fingerprint, not a checksum.
#
# `anchor build` does not use the platform-tools the installed Solana CLI defaults to. anchor-cli
# 1.x shells out to `cargo build-sbf --tools-version <v>` with <v> HARD-CODED in the CLI source
# (`BUILD_SUBCOMMAND` in cli/src/lib.rs), downloading it to
# ~/.cache/solana/<tools-version>/platform-tools/rust/bin/rustc. So the compiler that produces
# deployed bytecode tracks ANCHOR_VERSION, not SOLANA_VERSION -- bumping Anchor can silently
# change it, with a green build either way.
#
# This makes that change loud instead of silent. Run it AFTER a build has run, since that is what
# populates the cache directory this script reads.
#
# Both arguments are required and are checked together: the tools-version pins *which* compiler
# anchor fetched, and the rustc version pins *what* that compiler is. Checking only the latter
# would pass on a machine that happens to have some other matching toolchain cached from an
# earlier build.
#
# Anchor before 1.x additionally linked the resolved compiler as a `solana` rustup toolchain;
# 1.1.2 does not, so this reads the cache directly rather than going through rustup.
#
# Usage: scripts/assert-sbf-rustc.sh <expected-rustc> <expected-tools-version>
#   e.g. scripts/assert-sbf-rustc.sh 1.89.0-dev v1.52

set -euo pipefail

expected_rustc="${1:?usage: assert-sbf-rustc.sh <expected-rustc> <expected-tools-version>}"
expected_tools="${2:?usage: assert-sbf-rustc.sh <expected-rustc> <expected-tools-version>}"

rustc_bin="${HOME}/.cache/solana/${expected_tools}/platform-tools/rust/bin/rustc"

if [[ ! -x "${rustc_bin}" ]]; then
    echo "ERROR: no platform-tools rustc at ${rustc_bin}" >&2
    echo >&2
    echo "Either the build did not run yet (this script must follow anchor build /" >&2
    echo "cargo build-sbf), or anchor fetched a different tools-version than expected." >&2
    echo >&2
    echo "Cached tools-versions found:" >&2
    found=0
    while IFS= read -r candidate; do
        version="$("${candidate}" --version | awk '{print $2}')"
        tools="$(basename "$(dirname "$(dirname "$(dirname "$(dirname "${candidate}")")")")")"
        echo "  ${tools}  ->  rustc ${version}" >&2
        found=1
    done < <(find "${HOME}/.cache/solana" -type f -path '*/platform-tools/rust/bin/rustc' 2>/dev/null | sort)
    [[ ${found} -eq 0 ]] && echo "  (none)" >&2
    echo >&2
    echo "anchor-cli hard-codes its tools-version; if it changed, update SBF_TOOLS_VERSION" >&2
    echo "and SBF_RUSTC_VERSION in .github/workflows/ together and say why in the PR." >&2
    exit 1
fi

# e.g. "rustc 1.89.0-dev" -> "1.89.0-dev"
actual_rustc="$("${rustc_bin}" --version | awk '{print $2}')"

if [[ "${actual_rustc}" != "${expected_rustc}" ]]; then
    cat >&2 <<EOF

ERROR: the on-chain compiler changed.

  tools-version: ${expected_tools}
  expected rustc: ${expected_rustc}
  actual rustc:   ${actual_rustc}

The rustc that compiles the deployed programs is not the one this repo pins. That changes
the bytecode toolchain, so it must be a deliberate, reviewed decision -- not a side effect
of a cache miss or an upstream platform-tools respin.

If the change is intended, update SBF_RUSTC_VERSION in .github/workflows/ and say why in
the PR. If it is not, check ANCHOR_VERSION -- anchor's hard-coded tools-version is what
selects this compiler.
EOF
    exit 1
fi

echo "on-chain (SBF) rustc: ${actual_rustc} (platform-tools ${expected_tools})"
