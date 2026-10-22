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
| S-1 | Android without `adb` (§6.1) | settled | **go** - shipped on `libadb` 0.6.0 (MIT): USB/TCP/TLS/SPAKE2 pairing, shell v2, install/instrument, forwarding, logcat, screencap; live phone/TV with `adb` absent; USB data implemented but unverified | 1 | I-121/I-122 |
| S-2 | iOS/tvOS without Xcode tools (§6.2) | settled | **go** with recorded constraints (no direct-USB in `idevice`; iOS 17+ tunnel needs OS plumbing; XCTest end-to-end unverified) | 1 | I-95 |
| S-3 | webOS without `ares-*` (§6.3) | settled | **go** — `russh`/`russh-sftp` build and pass a loopback probe on Windows; protocol facts cited from ares-cli 2.4.0/ares-cli-rs; no webOS TV to validate against | 1 | I-99 |
| S-4 | Tizen without `sdb`/`tizen` CLI (§6.4) | settled | **go** for the Rust sdb client + fixture mock; hardware validation pending | 1 | I-93 |
| S-5 | CLI/TUI/daemon/IPC/lossless config (§3, §4.2, §3.5) | settled, implemented in Phase 4 | **go** — 26/26 probe checks pass on Windows; config **stays YAML**; the TUI, daemon, PTY harness and profiles shipped | 1 | I-91, I-92, I-125…I-130 |
| S-6 | Apple signing sources (§7.1) | settled | **go** for (a) local `.p12` re-sign (verified on Windows), **conditional go** for (b) ASC API and (c) free-Apple-ID sideloading (external services) | 1/3 | I-96 |
| S-7 | Tizen certificates (§7.3) | settled | **go** for import/inspection; creation needs Samsung account services (external, optional) | 1/3 | I-94 |
| S-8 | Standalone editor without LSP (§9.2) | settled | **go** — `impresario-core` builds unchanged for `wasm32-unknown-unknown` | 1 | I-98 |

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

**Phase 4 outcome (2026-10-21; landing-first UX 2026-10-22).** The probe
choices carried into the product:

- `ratatui` 0.30.2 + `crossterm` 0.29 drive the landing-first TUI (colorful
  `IMPRESARIO` wordmark and workspace picker, then the workspace menu and
  home/runs/tests/devices/apps/config); `TestBackend` + `insta` cover eleven
  frames (`src/tui/snapshots.rs`), and the TUI remains a client that spawns
  the real CLI (I-125, I-131).
- `portable-pty` 0.9.0 (dev-dependency) drives the real binary through ConPTY:
  first-run scaffold, slash completion + help, and run view with
  `MockAgent` → detach → close → attach → report (`tests/tui_pty.rs`, I-128).
  The probe's `ESC[6n]` warning was necessary but not sufficient: the PTY
  master must stay alive and UTF-8 tails must be trimmed on a character
  boundary.
- `interprocess` 2.4.6 local sockets back the auto-started per-home daemon.
  Job/lease state is file-based under `~/.impresario/jobs`, the socket name is
  home-scoped, the token is private, and idle exit, recovery, all three
  `--on-busy` policies and the Windows pipe-release regression are covered
  (`tests/jobs_cli.rs`, I-126).
- `clap_complete` 4.6.11 now ships as `impresario completions <shell>`.
- `yaml-edit` 0.3.2 passed the scalar no-op probe but mis-indents a block
  mapping inserted under an existing block key, so profile saving uses a
  validated text splice for the `profiles:` section; every other config edit
  still uses `yaml-edit` (I-127).
- The workspace MSRV moved from 1.87 to **1.88** because `ratatui` 0.30.2
  requires it.

Live Phase 4 evidence is the detached four-device run
`20261021-080459-ec51` (4 devices / 4 plays / 6 acts / 189 steps: web, phone,
Android TV, LG webOS) followed by `attach`/`report`, plus the focused web
detach run `20261021-073144-2763` (9/9) and the LG-only diagnosis run
`20261021-080418-843e` (2/2). The Windows Terminal/PowerShell 7, PowerShell
5.1 and cmd.exe checklist was executed; STATUS gaps 17–18 are closed.

