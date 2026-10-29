---
name: eco-routes-svm-release
description: Prepare, dry-run, publish, or audit eco/eco-routes-svm releases, semantic versions, deploy tags, program IDs, release assets, and exceptional mixed carry-forward releases. Use whenever a Routes SVM release, rotated release secret, release ancestry, or artifact provenance is involved; deployment itself belongs to eco-routes-svm-deploy.
---

# Routes SVM release

Work from an `eco/eco-routes-svm` checkout. Read its `CLAUDE.md`, `SECURITY.md`, `README.md` Release section, `.releaserc.json`, `.github/workflows/release.yml`, and `scripts/program-keypairs.mjs` before acting. Use the selected source ref's files, not this skill's snapshot, as authority.

## Choose the path

**Default: use the existing Release workflow.** Follow sections 1–4 for version prediction, authorized dry-run, artifact checks and authorized publication. Do not start by reproducing a historical manual release.

| Situation | Path |
|---|---|
| Normal release from a configured branch, with intended derived identities | Follow the standard steps below; the workflow builds the full production inventory for both clusters. |
| Preserve old binaries/addresses while releasing new programs, unexpected secret lineage, broken tag ancestry, or partial publication | Inspect [release-evidence.md](references/release-evidence.md) and current tooling before any publication retry or manual action. |
| Another unsupported or uncertain case | Gather source, tag, asset and workflow evidence; identify the unsupported step and propose a scoped recovery/exception plan. Continue safe preparation, but stop writes until a supported, reviewed path and its authorization are established. |

Historical receipts explain past decisions; they never supply the next release's version, IDs, secret lineage or permission. Do not relax standard provenance, compatibility or approval checks for an exception.

## Authority and scope

Skill invocation is not authorization. Separate read-only inspection, workflow dispatch (including dry-run), publication, asset replacement, and deployment. Execute only actions the user explicitly authorized, against the named repository/ref/version. Do not ask again when authorization already covers the exact action. A request to prepare or audit does not authorize dispatch, tags, branches, publication, or secret access.

Use the existing Release workflow, not a parallel manual pipeline. Stop normal publication for a security-relevant fix until the repository's private-disclosure policy is satisfied. Never publish vulnerability details. Never print secrets, private key material, or authenticated RPC URLs. Do not open `.secret` or personal configuration to discover credentials.

Before dispatch, check `gh auth status --no-refresh`, `gh api user --jq .login`, and `gh repo view eco/eco-routes-svm --json nameWithOwner,viewerPermission,defaultBranchRef`. Inspect `gh api repos/eco/eco-routes-svm/environments/release` for required reviewers and allowed refs, plus applicable branch/tag rules. Repository write/admin access alone does not prove organization-rule bypass or independent environment approval. If access cannot be established, continue local preparation and report the exact permission blocker; do not bypass protection.
Use `gh ruleset list --repo eco/eco-routes-svm` and `gh api repos/eco/eco-routes-svm/environments/release/deployment-branch-policies` for the applicable rule inventory and custom allowed refs. Treat inaccessible policy as an unresolved access check, not permission to bypass it.

## 1. Establish source and version

- Record the full source SHA, branch, reachable version tags, and releasable conventional commits. Use full history. `main` and `releases/<major>.<minor>.x` are configured release branches; maintenance ranges constrain their versions.
- Semantic-release computes the version: `feat:` → minor, `fix:` → patch, breaking changes → major; docs/chore-only history may produce no release. Do not bump Cargo package versions, invent a version override, add synthetic tags, or repair ancestry automatically.
- Distinguish `v<version>` (release/source commit) from `deploy/v<version>` (its direct child with real `declare_id!` values). Verify parent, tree diff, and addresses, not merely tag names or release `targetCommitish`. Normal deploy-tag changes are only the production `declare_id!` lines. Neither that child nor its placeholder-dependent test goldens belongs on `main`.
- Check the selected source's reachability from the intended release branch and semantic baseline. Historical tags can contain production IDs or diverge from normal ancestry; diagnose that evidence through the reference instead of automatically moving tags or switching to a manual release.
- Existing deploy tags are accepted by Release only when their tree and parent match the computed commit. A mismatch is a stop, not permission to move/delete a published tag. Partial publication requires evidence review before another dispatch; do not assume retry is harmless.

## 2. Check build and ID contract

Read pins from the chosen ref; the table below is a source snapshot, not a requirement to retain old versions:

| Component | Pin |
|---|---|
| Host Rust | `1.97.1` |
| Anchor | `1.1.2` |
| Solana CLI | `4.1.1` |
| solana-verify | `0.5.2` |
| Node | `22` |
| Verifiable image | `solanafoundation/solana-verifiable-build@sha256:0b4e3716fad9ca4b4aac3e3f977f43aad93a18c22296c0c0f44fc22e644bdd68` |

The SBF compiler is separate from host Rust and CLI version: platform-tools v1.52 uses rustc 1.89.0-dev. Release compiler evidence comes from the pinned `VERIFY_IMAGE`, package `tools-version` metadata and actual container build logs. `scripts/assert-sbf-rustc.sh` checks the host's cached Anchor SBF toolchain in PR checks; Release does not run it, so it does not prove the release compiler. Released bytecode uses `scripts/verifiable-build.sh` / `solana-verify build`, not local `anchor build`; export the exact pinned `VERIFY_IMAGE` before invoking the helper. Arrange scoped ownership recovery for container-created `target/` files as `release.yml` does before IDL/host steps or cleanup. IDLs use `anchor idl build`.

The explicit production inventory is:

