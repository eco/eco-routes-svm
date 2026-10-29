---
name: eco-routes-svm-release
description: Prepare, dry-run, publish, or audit eco/eco-routes-svm releases, semantic versions, deploy tags, program IDs, release assets, and exceptional mixed carry-forward releases. Use whenever a Routes SVM release, rotated release secret, release ancestry, or artifact provenance is involved; deployment itself belongs to eco-routes-svm-deploy.
---

# Routes SVM release

Work from an `eco/eco-routes-svm` checkout. Read its `CLAUDE.md`, `SECURITY.md`, `README.md` Release section, `.releaserc.json`, `.github/workflows/release.yml`, and `scripts/program-keypairs.mjs` before acting. Use the selected source ref's files, not this skill's snapshot, as authority. Source links and historical exceptions: [references/release-evidence.md](references/release-evidence.md).

## Authority and scope

Skill invocation is not authorization. Separate read-only inspection, workflow dispatch (including dry-run), publication, asset replacement, and deployment. Execute only actions the user explicitly authorized, against the named repository/ref/version. Do not ask again when authorization already covers the exact action. A request to prepare or audit does not authorize dispatch, tags, branches, publication, or secret access.

Use the existing Release workflow, not a parallel manual pipeline. Stop normal publication for a security-relevant fix until the repository's private-disclosure policy is satisfied. Never publish vulnerability details. Never print secrets, private key material, or authenticated RPC URLs. Do not open `.secret` or personal configuration to discover credentials.

Before dispatch, check `gh auth status --no-refresh`, `gh api user --jq .login`, and `gh repo view eco/eco-routes-svm --json nameWithOwner,viewerPermission,defaultBranchRef`. Inspect `gh api repos/eco/eco-routes-svm/environments/release` for required reviewers and allowed refs, plus applicable branch/tag rules. Repository write/admin access alone does not prove organization-rule bypass or independent environment approval. If access cannot be established, continue local preparation and report the exact permission blocker; do not bypass protection.
Use `gh ruleset list --repo eco/eco-routes-svm` and `gh api repos/eco/eco-routes-svm/environments/release/deployment-branch-policies` for the applicable rule inventory and custom allowed refs. Treat inaccessible policy as an unresolved access check, not permission to bypass it.

## 1. Establish source and version

- Record the full source SHA, branch, reachable version tags, and releasable conventional commits. Use full history. `main` and `releases/<major>.<minor>.x` are configured release branches; maintenance ranges constrain their versions.
- Semantic-release computes the version: `feat:` → minor, `fix:` → patch, breaking changes → major; docs/chore-only history may produce no release. Do not bump Cargo package versions, invent a version override, add synthetic tags, or repair ancestry automatically.
- Distinguish `v<version>` (release/source commit) from `deploy/v<version>` (its direct child with real `declare_id!` values). Verify parent, tree diff, and addresses, not merely tag names or release `targetCommitish`. Normal deploy-tag changes are only the production `declare_id!` lines. Neither that child nor its placeholder-dependent test goldens belongs on `main`.
- Current baseline: `v3.1.0` at `d22d794c2ced17ba790bdad8a04f3fba7d943086` is reachable from this skill's source. Re-check reachability for each new release. Historical `v3.0.0` is an IDs-bearing child off main, not a main ancestor; `deploy/v3.0.0` carries the same nine IDs on a different parent. Do not trust either tag's ancestry or reproduction provenance by name alone. See the reference.
- Existing deploy tags are accepted by Release only when their tree and parent match the computed commit. A mismatch is a stop, not permission to move/delete a published tag. Partial publication requires evidence review before another dispatch; do not assume retry is harmless.

## 2. Check build and ID contract

Read pins from the chosen ref. Snapshot at v3.1.0:

| Component | Pin |
|---|---|
| Host Rust | `1.97.1` |
| Anchor | `1.1.2` |
| Solana CLI | `4.1.1` |
| solana-verify | `0.5.2` |
| Node | `22` |
| Verifiable image | `solanafoundation/solana-verifiable-build@sha256:0b4e3716fad9ca4b4aac3e3f977f43aad93a18c22296c0c0f44fc22e644bdd68` |

Anchor's SBF compiler is separate from host Rust and CLI version: platform-tools v1.52 uses rustc 1.89.0-dev. Inspect package tools-version metadata and `scripts/assert-sbf-rustc.sh` when assessing reproducibility. Released bytecode uses `scripts/verifiable-build.sh` / `solana-verify build` in the pinned image, not local `anchor build`. IDLs use `anchor idl build`.

The explicit production inventory is:

```text
aggregator_prover portal intent_chainer hyper_prover local_prover
flash_fulfiller proof_helper polymer_prover layerzero_prover cctp_prover
```

Keep localnet mocks, PoC programs, and test-only template-buffer out. Never substitute an unfiltered build/copy glob for the inventory.

Committed IDs are placeholders. The helper's `release <placeholder-dir> <program>...` reads both-cluster placeholder binaries, derives addresses, and prints `program-ids.json`; it writes no private keys. Each entry carries `address`, `seed`, `salt`, and `dependencies`. Seeds include salt, both placeholder builds, and dependency seeds through Cargo/shared crates. Aggregator adds members in this configured order: `hyper_prover`, `polymer_prover`, `layerzero_prover`, `cctp_prover`. Dependencies propagate address changes. Chainer takes `Order.portal` at runtime, but its conservative Cargo dependency still participates in release derivation.

