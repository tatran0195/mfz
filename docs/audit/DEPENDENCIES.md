# Dependency/feature assessment

Assessment date: 2026-10-15. Evidence: exact Cargo locks, registry manifests/APIs,
`dependency-scan-initial.json`, `dependency-scan-remediated.json`, full advisories
under `advisories/`, and final root/harness feature trees. OSV is a source of known
advisories, not a proof of exploitability or absence of defects. Aliases are not
counted as independent vulnerabilities.

## Executed results

- Initial exact registry-version scan: **467 package versions, 13 matching package
  entries**. Some matches were maintenance notices or platform/server/Debug-specific
  paths, not demonstrated application exploits.
- Remediated exact-version scan across both locks: **428 distinct versions, two
  matching package entries**, both unmaintained notices (`fxhash`, `paste`). No
  earlier matched security-advisory package versions remain matched by that scan.
- All **416 shared root/harness package-version/checksum pairs align**. No extra
  conflicting version for a shared package name was present in final assertions.
- Final effective root and expanded-harness feature trees are saved in
  `dependency-features-final.txt` and `dependency-features-harness-final.txt`.

No advisory was suppressed to manufacture a clean report. Inactive optional
packages can remain in Cargo.lock; the raw scan retains those matches and actual
feature-tree reachability is described below.

## Changes and rationale

| Original stack/version | Remediation | Reachability/interpretation |
|---|---|---|
| rustls 0.23.39 | 0.23.46, with required aws-lc/webpki compatible updates | Modern AWS HTTPS path. Advisory-specific TLS peer/feature/Debug preconditions vary; no blanket claim that every listed attack was reachable. Patch rather than rely on those assumptions. |
| rustls 0.21.12, webpki 0.101.7, Hyper 0.14/h2 0.3 legacy path | Disable service SDK legacy `rustls` defaults; explicitly keep modern `default-https-client` and Tokio | Redundant old connector was enabled alongside the modern client. This removes the old stack rather than disables TLS. SSO/provider support remains in aws-config. |
| h2 0.4.13 | 0.4.16 | Known empty-DATA resource-exhaustion advisory fixed; real impact depends on negotiated HTTP/2 and peer. No arbitrary Internet server exposure invented. |
| aws-smithy-json 0.62.5 | 0.62.7; minimum required runtime-api 1.12.3 and smithy-types 1.4.9 | Uncontrolled-recursion advisory package patched. Generated-server-specific applicability must not be represented as a demonstrated CLI server exploit. No unrelated major service SDK upgrade. |
| anyhow 1.0.102 | 1.0.103 | Downcast-mut unsoundness patched; no application exploit call established. |
| cmov 0.5.3 | 0.5.4 | Architecture-specific correctness advisory patched in the lock. Windows x64 is the principal runtime; unused encoder feature was subsequently disabled, not claimed an active x64 crypto exploit. |
| crossbeam-epoch 0.9.18 | 0.9.20 | Pointer-formatting advisory patched; exact problematic formatting path was not established in MFZ. |
| rxing default image/imageproc/font/helper stack | Disable wrappers/client/serde defaults; use actual raw Luma8 QR reader with encoding support | MFZ already decodes images into luminance. Actual PNG round trips and full application type-check/build succeeded without imageproc/ttf-parser wrappers. |
| image default AVIF encoder | Explicit existing decoder list + rayon, no `avif` encoder | Registry feature inspection confirms `avif` enabled ravif/rav1e encoding; AVIF decoding required `avif-native`, which was never enabled. MFZ only writes PNG. All previously available decoders retained, not simply restricted to PNG to avoid an advisory. |
| Unused direct SSO/SSOOIDC dependencies | Remove redundant direct declarations | aws-config still includes needed SSO provider SDKs; cached SSO/profile/credential_process support is retained. Initial interactive SSO bootstrap is not implemented by MFZ. |
| Unconditional Windows registry dependency | Target-specific winreg | Root Linux all-target checks now compile actual application modules; unsupported Windows operations return errors rather than fake success. |

Service SDKs use modern HTTPS/Tokio explicitly. The production WebSocket path
still uses native TLS (Schannel on Windows), not a no-TLS transport. No external
AWS CLI/Session Manager Plugin fallback or substitute SDK implementation was added.

## Residual matches — honestly retained

### fxhash 0.2.1 — RUSTSEC-2025-0057, unmaintained

Actual current path: `mfz -> inquire 0.7.5 -> fxhash` (not the QR decoder; see
`fxhash-reachability.txt`). This is local UI data hashing, not a cryptographic key,
AWS signature or a public multi-tenant server map. No concrete runtime exploit
was established. Retained to avoid a speculative UI-library rewrite/fork solely
to hide a maintenance notice. Future compatible upstream migration remains a
maintenance task, not an unresolved confirmed credential bypass.

### paste 1.0.15 — RUSTSEC-2024-0436, unmaintained

The lock retains an optional rav1e/ravif encoder chain, but the final **all-target
feature graph has no active reverse dependency for paste** (`paste-reachability.txt`).
It is a compile-time proc macro even when enabled, not a remotely invoked parser.
The unused AVIF encoder feature was removed for concrete application-use reasons,
not by pretending the raw lock scan has no match.

## Reproduction

```text
python3 tools/audit_dependencies.py
cargo tree --locked --target all -e features
cargo tree --locked -i fxhash
cargo tree --locked --target all -i paste
```

Raw queries use `https://api.osv.dev/v1/querybatch` and full advisory metadata uses
`https://api.osv.dev/v1/vulns/{id}`. Only public crate names/versions are sent, no
source/config/credential material. Network/advisory freshness and inspected
third-party scope remain explicitly limited; no complete third-party formal audit
or permanent zero-vulnerability certification is claimed.
