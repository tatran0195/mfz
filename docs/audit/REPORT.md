# MFZ repository-wide remediation

**Audit date:** 2026-10-15. **Working branch:** `fix/repository-remediation`.
Upstream HEAD remains `0817c50175cc6c8714776fff9467684ecab98426`; no commits or pushes.
Delivered native-production-v3 tree `668854b1c20b58e175046482481e44b326c96883`
and earlier artifacts are preserved, not relabeled as verification of this work.

## Overall condition

The repository had confirmed application-boundary problems beyond the previously
reviewed native transfer engine: plaintext-secret removal/export, wrong target/
region selection, incomplete-MFA fallback, ambiguous profiles, destructive file/
credential handling and misleading failure/success paths. Corrections are now
implemented through the actual CLI/workflow callers, not only standalone helpers.

Current local evidence is **146 passing actual-module tests (repeated three times),
25 passing real-application offline subprocess checks, two actual Go-smux
interoperability passes, a complete Linux application build and strict Linux
all-target lint/type checks**. Complete Windows-GNU all-target type-check and strict root lint also passed,
including application-boundary/test modules. **No Windows API/MSTSC/live AWS/
affected-VM execution is claimed**. See the exact [verification ledger](VERIFICATION.md).

This is an evidence-backed repository remediation, not a claim that every
conceivable defect has been eliminated or that a local peer proves a Windows VM
will not freeze. The live Windows/AWS acceptance gates remain explicit.

## Important credential-material warning

Two non-placeholder-looking 64-character Base-32 MFA seeds were tracked in the
old `refs/environment.txt`, paired with device ARNs. The same bytes were confirmed
in the v1 transfer-fix, v2 performance and v3 production source ZIPs. **Validity and
revocation are unknown; no credential probing was performed.** If genuine,
replace those MFA devices. A seed compromises the MFA factor, not AWS access by
itself, and source removal is not revocation or erasure of Git/history/archives.

The unused old references were removed; a sanitized `mfz.ini.example` replaces
them. Earlier archives remain byte-for-byte unchanged as required, so do not
redistribute them as sanitized releases. No seed value is reproduced in the new
reports. No unredacted deletion patch is supplied: such a diff would repeat the
removed values. See [MFZ-A26](ISSUES.md) and [exposure metadata](legacy-material-exposure.json).

## Severity-grouped inventory

Detailed baseline behavior/evidence, causes, impact, confidence/likelihood,
locations, callers, remediation and verification/status are maintained in
[ISSUES.md](ISSUES.md), with coverage in [COVERAGE.md](COVERAGE.md).

| Priority | Findings | Current disposition |
|---|---|---|
| High: secret exposure/data integrity/core target/authentication | A01–A12 | Persistence/export validation, endpoint/credential ownership, consistent region/provider/token handling, fail-closed MFA, unique discovery and dependency corrections implemented; local regressions passed, Windows/cloud gates remain where relevant. |
| High conditional material risk | A26 | Material removed from active source; all historical artifacts/history intentionally preserved. Real validity/replacement outside available credentials is unknown. |
| High conditional intermediate-remediation regression | A28 | Windows tempfile/replace ACL risk was caught during remediation, not falsely attributed to upstream direct writes. Private creation/DACL-preserving recovery implemented; native Windows execution still required. |
| Medium: correctness/reliability/local resource ownership | A13–A23, A27 | Parser/config origin, validation, locked atomic edits, backend errors, QR/IAM lifecycle, process/drive/hosts/elevation/export/logging/CI and association ownership corrected. Verification depth is stated per finding, not inferred from implementation. |
| Low/defense-in-depth/quality | A24–A25 | Six previous root lint-category exemptions replaced by local fixes; secret Debug removed/redacted. No observed current whole-object production exfiltration was invented from a derive. |

No critical remote exploit is claimed merely from a pattern or package/advisory
match. Maintenance notices and known unsupported features are not conflated with
confirmed application vulnerabilities.

## Principal implemented corrections

### Credentials, files and profile authority

- Full known-field replacement actually removes plaintext keys; keyring writes are
  verified before removal. Readback/write/lock failures retain meaningful recovery
  state rather than print a false success. Unsupported mock backends cannot claim
  persistent secure storage.
- Locked read-through-update and atomic/no-clobber persistence migrate all Add/
  Edit/Secure callers. Windows secret-bearing temporary files have private security
  **at creation**, not after writing; existing DACLs and partial-replacement recovery
  are handled explicitly. Recovery cleanup failure reports possible old secrets.
- Ordinary exports omit AWS/TOTP/RDP secrets and refuse overwrite. Explicit
  `--include-secrets` and `--force` are separate policies. Config pointers pin the
  selected source; multiple exports cannot overwrite one output filename.
- Exact/deterministic profile parsing rejects malformed fallback/ambiguous sources.
  Explicit `--config mfz.ini` remains explicit (a real subprocess regression first
  reproduced the old fallback). Omitted CLI config alone permits documented search.
- External file reading/referenced paths require consent before reads, then the
  effective AWS/credential/shared-resource operation requires confirmation before
  use. `--trust-profile` is an explicit reviewed automation opt-in, not implied by
  filename/section name. Significant password bytes round-trip without line injection.

