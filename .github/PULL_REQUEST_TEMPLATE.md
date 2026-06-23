## 🔒 Security attestation (required)

The programs in this repository are deployed on-chain and hold user funds. A public PR
that fixes or reveals a vulnerability in deployed code exposes that bug to attackers
before a fix can ship.

Confirm one of the following (deployment status can be hard to judge — when unsure,
treat it as deployed and disclose privately):

- [ ] This PR is **not** a security fix, **or**
- [ ] This is a security fix and a maintainer has **confirmed the affected code is not deployed on-chain** (and is not about to be), **or**
- [ ] This is the public merge of a fix coordinated via a private advisory, and the on-chain mitigation is **already deployed and verified live** (see [`SECURITY.md`](../SECURITY.md) → "Coordinated fix and disclosure").

> If this is a security fix for deployed code that is **not** yet mitigated on-chain:
> **close this PR and do not push the branch.** Report privately via the
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
