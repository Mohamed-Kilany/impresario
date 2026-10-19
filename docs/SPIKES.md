# Feasibility spikes — TASK-2 phase 0

Time-boxed spikes for the capabilities TASK-2 §6/§7 must replace or build. Every
spike closes with a **go/no-go**, the **ladder rung** it lands on (§6.0) and the
`I-nn` decision that records it. A failed spike triggers the fallback ladder,
never a silent shortcut.

Rules that apply to every entry here:

- **Facts are checked, not recalled** (§1.3): protocol details come from the
  reference implementation or the crate source, with a citable location, not
  from memory. Where a claim was *not* checked, the entry says so.
- **No unverified claims** (§1.2): a capability is "verified" only where it was
  actually exercised on this host's hardware. Everything else says `unverified`
  and carries a hardware runbook or an `#[ignore]` test in the work package.
- **Licenses** (§1.4): every crate/payload is recorded in `docs/THIRD_PARTY.md`
  before it is used; GPL/LGPL code may be studied for protocol facts, never copied.
- The inquiry was made *before* the implementation that depends on it, per §1.1.

Vocabulary in the entries: **verified** = produced by a command run on this host
or read in a file whose path is cited; **reported** = stated by a prior-art
project/agent research whose artefact exists but which was not re-run here;
**unverified** = no evidence yet, tests are hardware-gated.

## Matrix

| ID | Capability (spec) | Status | Verdict | Rung | Decision |
|---|---|---|---|---|---|
| S-1 | Android without `adb` (§6.1) | open | — | — | — |
| S-2 | iOS/tvOS without Xcode tools (§6.2) | settled | **go** with recorded constraints (no direct-USB in `idevice`; iOS 17+ tunnel needs OS plumbing; XCTest end-to-end unverified) | 1 | I-95 |
| S-3 | webOS without `ares-*` (§6.3) | open | — | — | — |
| S-4 | Tizen without `sdb`/`tizen` CLI (§6.4) | settled | **go** for the Rust sdb client + fixture mock; hardware validation pending | 1 | I-93 |
| S-5 | CLI/TUI/daemon/IPC/lossless config (§3, §4.2, §3.5) | settled | **go** — 26/26 probe checks pass on Windows; config **stays YAML** | 1 | I-91, I-92 |
| S-6 | Apple signing sources (§7.1) | settled | **go** for (a) local `.p12` re-sign (verified on Windows), **conditional go** for (b) ASC API and (c) free-Apple-ID sideloading (external services) | 1/3 | I-96 |
| S-7 | Tizen certificates (§7.3) | settled | **go** for import/inspection; creation needs Samsung account services (external, optional) | 1/3 | I-94 |
| S-8 | Standalone editor without LSP (§9.2) | open | — | — | — |

## Method

- **Crate probes** run in a throwaway workspace outside the repo
  (`%TEMP%\opencode\spike-…`), pinned to the version being evaluated; the exact
  command and observed output land in the spike entry.
- **Source facts** are quoted with `file:line` (or a URL path) from a checked-out
  dependency or from the upstream repository of a reference implementation.
- **Hardware probes** run only on devices reachable from this host, and are
  recorded with the evidence the run produced (version strings, timings, raw
  output). Unavailable hardware means the spike verdict is scoped down and the
  remaining steps become `#[ignore]` tests + a runbook in `docs/DEVICES.md`.
- Time-box per spike: one working session. Hitting the box without an answer is
  itself a result ("not settled here") and is recorded as such.

---

## S-2 · iOS/tvOS without Xcode tools

