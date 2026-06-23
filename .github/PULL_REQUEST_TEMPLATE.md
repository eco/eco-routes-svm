## 🔒 Security attestation (required)

The programs in this repository are deployed on-chain and hold user funds. A public PR
that fixes or reveals a vulnerability in deployed code exposes that bug to attackers
before a fix can ship.

- [ ] This PR does **not** fix or disclose a security vulnerability in already-deployed code.

> If it does: **close this PR and do not push the branch.** Report privately via the
> [Security tab → "Report a vulnerability"](https://github.com/eco/eco-routes-svm/security).
> See [`SECURITY.md`](../SECURITY.md). This applies to humans and AI agents alike.

---

## Description

<!-- Provide a brief description of the changes in this PR -->

## Checklist

- [ ] Code formatted (`cargo +nightly fmt`)
- [ ] Imports sorted (`cargo sort --workspace`)
- [ ] Clippy warnings resolved (`cargo clippy --all-targets -- -D warnings`)
- [ ] All tests passing (`anchor test`)
- [ ] Golden files updated if needed (`GOLDIE_UPDATE=1 cargo test`)
- [ ] Documentation updated for API changes
