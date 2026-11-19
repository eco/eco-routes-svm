#!/usr/bin/env bash
# One step of .github/workflows/{deploy,finalize,close}.yml:
# deploy-step.sh <check|hash <plan-log>|gate <plan-log>|deploy|idl|verify|finalize|close|close-buffers|redact>.
# Reads CLUSTER, RPC_URL, ASSETS (release download dir, holds program-ids.json), PLAN (plan.json)
# and KEYPAIR (deployer keypair file) from the environment; KEYS_DIR (default target/keys) holds
# the derived program keypairs. Never prints key material.
# `redact` filters stdin: everything bound for the uploaded artifact goes through it, since RPC
# client errors echo the request URL (API key) and masking does not cover artifact files.
set -euo pipefail
set +x

readonly REPOSITORY_URL="https://github.com/eco/eco-routes-svm"
# Each attempt re-signs the writes still unconfirmed when the blockhash expires (about a minute).
readonly MAX_SIGN_ATTEMPTS=20

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
  local selected
  if [ "${PROGRAMS:-all}" = all ]; then
    selected=$(jq -r 'keys[]' "${ASSETS}/program-ids.json")
  else
    selected=$(printf '%s\n' "$PROGRAMS" | tr ',' '\n')
  fi
  printf '%s\n' "$selected" | while read -r program; do
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

# Legacy full releases reclaim every deployer-owned buffer. Selected runs touch only fresh,
# explicit buffers created by this run; a successful upload already consumed its buffer.
legacy_full_deploy() {
  [ ! -f "${ASSETS}/deployment-policy.json" ] &&
    [ "$(jq -er '.selected | length' "$PLAN")" = "$(jq -er 'length' "${ASSETS}/program-ids.json")" ]
}