### AWS/MFA/discovery and native session

- One cached SDK configuration preserves selected profile, region and provider
  chain across MFA/EC2/SSM. CLI/config override precedence is consistent; non-MFA
  refresh-capable providers are not unnecessarily frozen into static snapshots.
- Session-token absence is typed/converted correctly through SDK and WebSocket
  signing; incomplete MFA fails before provider access rather than downgrade.
- EC2 tag values split once and discovery paginates with a bounded unique-target
  requirement. Shared tags never choose the first arbitrary running instance.
- Native Rust SDK/WebSocket/SSM/smux transport remains. The codec/flow/writer/smux/
  allocation/cleanup hot modules are byte-identical to v3; current audit changes
  affect configuration/ownership and signing credentials, not a speculative
  rewrite or new packet throttle. Native/SDK/actual Go interoperability passed. An alternating same-worker writer microbenchmark also preserved 64 writes/zero duplicates in every sample; raw results are linked in the verification ledger, not claimed as full-system equivalence.
- SSM-only avoids RDP vault dependencies; unexpected native termination is an
  error, with owned shutdown awaited. No unrelated listener/process is killed.

### Windows/RDP/elevation and QR/IAM

- RDP uses its numeric loopback endpoint, an absolute system mstsc, checked output
  generation and an owned cancellable child. Canonical directory/exact DOS target,
  safe profile file and observed credential ownership replace broad blind teardown.
  Existing exact-port credentials are refused, never silently overwritten.
- Hosts updates are explicit, validated and batch-preflighted; retain the real
  inode/ACL, check append/rollback, await/check elevated exit and verify result.
  Elevation sends the validated snapshot, not a mutable config to reread as admin.
- Actual Windows system/Known Folder APIs replace hardcoded Windows/OneDrive paths.
  Shell install completes the command before changing default; uninstall preserves
  unrelated keys/another executable and restores/repairs owned defaults.
- Real URL/Base-32 handling, bounded image input, actual QR PNG/independent TOTP
  round trips and no-clobber saves replace regex/interpolation assumptions.
- IAM pending devices have bounded cleanup; input cancellation/EOF terminates.
  Invalid save targets fail before create; active-device/local-save failure is
  explicitly reported and never automatically deactivates an acknowledged device.
- Logging WorkerGuard is retained/flushed and normal/error exits return ExitCode.
  Actual final diagnostic events were checked after process exit, not assumed.

## Security boundaries and deliberate limits

- IAM is still the cloud authorization boundary; local tag validation is not an
  IAM-policy substitute. Loopback is not a per-user firewall; trust the local
  machine and keep RDP/NLA/server-identity checks. `.rdp` performance hints do not
  prove UDP disabled; no silent machine-wide security/network policy was changed.
- Keyring/temporary RDP credentials are per-user/session resources with checked
  observed ownership, not a transaction against arbitrary noncooperating programs.
  Abrupt kill/runtime destruction/credential expiry can prevent full cleanup.
- Existing output grants are preserved, not promised revoked. Plaintext config,
  explicit secret exports/QR and explicit enrollment `--save` remain intentional
  supported workflows with warnings; migration is not secure erasure of backups.
- AWS SDK SSO/provider support remains; interactive SSO bootstrap is external.
  Unsupported KMS handshake actions fail closed, not silently downgrade or call
  the external AWS CLI/plugin. Supported image decoders were retained.
- No database, frontend, web routes or tenancy schema exists in this application.
  None were fabricated as findings or as an excuse for unnecessary frameworks.

## Removed complexity — with concrete reasons

- Removed unused Python external-CLI launcher/reference config only after caller
  search and nonexecuting syntax/provenance review; original history/baselines remain.
- Removed redundant direct SSO declarations/legacy TLS connector while retaining
  modern HTTPS/SSO/providers, and unused imageproc/font/AVIF-encoder wrappers after
  proving the actual luminance/PNG path compiled and round-tripped without them.
- Removed fake `SsmSession::wait() -> -1`, unsafe non-Windows shell fallbacks and
  machine-specific test assumptions. Kept useful aliases, explicit hosts commands,
  supported formats and genuine pinned/licensed Go interoperability code.
- Did not replace the CLI/transport with new frameworks or keep incorrect internal
  APIs merely for superficial compatibility; all repository callers/tests migrated.

## Dependencies and remaining uncertainty

Known security-advisory versions were updated/removed. The exact OSV rescan matched
only two **maintenance** notices: fxhash (current local inquire UI path) and paste
(inactive optional encoder lock path). Read [DEPENDENCIES.md](DEPENDENCIES.md) for
reachability, exact versions and limitations; no advisory suppression/zero-risk
certification was used.

Live Windows ACL/registry/UAC/Credential Manager/mstsc/DOS behavior, live AWS/IAM/SSO
and the affected guest remain unexecuted. Explicit checks and expected outcomes
are in [WINDOWS_ACCEPTANCE.md](WINDOWS_ACCEPTANCE.md). Remote CI is provided, not
claimed run. [CONTINUATION.md](CONTINUATION.md) preserves any open work/gates.

Use a fresh sanitized source directory rather than overlaying old source ZIPs:
an overlay can leave deleted legacy files behind. No commit/push or historical
artifact modification was performed.
