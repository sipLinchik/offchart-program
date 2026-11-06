# Security

## Reporting a vulnerability

Report vulnerabilities through GitHub private vulnerability reporting on this repository.

Open a draft advisory here:

[https://github.com/siplinchik/offchart-program/security/advisories/new](https://github.com/siplinchik/offchart-program/security/advisories/new)

Use **Security Advisories** (private vulnerability reporting). Do not file a public GitHub issue for a security report.

There is no separate security email for this repository.

## Scope

The on-chain program in `programs/offchart-lite` is in scope, as deployed on Solana mainnet at `8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P`.

This program has not been audited. A `solana-verify` match attests that the deployed binary was built from a given commit. It is not a security review. The program is upgradeable; the upgrade authority is `4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer`.
