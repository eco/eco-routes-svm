#!/usr/bin/env bash
#
# Builds released programs for both clusters with `solana-verify build` in the pinned image,
# the build that `solana-verify verify-from-repo --base-image "$VERIFY_IMAGE"` reproduces.
# Writes <out-dir>/<cluster>/<program>.so.
#
# Usage: VERIFY_IMAGE=<image@sha256:...> scripts/verifiable-build.sh <out-dir> <program>...
set -euo pipefail

: "${VERIFY_IMAGE:?VERIFY_IMAGE must be set}"
out_dir=${1:?usage: verifiable-build.sh <out-dir> <program>...}
shift
[ "$#" -gt 0 ] || { echo "usage: verifiable-build.sh <out-dir> <program>..." >&2; exit 1; }

for cluster in mainnet devnet; do
  features=()
  if [ "$cluster" = mainnet ]; then
    features=(-- --features mainnet)
  fi
  mkdir -p "${out_dir}/${cluster}"
  for program in "$@"; do
    solana-verify build --base-image "$VERIFY_IMAGE" --library-name "$program" ${features[@]+"${features[@]}"}
    cp "target/deploy/${program}.so" "${out_dir}/${cluster}/${program}.so"
  done
done
