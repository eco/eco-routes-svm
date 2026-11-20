# Vendored program binaries

Test fixtures that only the integration tests load; none is released or deployed.

| File | Program ID | Source | License |
| --- | --- | --- | --- |
| `helium_treasury_management.so` | `treaf4wWBBty3fHdyBpo35Mz84M8k3heKXmjmi9vFt5` | Helium `treasury-management`, helium/helium-program-library | Apache-2.0, see [LICENSE-helium-program-library](LICENSE-helium-program-library) |
| `helium_circuit_breaker.so` | `circAbx64bbsscPbQzZAUvuXpHqrCe6fLMzc2uKXz9g` | Helium `circuit-breaker`, helium/helium-program-library | Apache-2.0, see [LICENSE-helium-program-library](LICENSE-helium-program-library) |

Both were dumped unmodified from mainnet-beta at slot 471448476. Their digests and refresh steps are in `integration-tests/tests/fixtures/split-venues/README.md`.

`LICENSE-helium-program-library` is the upstream text with trailing whitespace removed.
