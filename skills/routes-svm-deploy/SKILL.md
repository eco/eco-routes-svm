---
name: eco-routes-svm-deploy
description: Plan, deploy, inspect or recover Eco Routes SVM releases, selected-program deployments, IDL uploads and verification failures. Use for mainnet/devnet deployment readiness, funding, 502 verification recovery, and separately authorized finalize/close decisions; never treat skill invocation as permission to write.
---

# Eco Routes SVM deployment

Operate in `eco/eco-routes-svm`. Read its `CLAUDE.md` and `SECURITY.md`. Keep vulnerabilities and security fixes private under that policy. This skill changes no program or pipeline behavior.

## Authorization and scope

Default to read-only planning. A workflow dispatch is a write, even though its first job only reads chain state. Do not dispatch, fund, derive secret keys, deploy, initialize, upload an IDL/verification record, submit a remote verification job, finalize, close or upgrade without user authorization covering those exact actions, cluster, release and programs. A request to inspect or invoke this skill grants none of those permissions. Do not ask again for actions already explicitly authorized; stop when the requested scope ends.

Deployment approval is not finalization approval. Never finalize automatically. Keep upgrade authority until the test team approves and the user separately authorizes finalization. Never approve your own GitHub environment job or bypass required reviewers/self-review protection.

For release creation or mixed-release artifact construction, use installed `eco-routes-svm-release` (repository source: `skills/routes-svm-release/SKILL.md`). For failed/partial runs, 502s or historical v3.1 evidence, read [recovery.md](references/recovery.md) before deciding to retry.

## 1. Establish immutable identity

Record repository, version (without `v`), cluster, exact comma-separated lib names, release commit, deploy-tag commit, workflow ref and asset hashes. Resolve `v<version>` and `deploy/v<version>` independently; the latter supplies real `declare_id!` values and the deployer/scripts used by execution. Do not assume an arbitrary old deploy tag belongs to its release. Use a clean checkout of that deploy tag for local planning; do not replace a teammate's checkout.

Download the complete release manifest and target-cluster assets into a fresh directory. Keep any `deployment-policy.json` beside `program-ids.json`; verify the release's asset list first, because absence of policy means legacy deployability, not a download failure to ignore.

```bash
# Read/download only. VERSION and CLUSTER are the reviewed values.
gh release download "v${VERSION}" --repo eco/eco-routes-svm --dir release \
  --pattern program-ids.json --pattern deployment-policy.json \
  --pattern "*.${CLUSTER}.so" --pattern "*.${CLUSTER}.json"
```

Check selected `.so` filenames, IDL `.address`, manifest address and deploy-tag `declare_id!` agree (`scripts/deploy-step.sh check` performs those checks with `ASSETS`, `CLUSTER`, `PROGRAMS`). Hash the downloaded bytes and retain the release asset inventory. Do not rebuild or substitute local `anchor build` output. Verification must use the release's digest-pinned `VERIFY_IMAGE` and mainnet feature where applicable.

Use explicit names such as `intent_chainer`, not directory names such as `intent-chainer`. Reject unknown, empty, duplicate or policy-forbidden names. Avoid default `all`: mixed releases deliberately block inherited programs. Keep the **full** manifest for transitive dependency validation, but derive keys and write only selected programs. Unselected dependencies must already have matching deployed bytecode; they receive no deploy/init/reserve/IDL/verify writes.

Chainer's Cargo dependency on Portal conservatively affects derivation/finalization ordering. It accepts the Portal address through committed `Order.portal` with runtime checks; do not claim its binary hardcodes the historical Portal ID.

## 2. Review environment and keys without exposing them

| Location | Required identity/control |
|---|---|
| GitHub `release` environment | Required reviewers, self-review prevented, allowed deployment refs reviewed (`main`, or explicitly protected release branches) |
| Environment `DEPLOYER_KEYPAIR` | JSON payer and upgrade-authority keypair; derive **public key only** in a trusted secret-handling context |
| Repository `DEPLOYER_ADDRESS` | Must equal that public key and every selected partial program's expected authority |
| Environment `PROGRAM_KEYPAIR_SECRET` | Same derivation secret that produced selected release addresses; deploy only, never needed by read-only planning/finalize/close |
| Repository `ALCHEMY_API_KEY` | Solana mainnet/devnet RPC access; URL contains credential and must stay redacted |
| Tag rules/review | Protect `v*` and `deploy/*`; review workflow, derivation-script and salt changes |