`PROGRAM_KEYPAIR_SECRET` is hex, at least 32 bytes; possession permits claiming derived addresses. Rotation changes every derived ID even when bytecode is unchanged. After the mixed v3.1.0 release and secret rotation, a normal release under the replacement secret rekeys inherited programs too. Before publication, require an authorized secret holder to confirm lineage without fetching/exposing secret values, and confirm the operator intends every resulting identity change. Unknown lineage or unintended rekeying stops publication. Selected deployment does not make normal Release safe after rotation: Release derives every program. Never derive inherited keys with a replacement secret.

Salts default to zero in `scripts/program-salts.json`. Only raise them, with explicit authorization for the address change and dependency impact; never lower salts, change derivation `DOMAIN`, or edit committed placeholders to force an address.

## 3. Dry-run, inspect, then publish

Release is manual, default `dry_run=true`, concurrency group `release`, environment `release`; no automatic release on push. It computes the semantic version, builds placeholders, derives IDs, rewrites IDs on-runner, builds both clusters, stamps IDL versions, and constructs/checks the deploy child. Dry-run publishes no release, tags, or branches; it still dispatches a credential-bearing workflow and needs authorization.

Only after authorization for that exact dispatch, use the configured branch, for example:

```bash
gh workflow run release.yml --repo eco/eco-routes-svm --ref main -f dry_run=true
```

Record the run URL/SHA and inspect its summary and `release-v<version>-dry-run` artifact. If no releasable commit exists, report that result; do not manufacture a bump. Before a live run, confirm source did not change and review the predicted version, IDs, inventory, and secret lineage. Re-run inspection after any changed input.

Only with publication authorization covering the exact branch and predicted version:

```bash
gh workflow run release.yml --repo eco/eco-routes-svm --ref main -f dry_run=false
```

The workflow pushes the deploy tag before semantic-release publishes `v<version>` and assets, then creates `releases/<major>.<minor>.x`. Report each outcome separately; workflow success alone is not artifact proof. Do not manually publish beside a running workflow. Do not automatically modify tags, assets, history, or maintenance branches after failure.

## 4. Validate published deliverable

Inspect remote release metadata and download into a fresh local evidence directory; local downloads are not publication:

```bash
gh release view v3.1.0 --repo eco/eco-routes-svm --json tagName,url,assets,targetCommitish
gh release download v3.1.0 --repo eco/eco-routes-svm --dir release-evidence/v3.1.0
```

Replace the example version with the reviewed version. Resolve tag SHAs independently. For ten programs, a normal release has 41 assets: two `.so` and two IDLs per program, plus `program-ids.json`. Check every `<program>.<cluster>.so` and `<program>.<cluster>.json`, no unexpected assets, IDL `.address` equals manifest address, IDL `.metadata.version` equals release version, and manifest dependency closure matches the source contract. Both clusters share program IDs, not bytecode; mainnet builds require `mainnet`. Hyper, Polymer, and proof-helper IDLs embed cluster-specific addresses. Record asset digests and compare bytes against reviewed dry-run evidence where available.

For reproduction, inspect `.github/workflows/reproduce.yml` at the intended ref. It rebuilds **every** program from the tag's own pins and compares on-chain bytecode and canonical IDLs. `upload=false` avoids asset writes; dispatch still needs authorization. `upload=true` replaces release assets and needs separate authorization. A placeholder `v` tag is not suitable merely because the workflow accepts its name: normal releases require `deploy/v<version>` carrying real IDs. Mixed releases need per-program provenance; whole-tree reproduction can intentionally disagree for inherited programs and IDL versions.

## Exceptional mixed carry-forward

Read [references/release-evidence.md](references/release-evidence.md) before proposing this path. Require an explicit mixed-release policy, exact inherited/new program partition, source/asset provenance, and authorization for each publication action. This is not an automatic fallback for failed Release. The normal workflow does not compose mixed assets or attach a deployment policy; do not dispatch it expecting selective behavior.

Carry inherited manifest entries and binaries unchanged; change inherited release IDLs only at `metadata.version`. Keep their on-chain IDLs untouched. Include the full manifest and `deployment-policy.json` listing **all** inherited programs under `nondeployable_programs`. Build only newly authorized programs with existing helpers and reviewed dependency IDs; never rebuild inherited binaries or regenerate inherited seeds/keys. Do not invent a generic secret-handling script or promise a whole-tree rebuild reproduces historical carry-forward artifacts.

## Handoff

Return source/release/deploy SHAs and ancestry, semantic baseline/prediction, dispatch authorization scope and run links, pins, asset inventory/digests, ID/IDL checks, mixed provenance/policy if applicable, and unresolved differences. Mark inherited evidence explicitly. Pass the immutable release and exact new program selection to installed `eco-routes-svm-deploy`; publication does not authorize deploy, funding, init, finalize, close, upgrade, or Slack. Leave programs upgradeable until the test team approves finalization.