```text
aggregator_prover portal intent_chainer hyper_prover local_prover
flash_fulfiller proof_helper polymer_prover layerzero_prover cctp_prover
```

Keep localnet mocks, PoC programs, and test-only template-buffer out. Never substitute an unfiltered build/copy glob for the inventory.

Committed IDs are placeholders. The helper's `release <placeholder-dir> <program>...` reads both-cluster placeholder binaries, derives addresses, and prints `program-ids.json`; it writes no private keys. Each entry carries `address`, `seed`, `salt`, and `dependencies`. Seeds include salt, both placeholder builds, and dependency seeds through Cargo/shared crates. Aggregator adds `hyper_prover`, `polymer_prover`, `layerzero_prover`, `cctp_prover`; derivation sorts dependency names, while the configured member order matters for initialization's remaining accounts. Dependencies propagate address changes. Subset helper runs can drop released dependency edges; they are not a mixed-release assembly recipe (see the reference).

`PROGRAM_KEYPAIR_SECRET` is hex, at least 32 bytes; possession permits claiming derived addresses. Rotation changes every derived ID even when bytecode is unchanged. Require an authorized secret holder to confirm lineage without fetching/exposing secret values, and confirm the operator intends every resulting identity change. Unknown lineage or unintended rekeying stops publication. Selected deployment does not make normal Release selective: Release derives every program. If existing identities must be preserved across rotation or carry-forward, use the exception reference; never derive inherited keys with a replacement secret.

Salts default to zero in `scripts/program-salts.json`. Only raise them, with explicit authorization for the address change and dependency impact; never lower salts, change derivation `DOMAIN`, or edit committed placeholders to force an address.

## 3. Dry-run, inspect, then publish

Release is manual, default `dry_run=true`, concurrency group `release`, environment `release`; no automatic release on push. It computes the semantic version, builds placeholders, derives IDs, rewrites IDs on-runner, builds both clusters, stamps IDL versions, and constructs/checks the deploy child. Dry-run publishes no release, tags, or branches; it still dispatches a credential-bearing workflow and needs authorization.

Only after authorization for that exact dispatch, set `WORKFLOW_REF` to the reviewed configured release branch:

```bash
gh workflow run release.yml --repo eco/eco-routes-svm --ref "$WORKFLOW_REF" -f dry_run=true
```

Record the run URL/SHA and inspect its summary and `release-v<version>-dry-run` artifact. If no releasable commit exists, report that result; do not manufacture a bump. Before a live run, confirm source did not change and review the predicted version, IDs, inventory, and secret lineage. Re-run inspection after any changed input.

Only with publication authorization covering the exact branch and predicted version:

```bash
gh workflow run release.yml --repo eco/eco-routes-svm --ref "$WORKFLOW_REF" -f dry_run=false
```

The workflow pushes the deploy tag before semantic-release publishes `v<version>` and assets, then creates `releases/<major>.<minor>.x`. Report each outcome separately; workflow success alone is not artifact proof. Do not manually publish beside a running workflow. Do not automatically modify tags, assets, history, or maintenance branches after failure.

## 4. Validate published deliverable

Inspect remote release metadata and download into a fresh local evidence directory; local downloads are not publication. Set `VERSION` to the reviewed computed version without `v`, and `EVIDENCE_DIR` to a new evidence directory:

```bash
gh release view "v${VERSION}" --repo eco/eco-routes-svm --json tagName,url,assets,targetCommitish
gh release download "v${VERSION}" --repo eco/eco-routes-svm --dir "$EVIDENCE_DIR"
```

Resolve tag SHAs independently. Derive the expected inventory from the selected source's production list: each program contributes two `.so` files and two IDLs, plus one shared `program-ids.json` (41 assets for the ten-program snapshot above). Check every `<program>.<cluster>.so` and `<program>.<cluster>.json`, no unexpected assets, IDL `.address` equals manifest address, IDL `.metadata.version` equals release version, and manifest dependency closure matches the source contract. Both clusters share program IDs, not bytecode; mainnet builds require `mainnet`. Check feature-dependent IDL addresses against the selected source. Record asset digests and compare bytes against reviewed dry-run evidence where available.

For reproduction, inspect `.github/workflows/reproduce.yml` at the intended ref. It rebuilds **every** program from the tag's own pins and compares on-chain bytecode and canonical IDLs. `upload=false` avoids asset writes; dispatch still needs authorization. `upload=true` replaces release assets and needs separate authorization. A placeholder `v` tag is not suitable merely because the workflow accepts its name: normal releases require `deploy/v<version>` carrying real IDs. Mixed releases need per-program provenance; whole-tree reproduction can intentionally disagree for inherited programs and IDL versions.

## Exceptions are not a second release pipeline

Read [references/release-evidence.md](references/release-evidence.md) only when the normal contract cannot satisfy the request or observed state. It covers mixed carry-forward invariants, ancestry/provenance checks and a historical example. A failed normal workflow does not authorize manual tagging, asset replacement, secret rotation or mixed assembly. Diagnose first; require an exact reviewed recipe and authorization for any exceptional writes. If the current tooling cannot support the required invariant, report that concrete gap and stop publication rather than inventing a bypass.

## Handoff

Return source/release/deploy SHAs and ancestry, semantic baseline/prediction, dispatch authorization scope and run links, pins, asset inventory/digests, ID/IDL checks, exception provenance/policy if applicable, and unresolved differences. Mark inherited evidence explicitly. Pass the immutable release and authorized program selection to installed `eco-routes-svm-deploy`; publication does not authorize deploy, funding, init, finalize, close, upgrade, or Slack. Leave programs upgradeable until the test team approves finalization.