Secret names/listing do not prove their values match. Ask an authorized holder to attest the pubkey/derivation identity if inaccessible; do not export secrets to chat or artifacts. A trusted holder can run `solana-keygen pubkey "$KEYPAIR"` locally. The workflow's second plan uses that pubkey and rejects mismatch with the first plan hash. Do not disable the gate to fix a mismatch. Derivation independently rejects selected keys whose addresses do not match the manifest.

Check the active operator and repository access read-only before planning a workflow:

```bash
gh auth status --no-refresh
gh api user --jq .login
gh repo view eco/eco-routes-svm --json nameWithOwner,viewerPermission,defaultBranchRef
gh api repos/eco/eco-routes-svm/environments/release \
  --jq '{name,protection_rules,deployment_branch_policy}'
```

Do not print tokens (`gh auth token`, token-bearing config dumps or debug traces). Compare this identity with the authorized operator and actual environment reviewer/branch policy; repository `ADMIN` is not permission for every action or self-approval. If environment policy is inaccessible or custom deployment branch restrictions are present, obtain the authorized administrator's read-only policy evidence before dispatch; do not treat missing evidence as no protection.
Inspect `gh ruleset list --repo eco/eco-routes-svm` and `gh api repos/eco/eco-routes-svm/environments/release/deployment-branch-policies` for rules and custom allowed refs; unreadable policy remains an explicit blocker to dispatch, not to local preparation.
Read the public planner identity with `gh variable get DEPLOYER_ADDRESS --repo eco/eco-routes-svm` and check for a `release` environment override using `gh variable list --repo eco/eco-routes-svm --env release`. This reveals no secret values. Confirm it matches the authorized payer public key; do not infer that secret metadata proves wallet ownership.

## 3. Produce a plan without dispatch

Use the deploy-tag-built `deployer` (separate Cargo workspace/lockfile). Obtain toolchain versions and build procedure from that tag's `.github/actions/setup-deployer/action.yaml`; do not silently use a binary built from main. Run `deployer plan --help` before use. `plan` reads RPC and writes local plan/log files, not chain state; it needs no payer or program private keys.

Use a fresh evidence directory for each plan: summaries append, so reusing an old summary can mix stale balances with current results. Preserve prior receipts rather than presenting the first summary block as the latest plan.
Before handoff or temporary-directory cleanup, copy the redacted receipts to the operator's chosen durable evidence store. Report that retained location; a temporary path alone is not durable retention.

```bash
# In the reviewed deploy-tag checkout. Set RPC_URL privately; never enable shell tracing.
# DEPLOYER is the reviewed public key; PROGRAMS is the exact selection.
(
  set -euo pipefail
  set +x
  export ASSETS=release
  receipt_dir=$(mktemp -d "${TMPDIR:-/tmp}/routes-svm-plan.XXXXXXXX")
  printf 'Plan receipt directory: %s\n' "$receipt_dir"
  deployer plan --version "$VERSION" --cluster "$CLUSTER" \
    --program-ids "$ASSETS/program-ids.json" --assets "$ASSETS" \
    --out "$receipt_dir/plan.json" --summary "$receipt_dir/summary.md" \
    --programs "$PROGRAMS" --deployer "$DEPLOYER" \
    2>&1 | scripts/deploy-step.sh redact | tee "$receipt_dir/plan.log"
)
```

`RPC_URL` is the CLI environment input. For a chainer-only plan, no unrelated config inputs are needed. For other selected programs, supply reviewed inputs through CLI flags or environment: `HYPER_SENDERS`, `POLYMER_EMITTERS`, `LAYERZERO`, `CCTP`, `HYPER_RESERVE_LAMPORTS`, `LAYERZERO_RESERVE_LAMPORTS`, `COMPUTE_UNIT_PRICE`. Read the tag's README deployment input table and `packages/deployer/src/inputs.rs` for exact JSON; never use example peer/worker addresses as real configuration. Aggregator membership comes from the release, not an operator override.

