# Deployment recovery and inherited v3.1 receipt

Read after `SKILL.md` when a run fails, pauses or appears inconsistent. All inspection commands below read remote state or download records. They are not permission for a new deployment or verification submission.

## Reconcile before retry

1. Identify the exact run/attempt, workflow ref, release/deploy-tag SHAs, cluster, selection and approved plan hash. Download its redacted `deploy-v<version>-<cluster>` artifact and inspect each completed/failed step. Record absent artifacts rather than assuming cleanup succeeded.
2. Inspect chain state at confirmed slots: executable bytes/hash, programdata authority and deployment slot, transaction status, config contents, canonical IDL, verification record and remote job. An RPC timeout can follow a committed transaction. Check signatures and accounts before resending; never infer rollback from a failed client or red GitHub check.
3. Separate what landed from what remains. `deploy` can succeed before apply/IDL/verify fails. Apply can finish only some sequential transactions. Configs already equal are skipped; conflicting immutable config is not a retryable missing config.
4. If the complete retry write set remains authorized, fix the cause, dispatch again and review a **new** plan. Deploy is not step-selective resume: every `Partial` program gets verification again, including verification-record upload and mainnet remote submission, even when only IDL/setup work was missing. Obtain authorization for those repeated writes too. A failed attempt changes chain facts and invalidates the old hash. Do not reuse an old approval, edit the plan/hash, broaden selection or use direct CLI writes to dodge gates.
5. Report independent outcomes: chain deploy/init/readback, canonical IDL, local reproducible hash, uploaded verification record, remote verification status, workflow status and cleanup. Do not collapse them into one green/red result.

## IDL interruption

Keep exact release IDL bytes and selected plan. `deployer idl` reads the canonical Program Metadata account and scratch buffer before every transaction, writes differing chunks sequentially, then initializes or atomically replaces the whole canonical IDL. A failed upload can resume through a newly reviewed deploy run while authority remains. Do not close a differing canonical IDL or switch to parallel Anchor/metadata upload tooling. Finalization before IDL completion makes recovery impossible through that authority.

## Verification 502 or timeout

In `solana-verify` 0.5.2, `verify-from-repo` builds/checks bytecode and uploads verification parameters to the PDA; it does **not** submit a remote job (`--remote` is rejected as deprecated). The repository script subsequently calls `remote submit-job` on mainnet. That command prints `Verification request sent with request id` after acceptance and then polls. A polling HTTP 502 after that receipt means the job was accepted, not that submission failed. Do not repeat either write command blindly.

Extract the job ID and verification-parameter upload signature from existing logs. Check the job and program with the actual 0.5.2 subcommands:

```bash
# READ: JOB_ID and PROGRAM_ID identify the affected mainnet receipt.
solana-verify remote get-job --job-id "$JOB_ID"
solana-verify remote get-status --program-id "$PROGRAM_ID"
```

These two commands query the OtterSec API only in 0.5.2; they do not use RPC, so supply no credential URL. Do not confuse `get-status` with `get-job`: the latter requires `--job-id`, not a program address. If the service is unavailable, retain the job ID and mark remote status unknown; use later read-only checks, not a new submission as a probe.

Read the uploaded verification PDA and actual deployed executable hash without signing or uploading:

```bash
# READ: DEPLOYER is the receipt's public uploader key; RPC_URL is the correct cluster endpoint.
(
  set -euo pipefail
  set +x
  solana-verify get-program-pda --program-id "$PROGRAM_ID" --signer "$DEPLOYER" \
    -u "$RPC_URL" 2>&1 | scripts/deploy-step.sh redact
  solana-verify get-program-hash -u "$RPC_URL" "$PROGRAM_ID" \
    2>&1 | scripts/deploy-step.sh redact
)
```

Compare the PDA's uploader/program/commit/features/image against the release receipt and the rebuilt/on-chain hash. Exit 0 alone is insufficient: 0.5.2 can exit successfully on a hash mismatch without uploading a record, so require `Program hash matches`. A completed remote job must correspond to the intended bytes and program, not merely any old verified program status.

Only if reads establish missing/failed work, propose a retry with its **complete** write set under fresh authorization/review, including repeated verification writes and remote submission. A verified remote job plus completed deploy/IDL/cleanup needs a truthful incident record, not repeated chain writes to repair CI color.

## Buffers, keys and records

Selected runs use a new `BUFFER_DIR` per run/attempt. Before upload, `out/buffers.txt` records only program name and fresh buffer public address. Cleanup closes only those buffers; successfully consumed buffers no longer exist. An RPC error is not proof of account absence.

If cleanup failed, preserve the public buffer record and inspect those exact addresses/authorities. Closing a leftover buffer is a separately authorized write; follow the release-tag script and current Solana help. Never scan and close unrelated runs' buffers. Legacy full-manifest releases without policy use deployer-wide `--buffers` cleanup: review whether the payer is shared or another run wrote buffers before authorizing that path.

Delete temporary program, buffer and payer key files even when cleanup fails; public buffer addresses allow later authority-based cleanup without retaining their private keys. Never upload private keys, derivation secrets, RPC URLs or raw secret-bearing logs. Keep redacted plan, logs, public addresses, signatures, slots and asset hashes. Record unresolved rent separately from deleted local keys. Do not remove another operator's files or persistent custody key.

## Upgrade, rollback and drain limits

Inspect `packages/deployer/src/cli.rs`, `classify.rs`, planning code and `scripts/deploy-step.sh` at the **target deploy tag** before making capability claims. Baseline d22d794 has no general upgrade command. Changed bytecode at an existing address classifies as `Foreign`; deploy actions target `New`, not arbitrary in-place replacement. Retained upgrade authority means loader capability, not a supported automatic upgrade workflow.

