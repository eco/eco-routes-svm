#!/usr/bin/env bash
# One step of .github/workflows/deploy.yml:
# deploy-step.sh <check|deploy|verify|finalize|close-buffers|redact>.
# Reads CLUSTER, RPC_URL, ASSETS (release download dir, holds program-ids.json), PLAN (plan.json)
# and KEYPAIR (deployer keypair file) from the environment; KEYS_DIR (default target/keys) holds
# the derived program keypairs. Never prints key material.
# `redact` filters stdin: everything bound for the uploaded artifact goes through it, since RPC
# client errors echo the request URL (API key) and masking does not cover artifact files.
set -euo pipefail
set +x

readonly REPOSITORY_URL="https://github.com/eco/eco-routes-svm"

redact() {
  local line
  while IFS= read -r line || [ -n "$line" ]; do
    if [ -n "${RPC_URL:-}" ]; then
      line=${line//"$RPC_URL"/<redacted-url>}
    fi
    printf '%s\n' "$line"
  done | sed -E "s#(https?|wss?)://[^[:space:]()\\\\\"'<>]*#<redacted-url>#g"
}

program_address() {
  jq -er --arg program "$1" '.[$program].address' "${ASSETS}/program-ids.json"
}

check() {
  jq -r 'keys[]' "${ASSETS}/program-ids.json" | while read -r program; do
    local address lib idl
    address=$(program_address "$program")
    lib="programs/${program//_/-}/src/lib.rs"
    idl="${ASSETS}/${program}.${CLUSTER}.json"
    grep -qF "declare_id!(\"${address}\")" "$lib" || { echo "${lib}: declare_id! is not ${address}" >&2; return 1; }
    test "$(jq -r .address "$idl")" = "$address" || { echo "${idl}: address is not ${address}" >&2; return 1; }
    test -f "${ASSETS}/${program}.${CLUSTER}.so" || { echo "missing ${program}.${CLUSTER}.so" >&2; return 1; }
    echo "ok ${program} ${address}"
  done
}

# A failed `solana program deploy` strands its buffer and the rent in it; only the deployer key,
# which exists only inside this workflow, can reclaim it. The key writes buffers nowhere else and
# runs never overlap on a cluster, so every buffer it holds is a leftover.
close_buffers() {
  solana program close --buffers -u "$RPC_URL" -k "$KEYPAIR"
}

deploy() {
  close_buffers
  local price_flags=()
  if [ "${COMPUTE_UNIT_PRICE:-0}" != 0 ]; then
    price_flags=(--with-compute-unit-price "$COMPUTE_UNIT_PRICE")
  fi
  deployer actions --plan "$PLAN" --action deploy | while read -r program; do
    local address
    address=$(program_address "$program")
    if solana program show -u "$RPC_URL" "$address" > /dev/null 2>&1; then
      echo "${program} ${address} already on chain, skipping"
      continue
    fi
    solana program deploy -u "$RPC_URL" -k "$KEYPAIR" --upgrade-authority "$KEYPAIR" \
      --program-id "${KEYS_DIR:-target/keys}/${program}-keypair.json" \
      ${price_flags[@]+"${price_flags[@]}"} "${ASSETS}/${program}.${CLUSTER}.so"
  done
}

verify() {
  local commit uploader features=()
  commit=$(git rev-parse HEAD)
  uploader=$(solana-keygen pubkey "$KEYPAIR")
  if [ "$CLUSTER" = mainnet ]; then
    features=(-- --features mainnet)
  fi
  deployer actions --plan "$PLAN" --action verify | while read -r program; do
    local address
    address=$(program_address "$program")
    echo "verifying ${program} ${address} at ${commit}"
    solana-verify verify-from-repo -u "$RPC_URL" --program-id "$address" "$REPOSITORY_URL" \
      --commit-hash "$commit" --library-name "$program" --base-image "$VERIFY_IMAGE" \
      -k "$KEYPAIR" -y ${features[@]+"${features[@]}"}
    if [ "$CLUSTER" = mainnet ]; then
      solana-verify remote submit-job -u "$RPC_URL" --program-id "$address" --uploader "$uploader"
    fi
  done
}

finalize() {
  deployer actions --plan "$PLAN" --action finalize | while read -r program; do
    local address
    address=$(program_address "$program")
    echo "finalizing ${program} ${address}"
    solana program set-upgrade-authority -u "$RPC_URL" -k "$KEYPAIR" "$address" --final
  done
}

case "${1:-}" in
  check | deploy | verify | finalize | redact) "$1" ;;
  close-buffers) close_buffers ;;
  *) echo "usage: $0 <check|deploy|verify|finalize|close-buffers|redact>" >&2; exit 2 ;;
esac