Review normalized inputs, selected set, policy, dependency closure, genesis hash, deployer, observed bytecode/authority/configs, actions, funding and `plan_hash`:

| Status | Meaning/action |
|---|---|
| `New` | Absent program and programdata; eligible for new deployment |
| `Partial` | Release bytes match; authority is deployer. Plan always includes verification; init/IDL skip equal state |
| `Live` | Matching immutable program; leave unchanged |
| `Closed` | Missing programdata; burned address; cannot redeploy |
| `Foreign` | Wrong bytes/authority/account type, or pre-funded new address/programdata; stop |

A status is not a health verdict. In particular `Partial` may be fully deployed but intentionally not finalized. Re-dispatching Deploy is not step-selective recovery: it repeats verification-record writes and, on mainnet, remote job submission even if verification already succeeded. Authorize those repeated writes explicitly before proposing a retry. The hash binds selection, policy, release/binaries, genesis, public key, inputs/price and read facts; balances are excluded. Keep the local plan as readiness evidence, not permission or a hash to force onto a later workflow.

### Funding is not a flat fee

Use the current plan estimate, not historical SOL spent. It includes program rent, reserve shortfalls, IDL rent/scratch peak, signature/priority fees and an allowance for config/unpriced transactions. Loader-v3 drains upload buffers back before charging larger programdata; do not double-count buffer rent as permanently spent. Rent remains locked while accounts remain; final program rent cannot be reclaimed. IDL replacement scratch rent is returned. Reserve deposits are protocol balances, not transaction fees.

`compute_unit_price` is micro-lamports/CU, bound to the plan; apply uses price × 1.4M CU per v1 transaction. A LayerZero reserve must cover pending endpoint/ULN setup and remain rent-exempt; operational proof rent also needs ongoing funding. A shortfall fails after plan evidence is written. Funding is a separate authorized transfer; afterwards re-plan. Do not infer that a failed run sent nothing or that its balance delta equals network fees.

## 4. Execute only the authorized workflow scope

Read current `deploy.yml` and the release-tag script before dispatch. The dispatch workflow ref and checked-out deploy tag are different identities: choose an allowed, reviewed workflow ref, while the workflow checks out `deploy/v${VERSION}`. Confirm no same-cluster lifecycle run overlaps. One concurrency group covers Deploy/Finalize/Close; newer waiting runs can cancel older waiting runs.
A dispatch used only to inspect the plan must be rejected/cancelled at its pending environment gate by an authorized operator. Leaving it waiting retains the shared cluster concurrency slot and can block other lifecycle runs. Prefer local read-only planning when no execution is authorized.

Only when dispatch is authorized, a chainer-only request has this form (never run as a readiness check):

```bash
# WRITE: creates a deployment workflow run; requires explicit scope authorization.
gh workflow run deploy.yml --repo eco/eco-routes-svm --ref main \
  -f version="$VERSION" -f cluster="$CLUSTER" -f programs=intent_chainer \
  -f compute_unit_price="$COMPUTE_UNIT_PRICE"
```

For other selections include their reviewed workflow inputs. Stop at the environment gate for independent human review. Read the actual plan-job summary, not only the local readiness plan. The approved job re-plans with the secret's public key and requires the same hash; apply rechecks before writes. Changed chain state means a fresh plan/review, never gate bypass.

Execution order:
1. Derive only selected program keys. Before uploading, apply the buffer-scope gate below; deploy selected `New` programs using the script's selected or legacy branch.
2. `deployer apply` funds selected reserve shortfalls, initializes selected configs and reads them back.
3. `deployer idl` writes/read-checks selected upgradeable canonical Program Metadata IDLs, resuming sequential chunk writes. Never substitute `anchor idl init/upgrade` or the parallel metadata CLI.
4. Verify selected program bytes with `solana-verify verify-from-repo`; require `Program hash matches`, not exit code alone. This uploads verification parameters; on mainnet the script then submits the remote job with `remote submit-job`.
5. Run the same branch's buffer cleanup, delete temporary program/buffer/payer keys even on failure, and retain redacted public records. Cleanup scope is not determined by the spelling of `programs`.