**Not probed or not adopted:** `inquire`/`cliclack` were compiled in the probe
but the wizard/prompt is line-based, `indicatif` was not needed, and `keyring`
is not used yet (no secret store path landed in Phase 4). Those crates stay
candidates in `docs/THIRD_PARTY.md`; the daemon/IPC and job-control gaps the
spike left open are now implemented (I-126).

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

**Verdict: go** — rung 1. **Phase 3 shipped this on `libadb` 0.6.0, not the `adb_client` +
own-pairing plan below; the original spike analysis is retained as history and
the outcome is at the end of this section (I-121).**

**Transport + commands.** `adb_client` 3.2.3 (MIT), used through
`message_devices` (direct protocol, **no adb server**): TCP with `STLS` → TLS
and adb-key certificate, token signing (`ADBRsaKey`, `read_adb_private_key`),
AUTH, shell, sync push/pull, install/uninstall, framebuffer/screencap,
reboot/root/verity/stat; mDNS discovery behind the `mdns` feature
(`_adb-tls-connect._tcp.local.`); USB behind the `usb` feature (rusb). Its
`server`/`server_device` modules (adb server on 5037) are **not** used —
D-19 forbids the external daemon, and the direct client is the compliance path.
The direct client does **not** pair: `pair.rs` lives under `server/commands/`
(i.e. it is a wrapper around `host:pair` on an adb server).

**Pairing (implemented by us), verified first-hand from
`aosp-mirror`/`android.googlesource.com` `platform/packages/modules/adb`
(Apache-2.0) and BoringSSL (Apache-2.0):**

- Discovery: `_adb-tls-pairing._tcp` and `_adb-tls-connect._tcp`
  (`adb_mdns.h`).
- TLS: **TLS 1.3 only** (`tls_connection.cpp`: min = max = `TLS1_3_VERSION`),
  self-signed cert on both sides, peer verification **accepts any certificate**
  (`SetCertVerifyCallback([] { return 1; })` in `pairing_connection.cpp`);
  64 bytes of exported keying material with label `"adb-label"` **including
  its NUL** (`sizeof(kExportedKeyLabel) == 10`, `SSL_export_keying_material(...,
  kExportedKeyLabel, sizeof(kExportedKeyLabel), nullptr, 0, false)`) are
  **appended to the pairing password**.
- SPAKE2: BoringSSL `SPAKE2_CTX` = **draft-irtf-cfrg-spake2-02**, roles
  `alice` (client) / `bob` (server), names `"adb pair client\0"` /
  `"adb pair server\0"` **including NUL** (AOSP passes `sizeof`, not `strlen`),
  SHA-512, 32-byte messages, 64-byte key. Transcript =
  `len64(my_name) || my_name || len64(their_name) || their_name ||
  len64(my_msg) || my_msg || len64(their_msg) || their_msg ||
  len64(dh_shared) || dh_shared || len64(password_hash) || password_hash`
  (u64 little-endian lengths; order reverses for Bob), with
  `password_hash = SHA-512(password)` and
  `password_scalar = reduce(SHA-512(password))`; the **password scalar hack**
  adds `l`, `2l`, `4l` (wrapping 32-byte adds) until the low 3 bits are zero so
  torsion cancels. `M`/`N` are fixed Edwards points derived from the SHA-256
  seeds `"edwards25519 point generation seed (M)"` / `"(N)"`
  (`spake25519.cc` lines 40-113; encoded N =
  `10e3df0ae37d8e7a99b5fe74b44672103dbddcbd06af680d71329a11693bc778`,
  M = `5ada7e4bf6ddd9adb6626d32131c6b5c51a1e347a3478f53cfcf441b88eed12e`).
- Cipher: AES-128-GCM. Key = `HKDF-SHA256(ikm = SPAKE2 key,
  info = "adb pairing_auth aes-128-gcm key")`; nonce = 12 zero bytes with a
  **u64 little-endian sequence counter in the first 8**, separate counters for
  encrypt and decrypt, both starting at 0; 16-byte tag appended.
