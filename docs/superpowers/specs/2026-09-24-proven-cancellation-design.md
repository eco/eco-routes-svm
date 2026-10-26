# Proven Cancellation — Design (pointer)

The full cross-VM design lives in `eco-routes` at
`CLAUDE/specs/2026-09-24-proven-cancellation-design.md` (single source of truth).

SVM scope, summarized:

- **Destination:** new `cancel` instruction writes a permanent `FulfillMarker { claimant: CANCELLED }` on the
  existing marker PDA when `route.deadline < now`; `fulfill` rejects `claimant == CANCELLED`. `close_fulfill_marker`
  is removed so the marker is permanent: closing it freed the PDA and let a fulfilled intent be cancelled later.
- **Source:** `Proof` layout and all prover programs unchanged; `withdraw` rejects `claimant == CANCELLED`;
  `refund` allows a proven cancellation immediately and CPIs `close_proof` to return the rent.
- **Client-visible changes:** `docs/proven-cancellation.md`.
- **Shared:** `eco-svm-std::CANCELLED` = 12 zero bytes ‖ the low 20 bytes of `keccak256("eco.portal.intent.cancelled")`
  — the EVM address `0xe685056aEc77686A83E2a6bDf37c6f71dD2fdB5f` left-padded to 32 bytes, golden-pinned to the EVM
  bytes. It is deliberately a valid EVM address, hash-derived so no EVM or Solana key controls it; `withdraw`
  rejects it.
- **Release:** one atomic release; every changed program deploys under a new program ID (derived from its bytecode); minor version. An old-generation source (EVM prover or
  old SVM portal) would treat the sentinel as a payable claimant and a permissionless `withdraw` would burn the
  reward, so every release that changes these programs needs new program IDs / a new EVM root SALT, and whitelists must never cross
  generations.