close_buffers() {
  if legacy_full_deploy; then
    solana program close --buffers -u "$RPC_URL" -k "$KEYPAIR"
    return
  fi
  [ -d "${BUFFER_DIR:?BUFFER_DIR must name the current run buffer directory}" ] || return 0
  local buffer_keypair address output
  shopt -s nullglob
  for buffer_keypair in "$BUFFER_DIR"/*-buffer-keypair.json; do
    address=$(solana-keygen pubkey "$buffer_keypair")
    if ! output=$(solana program show "$address" -u "$RPC_URL" --output json 2>&1); then
      if grep -qF "Unable to find the account ${address}" <<<"$output"; then
        continue
      fi
      printf '%s\n' "$output" >&2
      return 1
    fi
    solana program close "$address" -u "$RPC_URL" -k "$KEYPAIR" --authority "$KEYPAIR"
  done
}

deploy() {
  close_buffers
  # The reviewed plan fixes the price; `apply` reads it from there too.
  local price price_flags=() buffer_flags=()
  price=$(jq -er '.inputs.compute_unit_price' "$PLAN")
  if [ "$price" != 0 ]; then
    price_flags=(--with-compute-unit-price "$price")
  fi
  # The plan lists only programs absent from the chain when it was made, and runs on a cluster
  # never overlap, so nothing here can already be deployed. Writes go through the RPC: validator
  # TPUs drop much of what a CI runner sends, and each drop is a re-sign.
  deployer actions --plan "$PLAN" --action deploy | while read -r program; do
    if ! legacy_full_deploy; then
      mkdir -p "${BUFFER_DIR:?BUFFER_DIR must name the current run buffer directory}"
      local buffer="$BUFFER_DIR/${program}-buffer-keypair.json"
      # Never reuse a key: this directory belongs to this workflow run and attempt.
      test ! -e "$buffer" || { echo "buffer key already exists: ${buffer}" >&2; return 1; }
      (umask 077; solana-keygen new --no-bip39-passphrase --silent --outfile "$buffer" >/dev/null)
      local buffer_address
      buffer_address=$(solana-keygen pubkey "$buffer")
      printf '%s %s\n' "$program" "$buffer_address" >> "$(dirname "$PLAN")/buffers.txt"
      buffer_flags=(--buffer "$buffer")
    fi
    echo "deploying ${program} $(program_address "$program")"
    solana program deploy -u "$RPC_URL" -k "$KEYPAIR" --upgrade-authority "$KEYPAIR" \
      --program-id "${KEYS_DIR:-target/keys}/${program}-keypair.json" \
      ${buffer_flags[@]+"${buffer_flags[@]}"} \
      --use-rpc --max-sign-attempts "$MAX_SIGN_ATTEMPTS" \
      ${price_flags[@]+"${price_flags[@]}"} "${ASSETS}/${program}.${CLUSTER}.so"
  done
}

# Each upgradeable program's release IDL becomes its canonical Program Metadata account, which
# only its upgrade authority (the deployer) can write and nobody can once it is final. The
# deployer skips an IDL already on chain, resumes a half-written one, and reads every upload
# back; final programs are reported and left alone.
idl() {
  deployer idl --plan "$PLAN" --assets "$ASSETS" --deployer-keypair "$KEYPAIR" || {
    echo "IDL upload failed; nothing is lost: rerun the deploy workflow, whose IDL step resumes from the chain, before finalizing" >&2
    return 1
  }
}

verify() {
  local commit uploader features=()
  commit=$(git rev-parse HEAD)
  uploader=$(solana-keygen pubkey "$KEYPAIR")
  if [ "$CLUSTER" = mainnet ]; then
    features=(-- --features mainnet)
  fi
  deployer actions --plan "$PLAN" --action verify | while read -r program; do
    local address output
    address=$(program_address "$program")
    echo "verifying ${program} ${address} at ${commit}"
    # On a hash mismatch solana-verify uploads no record but still exits 0.
    output=$(solana-verify verify-from-repo -u "$RPC_URL" --program-id "$address" "$REPOSITORY_URL" \
      --commit-hash "$commit" --library-name "$program" --base-image "$VERIFY_IMAGE" \
      -k "$KEYPAIR" -y ${features[@]+"${features[@]}"} </dev/null)
    printf '%s\n' "$output"
    grep -qF 'Program hash matches' <<<"$output" || {
      echo "${program}: the build from ${commit} does not match the deployed bytecode; no verification record was uploaded" >&2
      return 1
    }
    if [ "$CLUSTER" = mainnet ]; then
      solana-verify remote submit-job -u "$RPC_URL" --program-id "$address" --uploader "$uploader"
    fi
  done
}

# The plan job's hash, for the approved job to compare against.
hash() {
  local log=$1 computed
  computed=$(sed -n 's/^plan_hash=//p' "$log" | tail -n 1)
  [ -n "$computed" ] || { echo "no plan_hash in ${log}" >&2; return 1; }
  echo "hash=${computed}" >> "$GITHUB_OUTPUT"
  echo "Approve the next job to execute exactly this plan (\`${computed}\`), or reject it to stop with nothing written." >> "$GITHUB_STEP_SUMMARY"
}

# The approved job re-plans; it may write only if that plan is the one the reviewer approved.
gate() {
  local log=$1 computed
  computed=$(sed -n 's/^plan_hash=//p' "$log" | tail -n 1)
  [ -n "$computed" ] || { echo "no plan_hash in ${log}" >&2; return 1; }
  [ -n "${PLAN_HASH:-}" ] || { echo "no approved plan hash" >&2; return 1; }
  if [ "$PLAN_HASH" != "$computed" ]; then
    echo "the chain or inputs changed since the plan was approved (approved ${PLAN_HASH}, now ${computed}); dispatch again" >&2
    return 1
  fi
  echo "plan ${computed} is the approved one"
}

finalize() {
  deployer actions --plan "$PLAN" --action finalize | while read -r program; do
    local address
    address=$(program_address "$program")
    echo "finalizing ${program} ${address}"
    solana program set-upgrade-authority -u "$RPC_URL" -k "$KEYPAIR" "$address" --final
  done
}

# Burns each address for good; its rent returns to the deployer. Each program's IDL accounts
# close first: only its upgrade authority can close them, and only while the program exists.
close() {
  local recipient
  recipient=$(solana-keygen pubkey "$KEYPAIR")
  deployer close-idl --plan "$PLAN" --assets "$ASSETS" --deployer-keypair "$KEYPAIR"
  deployer actions --plan "$PLAN" --action close | while read -r program; do
    local address
    address=$(program_address "$program")
    echo "closing ${program} ${address}"
    solana program close -u "$RPC_URL" -k "$KEYPAIR" --authority "$KEYPAIR" \
      --recipient "$recipient" --bypass-warning "$address"
  done
}

case "${1:-}" in
  check | deploy | idl | verify | finalize | close | redact) "$1" ;;
  hash | gate) "$1" "${2:?usage: $0 $1 <plan-log>}" ;;
  close-buffers) close_buffers ;;
  *) echo "usage: $0 <check|hash <plan-log>|gate <plan-log>|deploy|idl|verify|finalize|close|close-buffers|redact>" >&2; exit 2 ;;
esac
