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
- **Shared:** `eco-svm-std::CANCELLED` = `keccak256("eco.portal.intent.cancelled")`, golden-pinned to the EVM bytes.
- **Release:** one atomic release under new program IDs; minor version.
