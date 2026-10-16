# Durable continuation / acceptance checklist

## Current state — 2026-10-15

Repository-wide local remediation/verification pass is complete for this source
snapshot; **live Windows/AWS/affected-VM acceptance is NOT complete**. No claim that
every conceivable defect is gone. Branch `fix/repository-remediation`, upstream
HEAD `0817c50175cc6c8714776fff9467684ecab98426`, no commits or pushes. Original v3
staged/source tree `668854b1c20b58e175046482481e44b326c96883` and all earlier artifacts
are preserved; do not silently alter their provenance.

Read `REPORT.md`, `ISSUES.md`/`ISSUES.json`, `VERIFICATION.md`, `COVERAGE.md`,
`DEPENDENCIES.md` and `WINDOWS_ACCEPTANCE.md` before continuing. Exact durable
logs are included in `evidence/`; larger baseline material is separately retained
in `/home/user/mfz-audit-baseline/`.

## Completed implementation / local gates

- [x] Full-field secret removal and verified migration, correct literal/quoted INI,
  strict model/source selection, exact explicit path vs omitted CLI discovery.
- [x] Locked read/update + atomic/no-clobber persistence; all main Add/Edit/Secure
  callers migrated. Private Windows creation/DACL-preserving recovery implemented.
- [x] Secret-free ordinary export, explicit secret/force policy, exact pointer source,
  CLI conflicts/plan validation and actual log flushing/ExitCode ownership.
- [x] Numeric-loopback RDP, absolute system mstsc, owned async child and scoped
  credentials, exact canonical DOS mapping teardown, checked platform errors.
- [x] Explicit hosts updates only; validated complete batches, inode/ACL retention,
  checked UAC result/postcondition and immutable validated child arguments.
- [x] Cached selected-profile SDK config/region/provider chain, refresh-capable
  non-MFA providers, optional tokens throughout signing, fail-closed incomplete
  MFA and bounded unique paginated EC2 selection.
- [x] Unsupported mock keyring persistence refused; normalized identity/fresh
  readback/checked deletion. Real Windows test is opt-in and disposable.
- [x] Actual QR/URI/Base32/image limits/no-clobber; IAM pending lifecycle/EOF and
  truthful active-device/save-failure outcome. No fabricated placeholder config.
- [x] Association install/uninstall ownership and prior/default/orphan state,
  preserving unrelated registry data; no UserChoice bypass.
- [x] Remove unused unsafe legacy references only after caller/provenance review;
  retain actual pinned/licensed Go interoperability source and native Rust runtime.
- [x] Compatible security dependency updates/feature pruning; 428-version OSV scan,
  416 shared version/checksum alignment, explicit two maintenance-notice residuals.
- [x] Original/expanded actual-module suite: **146 passed**, three final repeats.
- [x] Real application build and **25 actual offline CLI subprocess passes**.
- [x] Complete root Linux AND Windows-GNU all-target type-check/strict Clippy,
  expanded-harness strict lint, stable fmt/whitespace, CI syntax/command validation.
- [x] Actual Go peer build/vet/frozen hashes and **two interoperability passes**.
- [x] Release writer benchmark and alternating same-worker v3/current 3× matrix:
  4 MiB, RTT 0/50/250/500; 64 writes / zero duplicates in every sample. No TLS/AWS/
  RDP/guest performance claim; raw data and method limits retained.
- [x] No old lint-category command overrides/new blanket suppression or weakened
  assertions. Original 0/16 red and 8/8 intermediate evidence retained honestly.
- [x] Both temporary swap files disabled/deleted; no build/watch/server is needed.
  Rust/Go/MinGW/cache processes are nondurable and may need reinstall next session.

## Important material/provenance constraint

MFZ-A26 found two seed-like Base-32 values paired with MFA ARNs in the removed
legacy config, with the same bytes in all three historical source ZIPs. **Validity
and revocation are unknown**; no live testing of them occurred. If genuine, replace
the corresponding MFA devices. Earlier archives/history/baseline snapshots remain
unchanged by instruction and are NOT sanitized releases. See metadata-only
`legacy-material-exposure.json` and exact `preserved-artifacts.json` hashes.

**Never publish an unredacted git deletion diff for refs/environment.txt**: it would
repeat the removed seed values. New source packaging excludes Git/history/cache/
baselines and uses a fresh directory; overlaying old files can leave obsolete
references behind. Known-value scans intentionally do not print/store values.

## Next work, if Windows/cloud access becomes available

1. Run the exact disposable Windows/MSVC commands and expected outcomes in
   `WINDOWS_ACCEPTANCE.md`; record actual exit/state, not a cross-compile as runtime.
2. Exercise DACL/private creation/ReplaceFile recovery/backup cleanup on real NTFS;
   ensure no ACL broadening or destroyed only copy on partial failure.
3. Test actual Credential Manager persistence, pre-existing/changed exact-port and
   host-only mstsc credentials, child cancellation/status and DOS mapping races.
   Do not touch real user entries or unrelated drive mappings.
4. Test UAC cancellation/failure/timeout/postcondition and registry reassignment/
   prior-default/orphan cleanup in a disposable user/VM, preserving unrelated data.
5. Test live intended account/profile/region/SSO, pagination/uniqueness, IAM/MFA
   pending cleanup and active-device/local-save failure, plus expired/denied/
   interrupted session termination. No implicit CLI/plugin fallback or downgrade.
6. Reproduce clipboard AND redirected-drive transfers with independent whole-VM
   responsiveness/CPU/commit/disk/DPC/network observations. Local writer/Go cases
   are not proof of guest-kernel behavior.
7. Run the supplied expanded CI remotely only with explicit authorization; no
   commit/push or remote CI action was performed in this audit.
8. Maintenance: evaluate future compatible inquire replacement for fxhash notice;
   keep inactive paste notice visible while optional lock metadata retains it.
   Requery advisory freshness when dependencies/time change, do not suppress it.

## Standing constraints

- Keep entirely native Rust AWS SDK/WebSocket/SSM/smux; no required AWS CLI or
  Session Manager Plugin. Optional SDK credential_process/SSO setup is distinct.
- Preserve valid sharing/profile/QR functionality; do not solve hangs by disabling
  clipboard/drives, weakening NLA/identity checks or changing machine UDP policy.
- Keep issue/coverage/evidence current and distinguish confirmed defects, conditional
  risks, trade-offs and optional improvements. Do not invent database/web/tenant
  components or claim every possible problem eliminated.
- Explicit plaintext config/QR/enrollment save is supported with warnings and
  verified migration; existing destination grants are preserved, not all revoked.
  Sudden kill/power loss/noncooperating outside modifications have honest limits.
- No commits/pushes unless the user explicitly revises that instruction.