Normal release changes derive new IDs and therefore new PDA namespaces; they do not migrate old accounts or drain old intents. A rollback is not `git revert` plus rerun: clients/solvers/provers must still service outstanding intents under old addresses. Obtain an explicit settlement/drain and routing plan before abandoning them. Closure cannot retrieve arbitrary escrow tokens/PDA rent, and final programs cannot be closed or upgraded.

The README documents a narrow historical exception: a temporary upgrade corrected an upgradeable LayerZero path, then restored **exact** release bytecode. It was outside plan/hash/approval flow. Do not generalize it or automate it; it needs explicit expert authorization, compatibility review, exact-byte restoration and readback. Finalized LayerZero needs a new program/aggregator rather than that exception. A pre-funded or burned address generally requires a reviewed salt increase and new release, including affected dependents; do not send lamports to a fresh program/programdata address as funding.

## v3.1.0: inherited operational evidence, not a fresh check

Use this receipt only for comparison. Re-read live state before new operations. None of the following is a test performed by this skill's installation.

| Item | Inherited receipt |
|---|---|
| Release | [`v3.1.0`, d22d794c2ced17ba790bdad8a04f3fba7d943086](https://github.com/eco/eco-routes-svm/commit/d22d794c2ced17ba790bdad8a04f3fba7d943086) |
| Deploy source | [`deploy/v3.1.0`, c2946ab2749a4ecb82ecdd857b78ac8de9715fdc](https://github.com/eco/eco-routes-svm/commit/c2946ab2749a4ecb82ecdd857b78ac8de9715fdc), direct child with only ten `declare_id!` substitutions |
| Selection/cluster | `intent_chainer`, mainnet |
| Program | `EcoSEcjWKSEG5NqLW3mpzhC3kP4xWNkUEaywJihB5E84` |
| Existing Portal | `EcoN71qfZMZtxwqW1Ps1kzbNKyfpCEd1izZhh58PqsDb`; chainer uses committed `Order.portal`, not a hardcoded Portal ID |
| Deployment | [run 40784725791](https://github.com/eco/eco-routes-svm/actions/runs/40784725791) |
| Plan hash | `8705991150fa42dd87544fb753b1d9cec170e6ae8ac518b373ddfb5242558f70` |
| Deployment slot | `462660101` |
| Retained authority | `Cx92mzUNNr7mazZmuB5uTZtaYPNDFs6RwKVYF2hC7kH3` |
| Finality instruction | **NOT FINALIZED**; wait for test-team approval and separate user authorization |
| Completed steps | Deployment, apply, IDL and selected key derivation passed; key/artifact cleanup passed |
| Rebuilt/on-chain hash | `3de806b654e052b482cd41bfe3c456ac48d1d9da785aecf0e10dbf8d3e969ffb` matched |
| Verification parameter upload | `3fFzXpJadS11kpEuZV8hFynXf9f2Xwmc6cxkmuj4aoe9YzmqW96gWPiyAGTnmWxKmj9WqUNaRBhQkMnK2Jdo5DGp` |
| Remote job | [acfb21e7-f688-40c6-b2d7-def82d91e361](https://verify.osec.io/job/acfb21e7-f688-40c6-b2d7-def82d91e361) |
| CI versus remote | Polling HTTP 502 made GitHub red; remote job later reported `Completed` + `Verified` at `2026-10-28T23:28:24.040638` |
| Balance observation | `4.866673050 → 2.872844910 SOL`, delta `1.993828140 SOL`; not transaction-reconciled, not a universal fee |

The release was manually assembled as a **mixed** 24-asset release, versus the old 19 assets: nine old mainnet binaries unchanged; nine inherited IDLs changed only `metadata.version` to `3.1.0`; nine old manifest entries unchanged plus chainer; four new chainer files across both clusters; policy blocked all nine inherited programs. Do not deploy `all`, derive inherited keys under a rotated secret, or write inherited on-chain IDLs. Mainnet inherited assets do not establish a complete old devnet deployment set.

Historical [`v3.0.0`, f2d06e6a81e28bd1d5e0d8cf8fb4b7d542e42376](https://github.com/eco/eco-routes-svm/commit/f2d06e6a81e28bd1d5e0d8cf8fb4b7d542e42376) is an ID-bearing child of main commit `cdf96e8`, not a main commit or ancestor of current main. Its historical `deploy/v3.0.0` points at `4fba2d`, an ID-bearing child of off-main `a64f65e` with the same nine program IDs, but a different source parent. Neither tag is a placeholder-ID main source. Never blindly use the deploy tag as reproducible provenance: resolve the published artifacts against the actual ID-bearing tree/receipts; see `eco-routes-svm-release` for mixed carry-forward checks.

Pinned source anchors: [steps/verification/cleanup](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/scripts/deploy-step.sh), [IDL state machine](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/packages/deployer/src/idl.rs), [lifecycle selection](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/packages/deployer/src/selection.rs), [upgrade exception](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/README.md#administration-and-finalization), [closure limits](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/README.md#closing), [solana-verify 0.5.2 dispatch](https://github.com/Ellipsis-Labs/solana-verifiable-build/blob/f8cfe2f834f4334aad9c60a29284de0ba396f829/src/main.rs#L490-L502), [remote API implementation](https://github.com/Ellipsis-Labs/solana-verifiable-build/blob/f8cfe2f834f4334aad9c60a29284de0ba396f829/src/api/client.rs#L125-L287).