**Buffer-scope gate:** `legacy_full_deploy` is true exactly when `deployment-policy.json` is absent **and** the plan's selected count equals the full manifest count. It includes a comma-separated list naming every program, not only `all`. That branch runs payer-wide `solana program close --buffers` before deployment and again during cleanup, and does not use per-run explicit buffers. Before dispatch/approval, require explicit authority over every payer-owned buffer and evidence that no unrelated/concurrent upload uses that payer, including outside this repo's concurrency group. Otherwise stop; narrowing selection is only acceptable if it still matches the authorized deployment, not as a policy bypass. With a genuine mixed-release policy, or a proper subset, the existing selected branch uses fresh per-run buffers and closes only those recorded addresses. Do not add a synthetic policy solely to force a different branch.

Config init applies to hyper/polymer/CCTP/aggregator/LayerZero only. Portal, chainer, local prover, flash fulfiller and proof helper have no config init. Init, IDL and verification-record writes require upgrade authority and precede any finalization. Success ends **NOT FINALIZED**, with authority recorded.

## 5. Recovery, lifecycle and handoff

Use [recovery.md](references/recovery.md) for transaction reconciliation, IDL interruption, verification 502s, targeted buffer cleanup, upgrade/rollback limits and the v3.1 receipt. Never rerun writes just to make GitHub green.

Finalize and Close require separate task authorization, explicit selection and independent environment approval. Their read-only readiness commands are `deployer plan-finalize` and `deployer plan-close`, with `--version`, `--cluster`, `--program-ids`, `--assets`, `--out`, `--programs`, `--deployer` and private `RPC_URL`; inspect the deployed tag's CLI first. Policy still blocks inherited programs, including `all` on mixed releases.
Read-only lifecycle readiness checks may proceed during deployment planning without authority to finalize or close. Only execute their workflows after separate authorization; readiness evidence is not permission for a lifecycle write.
If you lack finalization authority or test-team approval, do not dispatch Finalize. You may run `deployer plan-finalize` locally and hand the redacted readiness report to an authorized operator. `DependencyNotFinal` for an upgradeable Portal is a valid stop, not permission to finalize Portal or broaden a mixed-release selection.

Finalize removes upgrade authority irreversibly. Require test-team approval, exact canonical IDLs, deployer-uploaded verification record and initialized configs. Dependencies must already be final or finalize first within the authorized, policy-allowed selection. **Inherited dependencies must already be immutable**: mixed-release policy prevents selecting them for finalization, so stop if one remains upgradeable; never bypass policy to finalize the dependent. LayerZero additionally requires full pinned setup, frozen active complete ALT, and real outbound/inbound tests plus readbacks for **every** mainnet path. Never replace that gate with a generic CI success.

Close burns addresses permanently, refuses final programs and programs required by final dependents, and closes dependents first. Close canonical IDL/scratch accounts **before** program closure; otherwise their authority/rent is stranded. Program closure does not drain arbitrary PDAs, escrow tokens, proof/reserve balances or endpoint/ULN accounts. Reconcile users' live intents and funds before proposing abandonment. Do not portray close as rollback.

Deliver: identity/asset hashes, selection and untouched dependencies, redacted plan/hash, normalized inputs, funding estimate, authorization/reviewer boundaries, run URL, each step's outcome, transaction signatures/slots, authority and finality state, IDL/hash/remote-job evidence, cleanup status, blockers and next authorized action. Label inherited receipts separately from new observations.

## Source baseline

These are pinned instructions, not proof of current chain state. Re-read the target tag when it differs:
- [Deploy workflow](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/.github/workflows/deploy.yml), [setup action](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/.github/actions/setup-deployer/action.yaml).
- [CLI](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/packages/deployer/src/cli.rs), [classification](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/packages/deployer/src/classify.rs), [deployment steps](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/scripts/deploy-step.sh).
- [Operational documentation](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/README.md#deploying-a-release), [security policy](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/SECURITY.md).