- Framing: `PairingPacketHeader { u8 version = 1, u8 type, u32 payload_len
  (LE) }`; types `SPAKE2_MSG = 0`, `PEER_INFO = 1`; payload cap
  `kMaxPeerInfoSize * 2`.
- PeerInfo: `{ u8 type; u8 data[8191]; }` = **8192 bytes**;
  `ADB_RSA_PUB_KEY = 0`, `ADB_DEVICE_GUID = 1`; exchanged encrypted; the client
  sends its ADB public key in `data`.
- Password bytes: the 6-digit code as ASCII (AOSP's own tests pass raw password
  bytes and append the exporter in `SetupTlsConnection`); the exact NUL
  convention for the code is a hardware-verified detail, not assumed.

**Crate incompatibility (verified, and the reason this is our code):**
RustCrypto `spake2` 0.4.0 implements **RFC 9382** (SHA-256, 6 × 32-byte
transcript, HKDF-SHA256 hash-to-scalar), while BoringSSL's `spake25519` is
draft-02 with a SHA-512 length-prefixed transcript and the scalar hack. They
are **not wire-compatible**; a pairing attempt would fail cleanly but not work.
Plan: port BoringSSL's protocol logic (~200 lines) onto `curve25519-dalek`
5.0 (BSD-3-Clause), `sha2`, `hkdf`, `aes-gcm`, and validate against the phone.
Fallback if point/scalar edge cases do not line up: call BoringSSL's
`SPAKE2_*` through `boring-sys` (the exact reference implementation; a C build
dependency like aws-lc, licence-clean).
TLS for the pairing channel: `rustls`/`tokio-rustls` with a verifier that
accepts any peer (authenticity comes from SPAKE2), certificate via `rcgen`.

**Hardware-gated (phone only, recorded in `docs/STATUS.md`):** end-to-end
pairing, connect after pairing, `am instrument`, logcat, screencap, install,
USB on Windows (driver), mDNS on the user's LAN. `adb_client`'s TCP/TLS path
and token signing were read in source but have not been run against a device
on this host.

**Phase 3 outcome (shipped; supersedes the plan above).** The implementation
does not use `adb_client` or our SPAKE2 port: `libadb` 0.6.0 (MIT) provides
TCP/TLS, the USB `nusb` backend and Android 11+ pairing (TLS 1.3 + SPAKE2 +
PeerInfo) behind direct host APIs, with no adb server or adb binary. The
`AdbTransport` uses it for USB/TCP/mDNS discovery, key auth, shell v2/exec,
sync install, instrumentation streams, `tcp:` forwarding, logcat, screencap,
`pm clear`/force-stop, `getprop` and `ip route`. First-run key creation works
around a Windows path bug in libadb's store (I-122).

Live on 2026-10-20, `PATH` reduced to `C:\Windows\System32;C:\Windows`:
Realme RMX3997 Android 16 (`run 20261020-153947-986f`, 86/86 steps), TCL
Smart TV Pro Android 11 (`run 20261020-154152-216d`, 89/89 steps) and the
managed-Chromium web run (`20261020-154341-d767`, 9/9). mDNS discovered both
Android targets; a fresh key paired through the pairing-code dialog and then
passed `device check`; the wizard drove the same pairing flow from a fresh
home and wrote `wizard_pair_phone`. Shell v2, getprop, `ip route` and a
PNG-validated screencap were exercised live. The USB data path is implemented
but not hardware-verified (no ADB interface was exposed by USB here; gap 15);
host logcat is skipped over USB and forward+instrumentation is refused with
explicit guidance. Decision I-121, key-store I-122.


## S-3 · webOS without `ares-*`

