# cctp-relay

Reference relayer for `cctp-prover` (E2E and manual relays; the production relayer lives in the solver).

```
export SOLANA_RPC_URL=<rpc url>   # preferred
cctp-relay --keypair <path> receive --source-domain <n> --tx <evm tx> --lookup-table <pubkey> [--dry-run]
cctp-relay --keypair <path> create-lookup-table
cctp-relay --keypair <path> reclaim-event-accounts [--dry-run]
```

The RPC URL is read from `SOLANA_RPC_URL`. The `--rpc-url` flag still works, but prefer the environment variable: a URL passed on the command line usually carries an API key and ends up in shell history and `ps`. The URL is never printed; errors and logs are redacted.

`receive --dry-run` simulates the delivery and exits non-zero if the simulation returns an error.