**Verdict: go** (rung 1 for everything the crate covers; rung 3 for the one host
service we cannot ship). Probe artefacts: `%TEMP%\opencode\spike-apple\`
(`src-idevice`, `src-apple-platform-rs`, `codesign-spike`, `src-go-ios`,
`src-pymobiledevice3`, `src-tidevice`, `REPORT.md`).

**Dependency.** `idevice` 0.1.68 (MIT). Verified here:

- The crate builds on Windows 11 (both with default features — which pull the C
  dependency `aws-lc-sys`, built with the VS BuildTools present on this host —
  and with `--no-default-features --features rustcrypto,usbmuxd,tcp,pair,afc,
  installation_proxy,mobile_image_mounter,syslog_relay,crashreportcopymobile`).
  The rustcrypto build has **zero C dependencies** but pins
  `rustls-rustcrypto 0.0.2-alpha`; that is a risk to record, not hide.
- Service inventory (read from `src-idevice/idevice/src/`): `afc`,
  `installation_proxy`, `installcoordination_proxy`, `house_arrest`,
  `springboardservices`, `screenshotr`, `syslog_relay`, `os_trace_relay`,
  `crashreportcopymobile`, `misagent`, `mobile_image_mounter`, `pcapd`,
  `preboard_service`, `diagnostics_relay`, `restore_service`, `core_device`
  (file/hid/pasteboard/orientation/screencapture/stdiosocket/location/icon),
  `core_device_proxy`, `rsd`, `tunnel.rs`/`tunneld.rs`, `remote_pairing`
  (TLS‑PSK + lockdown + opack), `xpc`, `tss`, `wda`/`wda_bridge`, and
  `dvt/*` (`process_control`, `device_info`, `energy_monitor`, `graphics`,
  `application_listing`, `location_simulation`, `condition_inducer`) incl. the
  testmanagerd/XCTest control path.
- **No direct-USB transport in the library**: `idevice/src/usbmuxd/mod.rs`
  is a *client* of a usbmuxd daemon (non-unix default TCP `127.0.0.1:27015`);
  the crate has no `rusb`/`nusb` dependency (grep of
  `src-idevice/idevice/Cargo.toml`; `nusb` appears only in the repo's separate
  `tools` crate). On Windows the daemon is Apple's Mobile Device Service →
  classified as a **tolerated host prerequisite** (D-19), reported by
  `impresario doctor`. `rusbmux` 0.3.0 (MIT OR Apache-2.0, 190 downloads,
  pure-Rust nusb) is a *candidate replacement*, but it is young and needs
  hardware validation — not adopted now.
- **iOS 17+ tunnel constraint (reported, not verified)**: prior art documents
  that a kernel-routable tunnel is needed for iOS ≥17 and that Windows
  17.0–17.3.1 needs extra drivers/root (`go-ios` README; pymobiledevice3
  `docs/guides/ios17-tunnels.md`). Our stack must therefore treat "device on
  iOS ≥17" as a first-class case with an explicit doctor check and a runbook,
  and must not pretend the tunnel is a detail. This is the item a Mac + device
  must settle before any Apple work package can be called verified.

**Running XCTest without `xcodebuild`.** The technique is portable in principle
(prebuilt runner `.app` + `.xctest`, install via installation_proxy, drive
testmanagerd over DVT/DTX, RSD tunnel on iOS ≥17, development-signed with
`get-task-allow`); it is implemented by `go-ios` (MIT), `pymobiledevice3`
(GPL-3.0, facts only) and, unpaused-not, `tidevice` (MIT). **End-to-end from
Windows is unverified here** and is the top Apple risk.

---

## S-4 · Tizen without `sdb`/`tizen`

**Verdict: go** for a pure-Rust sdb client and `.wgt` re-signer, tested against
byte fixtures and an in-process mock device; real-TV validation is hardware-gated
(no Tizen TV on this host).

**Transport (all verified first-hand from source):**

- TCP to `<tv>:26101`, no host daemon: `TizenAppInstallerCli/SdbClient/SdbTcpDevice.cs`
  (`int port = 26101`), in `github.com/elfpie/tizen-app-installer-cli`
  (master branch, **no license file** → facts only).
- Framing is ADB's: 24-byte header, 6 × u32 LE
  `{command, arg0, arg1, len, checksum, magic}`, `magic = command ^ 0xFFFFFFFF`;
  commands `CNXN 0x4E584E43`, `AUTH 0x48545541`, `OPEN 0x4E45504F`,
  `OKAY 0x59414B4F`, `WRTE 0x45545257`, `CLSE 0x45534C43`
  (`.../SdbClient/SdbCommand.cs`; `.../SdbClient/SdbFrame.cs`).
- Handshake: client `CNXN{arg0=0x01000000, arg1=maxdata 4096, payload="host::sdb-net-client"}`;
  device replies `CNXN` (negotiated maxdata in arg1, banner in payload); a device
  `AUTH` is not handled by the reference client and is on our open list.
- Services (OPEN payloads, NUL-terminated): `shell:<cmd>`, `capability:`,
  `sync:`. Channel ids: OKAY/WRTE/CLSE carry `arg0=remoteId, arg1=localId` from
  the device side; WRTE must be acknowledged with OKAY.
- `capability:` reply starts with a 2-byte hex length prefix that must be
  stripped, then `key: value` lines; keys used: `platform_version`,
  `sdk_toolpath`, `secure_protocol`, `product_version`.
- `sync:` push: `SEND|DATA|DONE` + 4-byte ASCII id + u32 LE length; DATA ≤64 KiB;
  DONE carries mtime; replies `OKAY`/`FAIL`.
- Upstream: `github.com/TizenTeam/sdb` (Apache-2.0 verified) is the sdb source
  (an adb fork — its `OVERVIEW.TXT` says "The Android Debug Bridge (SDB)").

**Device operations (verified from `TizenAppInstallerCli/TizenInstaller.cs`):**

- Install = push to `{sdk_toolpath}/app.wgt` (default
  `/home/owner/share/tmp/sdk_tools/tmp`) then
  `shell "0 vd_appinstall <packageId> <remotePath>"`; progress lines contain
  `[NN]`. Uninstall `0 vd_appuninstall <id>`, list `0 vd_applist`,
  DUID `shell "0 getduid"`.
- The reference installer re-signs **only when `platform_version >= 7.0`** and
  installs older packages as-is. This matches community reports
  (TizenBrew/Samsung tooling) and is the reason our build step must be
  version-aware rather than always-resign.
- Debug launch (reported, not re-verified): `app_launcher -w -s <appId>` prints
  `port: <n>`, then a local forward exposes the CDP endpoint
  (`Samsung/tizen-agent-skills`, Apache-2.0, `tizen-action-skills/common/…`).
  The Samsung repo layout differs from the research paths; our citations use the
  verified prefix `tizen-action-skills/`.

**Open (hardware-gated):** whether the TV ever issues AUTH; whether a locally
self-signed distributor certificate is accepted (Samsung's own tooling says real
devices need a Samsung-issued, DUID-bound profile); exact `vd_appinstall`
exit/progress semantics; the `capability` prefix across firmware generations.

---

## S-5 · CLI / TUI / daemon / IPC / lossless config

**Verdict: go** — rung 1 (own CLI/TUI/daemon over pinned crates, no external
tools). Probe project: `%TEMP%\opencode\spike-cli` (throwaway). All checks below
were **run on this Windows host**, 26/26 PASS:

| Probe | Result |
|---|---|
| `yaml-edit` 0.3.2 (Apache-2.0) no-op round trip | byte-identical, LF **and** CRLF |
| `yaml-edit` path read/set/remove | value changed, **every other byte identical** (comments, ordering, quoting, stream-level header comment) |
| `yaml-edit` invalid YAML | structured error with line/column, no panic |
| `toml_edit` 0.25.17 round trip / set / remove | byte-identical no-op; comments/tables intact (kept as the fallback) |
| `ratatui` 0.30.2 + `TestBackend` | headless render, deterministic second draw, `assert_buffer_lines` snapshot works |
| `clap` 4.6.7 + `clap_complete` 4.6.11 | completions generated for bash/zsh/fish/powershell/elvish |
| `portable-pty` 0.9.0 (ConPTY) | child output captured on Windows |
| `interprocess` 2.4.6 local socket | named-pipe round trip on Windows |
| `keyring` 4.2.0 | Windows Credential Manager set/get/delete round trip |
| `unicode-width` 0.2.2 | CJK=4, latin=2 columns |

**Two gotchas worth remembering (they cost probe iterations):**

1. *YAML lossless edits must serialize the `YamlFile`, not the `Document`.*
   Stream-level text (a file's leading comment) is a sibling of the document
   node; `Document::to_string()` drops it. Pattern: `YamlFile::from_str` →
   edit via `file.document()` → write `file.to_string()`. The probe asserts
   every-other-byte identity against this pattern.
2. *ConPTY asks the terminal for the cursor position at startup* (`ESC[6n`) and
   withholds the child's output until the answer (`ESC[1;1R`) arrives. PTY
   tests must answer it.

**Config format decision:** because `yaml-edit` passes the lossless test, the
config format **stays YAML** (D-04 unchanged) and no `migrate`-to-TOML is
needed; `toml_edit` remains the recorded fallback should `yaml-edit` rot
(recorded in I-91).

**Not probed here:** `inquire`/`cliclack` (compiled only; prompt behaviour is
tested in the work package via the step-model abstraction), `indicatif`.
Daemon supervision and job control are Phase-4 work; the IPC primitive is proven.

---

## S-6 · Apple signing sources

**Verdict:** (a) **go**, verified locally; (b) **conditional go** (crate exists,
no account to exercise it); (c) **conditional go, opt-in** (external service
dependency, ToS-grey — the user explicitly approved "with clear warnings").

- **(a) Local import + re-sign — verified on Windows.** `apple-codesign`
  0.29.0 (**MPL-2.0** — weak copyleft; used unmodified as a dependency, flagged
  in `THIRD_PARTY.md` per §1.4) built `rcodesign.exe` (38,338,560 bytes) and
  signed artefacts now on disk in `spike-apple\codesign-spike\`:
  `exe.signed`, `exe.ent.signed` (entitlements incl. `get-task-allow`), and a
  full `.app` bundle with `_CodeSignature/CodeResources`. Reported (artefact
  fixtures in `src-apple-platform-rs`): the cdhash matches the upstream
  trycmd fixture. **Gap verified by search:** the crate has **no
  provisioning-profile handling** (`embedded.mobileprovision`); the profile must
  be placed by us, and profile↔entitlements consistency is our job.
- **(b) App Store Connect API — conditional.** `app-store-connect` 0.7.0
  (Apache-2.0 OR MIT, 304k downloads) exists with device/bundle/certificate/
  profile/notary APIs. No paid account here → **no live call has been made**;
  the work package must build it behind a "not verifiable here" gate.
- **(c) Free-Apple-ID sideloading — conditional, opt-in.** Requires Apple's auth
  infra (anisette data from an external service) on top of crypto; prior art
  `isideload` 0.5.0 (MIT) depends on `idevice` and is the closest reference.
  Documented limits: 3 apps incl. itself, 10 App IDs/week, profiles/devices
  expire after 7 days. This never becomes a silent default: it is an explicit
  source, off by default, with warnings (approval recorded in I-96).

---

## S-7 · Tizen certificates

**Verdict:** import/inspection **go**; creation **conditional** (Samsung account
services, external). Evidence:

- Distributor certificates bind devices through **URI SANs**:
  `URN:tizen:packageid=` (first entry) then one `URN:tizen:deviceid=<DUID>` per
  device, subject `CN=TizenSDK` (+ email RDN), RSA-2048 `SHA512withRSA` CSR,
  **max 50 DUIDs**, and PKCS#12 must use the **legacy PBE-SHA1-3DES** encoding
  ("Tizen Studio's Java signer rejects modern AES/PBKDF2 PKCS#12 files") —
  verified in `github.com/gifflet/tizen-cert-cli` `internal/pki/pki.go`
  (MIT-licensed reference for our own implementation; study + reimplement, not
  copy).
- `.wgt` signature scheme (verified verbatim in
  `TizenAppInstallerCli/SigningManager/TizenResign.cs`): two files at archive
  root, `author-signature.xml` and `signature1.xml`; XML-DSig with
  `SignatureMethod = rsa-sha512`,
  `CanonicalizationMethod = exc-c14n`,
  `DigestMethod = sha512` per file over percent-encoded entry names (no
  transforms), plus one `URI="#prop"` reference whose digest is a **fixed
  constant per role** with `Transform = xml-c14n11`; `<SignatureValue>` = RSA
  PKCS#1 v1.5 SHA-512 over exclusive-C14N(`SignedInfo`), base64 wrapped at 76
  chars; `<KeyInfo><X509Data>` carries the whole chain; `<Object Id="prop">`
  carries the W3C widgets-digsig profile/role; the distributor signs the file
  list **including** the author signature.
- The fixed `#prop` digests double as a free oracle for our exclusive-C14N
  implementation — the single fiddly part of the signer.
- Creation of a *Samsung-issued* distributor certificate is a browser OAuth flow
  against Samsung account services (external by nature). Plan: always support
  import + inspection + self-generated author flow; offer the account flow as a
  clearly-marked external step; never silently degrade to an unsigned package.
- Hardware-gated unknowns: what the TV validates on install (self-signed
  distributor acceptance per firmware), and whether older firmware digests differ.

---

## S-1 · Android without `adb`

*Open — protocol facts and the live probe are the next work item.*

## S-3 · webOS without `ares-*`

*Open — research in flight.*

## S-8 · Standalone editor (WASM)

*Open — `cargo build -p impresario-core --target wasm32-unknown-unknown` not yet run.*
