# Release sources and mixed-release evidence

## Authoritative source snapshot

These are pinned at the v3.1.0 source commit. Re-read the intended release ref before executing anything:

- [Release workflow](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/.github/workflows/release.yml): dry-run default, pins, inventory, two-stage build, deploy-child guard, publication and maintenance branch.
- [Semantic-release configuration](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/.releaserc.json): branch ranges and asset globs. It does not include `deployment-policy.json`.
- [Key derivation helper](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/scripts/program-keypairs.mjs): zero-dependency Node helper, seed/dependency graph, modes, policy enforcement and selected-key derivation.
- [Verifiable build helper](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/scripts/verifiable-build.sh): builds named libraries for both clusters.
- [Reproduce workflow](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/.github/workflows/reproduce.yml): tag validation, tag-local pins, whole-inventory comparison, optional replacing upload.
- [README Release](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/README.md#release): release, policy, secret protections and deployment contract.
- [Security policy](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/SECURITY.md): private disclosure before public fix publication.

CLI examples use `gh workflow run --repo --ref -f`, `gh release view --repo --json`, and `gh release download --repo --dir`, checked against installed command help while drafting. This does not mean any workflow or release operation ran.

Vendored external-program binaries under `bins/` are integration fixtures, not production release assets or proof of current mainnet identity. When compatibility depends on them, verify their upstream revision, digest, executable program ID and intended cluster against the target source and runtime evidence. Obtain provenance from recorded upstream or dump receipts; if absent, mark it unknown rather than infer it from the filename. [CCTP's mirror](https://github.com/eco/eco-routes-svm/blob/d22d794c2ced17ba790bdad8a04f3fba7d943086/programs/cctp-prover/src/cctp.rs) records Circle's interface revision and uses the same MessageTransmitter V2 ID on both clusters; a `mainnet` feature toggle alone cannot establish external-program identity. Follow the README's real LayerZero binary/path checks rather than treating mock integration coverage as mainnet proof.

## Ancestry is evidence, not a naming convention

| Historical ref | Commit | Meaning |
|---|---|---|
| `v3.0.0` | `f2d06e6a81e28bd1d5e0d8cf8fb4b7d542e42376` | IDs-bearing child of main commit `cdf96e83e80ad0b5fe4d5568ba83f053f97ea16c`; not a main ancestor |
| `deploy/v3.0.0` | `4fba2d395f795cb046a0c55792fc54f4f6a4fda4` | Same nine IDs, but child of off-main commit `a64f65e6db662b8bd396baedbcc570d311b75153` |
| `v3.1.0` | `d22d794c2ced17ba790bdad8a04f3fba7d943086` | Main-reachable semantic baseline |
| `deploy/v3.1.0` | `c2946ab2749a4ecb82ecdd857b78ac8de9715fdc` | Direct child of v3.1.0; only ten production `declare_id!` replacements |

The v3.0 mismatch concerns ancestry, not different program IDs: the two tags carry the same nine IDs on different parents. This inherited operational evidence is not permission to repair tags or assume either tree reproduces every historical asset. Inspect source and artifact provenance. Published historical refs remain unchanged; unmerged proposals are not evidence of released behavior.

## Mixed-release procedure and acceptance gates

A mixed release deliberately combines published historical artifacts with newly built programs. A single current source checkout cannot prove the provenance of all its binaries. A rotated secret cannot recover old program identities. Before any write:

1. Record the new source SHA, its deploy-child SHA, semantic-release baseline and approved version. Record each inherited program's old release, asset name, manifest entry, cluster and cryptographic digest. Validate source provenance independently of a broken historical deploy tag.
2. Record the exact inherited/new partition and compatibility rationale: ABI, PDA/authentication expectations, runtime dependencies, compile-time dependency IDs, and cluster features. An inherited Portal does not automatically make every new dependent compatible. Chainer's Portal comes from committed `Order.portal` with runtime checks; its conservative Cargo derivation edge remains real.
3. Require authorization explicitly covering mixed carry-forward, policy contents, creation/publication of the new version and deploy ref, and any asset uploads. If a target version already exists, do not silently overwrite its assets. Never mutate published tags.
4. Download inherited artifacts from their approved published source into a fresh staging directory. Preserve inherited `.so` files byte-for-byte and full manifest entries structurally unchanged, including address, seed, salt and dependencies. Do not feed those programs through release derivation under a new secret.
5. Export the selected source's pinned `VERIFY_IMAGE` before `scripts/verifiable-build.sh`; the helper refuses an unset image. Its container can leave `target/` root-owned: arrange narrowly scoped ownership recovery for that scratch output, as the workflow does, before subsequent IDL/host steps or cleanup. Build new placeholder binaries for both clusters, then final binaries with the reviewed production declarations. Record container/SBF compiler evidence, not an unrelated host cache assertion.
   Compute `dependencies(metadata, fullProductionInventory)` using the exported helper and Cargo metadata **before** narrowing to new programs. The CLI's subset `release` mode silently treats omitted released crates as unreleased and can emit `intent_chainer: []` instead of `["portal"]`. Never publish that dependency list. A full-inventory derivation under the replacement secret would generate new inherited identities and is not a carry-forward result either.
   The stock CLI has no frozen-dependency-seed input. A separately reviewed composition recipe must hash each new program's name/NUL, salt, both placeholder binary hashes and sorted dependency name/NUL/seed pairs using the exact existing algorithm, substituting the **published inherited seeds**, then derive only the new keys with the authorized secret. Preserve every inherited manifest entry; require every new `dependencies` list to equal the full-inventory graph and validate selected key reproduction against the resulting full manifest. If this recipe/evidence is unavailable, stop before assembly/publication rather than substitute either malformed CLI result.
6. Assemble full manifest plus the approved new entries, both-cluster new binaries/IDLs, and inherited artifacts. In inherited IDLs update only `metadata.version`; compare parsed JSON with that field removed to prove equality. Do not rewrite inherited instruction schemas, addresses, metadata fields, or on-chain IDLs.
7. Attach `deployment-policy.json` shaped as `{"nondeployable_programs":["portal"]}`, listing the entire inherited set, not merely Portal. Keep this asset beside the full manifest for every later download. The normal semantic-release asset configuration omits this policy; publishing it is a separately authorized part of exceptional assembly, not a normal pipeline capability.
8. Compare inherited binaries byte-for-byte and inherited manifest entries structurally against their original receipts. Check new IDLs' addresses/version, binary cluster/features, source/deploy ancestry, complete inventory, dependency closure, and policy coverage. Record a public-only provenance table. Private key files never enter staging, artifacts, commits or logs.
   For v3.1.0 specifically, `intent_chainer.dependencies` is `["portal"]`; its seed incorporated the frozen v3.0.0 Portal seed. The final full manifest retained all nine inherited entries. Do not truncate it to the newly built programs: dependency checks and finalization ordering consume those edges.
9. Stop the normal Release workflow from running concurrently with exceptional assembly. Publish only through the exact reviewed and authorized recipe, reusing existing helpers rather than introducing another release pipeline. If no executable reviewed recipe exists, deliver the evidence and missing prerequisite; do not fabricate mixed automation.
10. Handoff only the new deployable names. Deployment, Finalize and Close reject inherited selection, including `all`. Unselected dependencies remain read-only; no inherited key derivation, deploy, init, reserve, IDL or verification write is allowed.

A whole-tree Reproduce run is not a mixed-release validator: its rebuild may differ from carried binaries, and release-version-only inherited IDLs intentionally differ from old canonical on-chain IDLs. Never upload its output over inherited assets to make checks green. Validate inherited provenance separately and reproduce new programs from the reviewed deploy tree.

## v3.1.0 example: inherited receipts, not new verification

The manually composed [published v3.1.0 release](https://github.com/eco/eco-routes-svm/releases/tag/v3.1.0) used a reduced historical inventory, not the normal 41-asset matrix. Its [manifest](https://github.com/eco/eco-routes-svm/releases/download/v3.1.0/program-ids.json) and [deployment policy](https://github.com/eco/eco-routes-svm/releases/download/v3.1.0/deployment-policy.json) are public artifact receipts; the comparisons below are inherited operational evidence, not checks rerun while writing this skill.

| Receipt | Observed historical result |
|---|---|
| Old inventory | 19 assets: nine mainnet binaries, nine mainnet IDLs, one manifest |
| Inherited binaries | All nine mainnet binaries unchanged |
| Inherited IDLs | Only `metadata.version` changed to `3.1.0` |
| Manifest | Nine old entries unchanged, plus `intent_chainer` |
| New assets | Chainer binary and IDL for both mainnet and devnet: four files |
| Policy | Nine inherited programs nondeployable |
| New inventory | 24 assets: old 19 + four chainer files + one policy |
| Chainer ID | `EcoSEcjWKSEG5NqLW3mpzhC3kP4xWNkUEaywJihB5E84` |
| Inherited Portal ID | `EcoN71qfZMZtxwqW1Ps1kzbNKyfpCEd1izZhh58PqsDb` |

Do not fabricate devnet artifacts for historical programs to meet a normal-release count. Derive expected inventory from approved carry-forward provenance, not a memorized number.

The downstream [mainnet deploy run 40784725791](https://github.com/eco/eco-routes-svm/actions/runs/40784725791) deployed Chainer while retaining upgrade authority, but concluded **failure** at Verify (HTTP 502 polling an accepted remote job). Deployment, IDL and cleanup passed; remote verification later completed. Reconcile through the deployment recovery reference rather than re-dispatching for CI color. These are inherited receipts, not a fresh chain check. Its NOT FINALIZED state remains intentional until the test team approves; release success grants no finalization permission.