**Verdict: go**, rung 1 (own Rust client; no `ssh`, `scp`, `openssl`, `ares-*`
or `luna-send` process on the host). Probe: `%TEMP%\opencode\spike-webos\`
(`FEASIBILITY.md`, `probe/`, `probe-default-nasm/`). There is **no webOS TV**
on this host, so the client stack and the protocol facts are verified, while
every TV-side behaviour stays unverified and is listed below. Decision I-99.

**Dependency and probe (verified on this host).** `russh` 0.63.3 and
`russh-sftp` 3.0.1, both Apache-2.0. With russh's default `aws-lc-rs` feature
the build fails without NASM (`aws-lc-sys` panics in `nasm_builder.rs`);
`default-features = false, features = ["ring", "rsa", "flate2"]` builds clean
(30.9 s) and needs no C toolchain. The loopback probe proved: host-key pinning
via `check_server_key`, Ed25519 auth, RSA-SHA2-256 auth with a key rebuilt from
a legacy PEM, one exec channel with stdout + exit status, and an SFTP
write/read + directory listing. This does **not** prove the TV's SSH server
accepts russh's KEX/ciphers/host-key algorithms, or that it offers SFTP.

**Device and key (verified from the reference implementations).** ares-cli
2.4.0 ships **no `ares-novacom`**; the facts come from `ares-cli-rs` and LG's
CLI guide. The dev-mode target is `prisoner@<tv>:9922`; the Developer Mode app
must be running. The key is a plain HTTP/1.1 `GET http://<tv>:9991/webos_rsa`
(no TLS); its passphrase is the code shown by the TV. `russh::decode_secret_key`
handles the legacy encrypted PEM (AES-128-CBC, key =
MD5(passphrase || IV[0..8])) with or without base64 `=`. A manual
PKCS#5/AES-128-CBC + PKCS#1 fallback is recorded only for the case where the TV
serves a cipher russh cannot parse; it is not on the default path.

**Commands and install.** Command execution is a normal SSH exec. Luna calls
are built as `/usr/bin/luna-send-pub -i luna://<service>/<folder>/<method>
'<json>'`. Install uploads the `.ipk` to `/media/developer/temp` (SFTP; the
reference falls back to `cat > "<path>"` over exec when SFTP is unavailable),
then calls `com.webos.appInstallService/dev/install` with
`{id, ipkUrl, subscribe:true}`; success is `return.details.state` matching
`installed|^SUCCESS`, failure matches `FAILED`. Launch is
`com.webos.applicationManager/launch` `{id, subscribe:false, params}`;
terminate `.../dev/closeByAppId`; list `.../dev/listApps`; running
`.../dev/running`. Inspect forwards remote 9998 through
`channel_open_direct_tcpip`. An IPK is an `ar` archive with `debian-binary`
(`2.0\n`), `control.tar.gz` and `data.tar.gz`; we build it in Rust, not with
`ares-package`.

**Licenses.** ares-cli, ares-cli-rs and dev-manager-desktop are Apache-2.0
(reference/facts only); `react-tv/node-webos` has **no LICENSE file**, so it is
reference-only. `ssh2`/libssh is rejected: it drags OpenSSL (and Perl/MSVC) into
the Windows build, which is worse than the `ring` stack even if a TV cipher
turns out to need the manual fallback.

**Hardware-gated (recorded in `docs/STATUS.md`):** the exact PEM cipher/key
size/padding, the SSH banner and algorithm negotiation, host-key type, SFTP
availability and `/media/developer/temp` permissions, the 9991 response
headers/chunking, whether `com.webos.appInstallService/dev/install` is still
the right service on current firmware (vs `appinstalld2/install`), whether
`luna-send-pub` is present and `-i`/`-n` behave over exec, and whether an
unsigned/unencrypted IPK is accepted.

## S-8 · Standalone editor (WASM)

**Verdict: go** — rung 1. `cargo build -p impresario-core --target
wasm32-unknown-unknown` succeeds on this host with **no changes** (29.7 s;
`rustup target add wasm32-unknown-unknown`). The standalone editor therefore
shares the exact parser/validator/formatter with the CLI and LSP
(§9.2's "one source of truth" is achievable without a second implementation),
served by the local HTTP server; no LSP process is needed in that mode.
The wasm boundary (a thin `wasm-bindgen` wrapper) and the HTTP asset serving
are work-package items, not feasibility risks.

