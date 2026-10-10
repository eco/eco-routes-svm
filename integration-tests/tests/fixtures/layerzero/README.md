# LayerZero endpoint binary for `layerzero_prover_real`

Not committed. Dump the deployed EndpointV2 and point the test at it:

```bash
solana program dump 76y77prsiCMvXMjuoZ5VRrhG5qYBrUMYTE5WgHqgjEn6 "$TMPDIR/lz_endpoint.so" --url devnet
shasum -a 256 "$TMPDIR/lz_endpoint.so"
LZ_ENDPOINT_SO="$TMPDIR/lz_endpoint.so" cargo test --test layerzero_prover_real -- --ignored --nocapture
```

Record the sha256 and date in the PR description when the test is run for a release.
