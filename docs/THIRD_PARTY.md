# Third-party dependencies and payloads

TASK-2 §1.4: before any crate, tool payload or reference source is used, its
license is recorded here. Our own crates are `MIT OR Apache-2.0`. Weak-copyleft
(e.g. MPL-2.0) is acceptable **only** as an unmodified dependency and is flagged
for the user. Code under GPL/LGPL may be studied for protocol facts but never
copied.

Status vocabulary: `candidate` (under evaluation in a phase-0 spike), `chosen`
(decided, `I-nn` cites it), `rejected` (with the reason), `reference` (read for
facts only).

## Dependencies

| Crate | Version | License | Used for | Status |
|---|---|---|---|---|
| `yaml-edit` | 0.3.2 | Apache-2.0 | lossless YAML config editing (rowan syntax tree) | chosen — I-91 |
| `toml_edit` | 0.25.17 | MIT OR Apache-2.0 | recorded fallback if `yaml-edit` rots | chosen as fallback — I-91 |
| `ratatui` | 0.30.2 | MIT | TUI (snapshot-testable via `TestBackend`) | chosen, shipped — I-92/I-125 |
| `crossterm` | 0.29.0 | MIT | terminal I/O backend | chosen, shipped — I-92/I-125 |
| `clap` | 4.6.7 | MIT OR Apache-2.0 | CLI parsing | chosen — I-92 |
| `clap_complete` | 4.6.11 | MIT OR Apache-2.0 | `impresario completions` for bash/zsh/fish/powershell/elvish | chosen, shipped — I-92/I-125 |
| `interprocess` | 2.4.6 | 0BSD OR Apache-2.0 | daemon IPC (named pipe / Unix socket, no TCP fallback) | chosen, shipped — I-92/I-126 |
| `portable-pty` | 0.9.0 | MIT | ConPTY/PTY-based acceptance tests (dev-dependency only) | chosen, shipped — I-92/I-128 |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 | daemon socket token (OS CSPRNG) | chosen — I-126 |
| `libc` | 0.2.190 | MIT OR Apache-2.0 | Unix pid liveness and daemon `setsid` (Unix-only dependency) | chosen — I-126 |
| `keyring` | 4.2.0 | MIT OR Apache-2.0 | secrets (Windows Credential Manager etc.) | candidate — probed in S-5, no Phase 4 code path uses it yet |
| `unicode-width` | 0.2.2 | MIT OR Apache-2.0 | display-width math (tables, reports) | chosen |
| `inquire` | 0.9.4 | MIT | interactive wizards | candidate — not adopted; Phase 4 wizard/prompt is line-based |
| `cliclack` | 0.5.6 | MIT | alternative prompt style | candidate — not adopted |
| `indicatif` | 0.18.6 | MIT | progress bars | candidate — not adopted |
| `idevice` | 0.1.68 | MIT | iOS/tvOS: usbmuxd client, lockdown, pairing, AFC, install, DDI, DVT, RSD | chosen — I-95 |
| `apple-codesign` | 0.29.0 | **MPL-2.0** | Mach-O / `.app` / `.ipa` signing, entitlements | chosen, **flagged to the user** (weak copyleft, unmodified dependency) — I-96 |
| `app-store-connect` | 0.7.0 | Apache-2.0 OR MIT | App Store Connect API (certs/profiles/devices) | candidate — I-96(b) |
| `isideload` | 0.5.0 | MIT | free-Apple-ID sideloading prior art (not a dependency — studied) | reference |
| `rusbmux` | 0.3.0 | MIT OR Apache-2.0 | pure-Rust usbmuxd over nusb (candidate replacement for Apple's service) | candidate, experimental (190 downloads; needs hardware) |
| `libadb` | 0.6.0 | MIT | Android: direct TCP/USB ADB, TLS/RSA auth, Android 11+ pairing (SPAKE2), shell v2, sync, install, instrumentation, forwarding, logcat, screencap | chosen - I-121 (supersedes `adb_client`, I-97) |
| `nusb` | 0.1.14 | Apache-2.0 OR MIT | Android USB backend for `libadb` (WinUSB on Windows) | chosen - I-121 |
| `adb_client` | 3.2.3 | MIT | Android direct ADB (S-1 candidate) | **not used** - superseded by I-121 before shipping |
| `rusb` | 0.9.4 | MIT | Android USB transport (S-1 candidate) | **not used** - `libadb` uses `nusb` (I-121) |
| `curve25519-dalek` | 5.0.0 | BSD-3-Clause | pairing: Edwards point/scalar arithmetic (S-1 plan) | **not used** - own pairing superseded by `libadb` (I-121) |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | managed Chromium archive SHA-256; `libadb` uses its own pairing hashes | chosen - I-123 |
| `hkdf` | 0.13.0 | MIT OR Apache-2.0 | pairing: AES key derivation (S-1 plan) | **not used** - own pairing superseded (I-121) |
| `aes-gcm` | 0.11.1 | Apache-2.0 OR MIT | pairing: peer-info encryption (S-1 plan) | **not used** - own pairing superseded (I-121) |
| `rustls` | 0.23.46 | Apache-2.0 OR ISC OR MIT | pairing TLS 1.3 channel (S-1 plan; transitive via `libadb` only) | **not a direct dependency** - I-121 |
| `tokio-rustls` | 0.26.6 | MIT OR Apache-2.0 | async TLS glue (S-1 plan) | **not used** - I-121 |
| `rcgen` | 0.14.10 | MIT OR Apache-2.0 | pairing self-signed certificate (S-1 plan) | **not used** - I-121 |
| `mdns-sd` | 0.21.6 | Apache-2.0 OR MIT | discovery of `_adb-tls-pairing` / `_adb-tls-connect` | chosen - I-121 |
| `ureq` | 2.12.1 | MIT OR Apache-2.0 | managed Chromium HTTPS download (consented, pinned) | chosen - I-123 |
| `zip` | 7.3.0 | MIT | managed Chromium archive extraction | chosen - I-123 |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | Windows AppContainer ACL grant for the managed browser; daemon `CreateProcess`/standard-handle inheritance and console features (Windows-only) | chosen - I-123/I-126 |
| `russh` | 0.63.3 | Apache-2.0 | webOS SSH transport (`ring` backend, no NASM/OpenSSL) | chosen — I-99 |
| `russh-sftp` | 3.0.1 | Apache-2.0 | webOS SFTP upload (exec `cat` fallback) | chosen — I-99 |
| `ssh2` | 0.9.6 | MIT OR Apache-2.0 | libssh2 binding; would drag OpenSSL/Perl into the Windows build | rejected — I-99 |

## Managed payloads (ladder rung 2/3, TASK-2 §6.0)

| Payload | Version | License | Used for | Status |
|---|---|---|---|---|
| Chrome for Testing (Chromium) | 155.0.8059.84 | Chromium source is BSD-3-Clause; the Chrome for Testing binary is distributed by Google under its own Chrome terms, not redistributed here | optional managed browser for the web connector; pinned per platform with size + SHA-256 | chosen - I-123. Downloaded only by `impresario tools install chromium` after consent; never redistributed by Impresario |
| Apple Developer Disk Image / DDI | TBD | Apple terms | iOS/tvOS device support (Phase 7) | planned; consent + checksum/verification required |

The build embeds no browser and no Apple DDI. `doctor` only reports their
presence; it never downloads.

## Host prerequisites (cannot be shipped; D-19 allows them explicitly)

| Prerequisite | Platform | Needed for | Reported by |
|---|---|---|---|
| Apple Mobile Device Service (usbmuxd, `127.0.0.1:27015`) | Windows | iOS/tvOS over USB; network pairing after setup | `impresario doctor` + docs (S-2) |

## Reference implementations studied for protocol facts (never copied)

| Project | License | Studied for | Spike |
|---|---|---|---|
| `TizenTeam/sdb` | Apache-2.0 | sdb = adb-derived host/daemon protocol (`OVERVIEW.TXT`, `SERVICES.TXT`) | S-4 |
| `Samsung/tizen-agent-skills` | Apache-2.0 | TV command surface, debug launch/port-forward, capability/DUID selection | S-4 |
| `elfpie/tizen-app-installer-cli` | **no license** | sdb wire client (frames, CNXN, sync push), install flow, `.wgt` resigner | S-4, S-7 — facts only |
| `reisxd/tizen.js` | GPL-3.0 | `.wgt` XML-DSig signer reference | S-7 — facts only |
| `gifflet/tizen-cert-cli` | MIT | CSR/SAN/DUID encoding, legacy PKCS#12, cert inspection | S-7 |
| `putdotio/taizn` | MIT | `.wgt` inspection/validation rules | S-7 |
| `danielpaulus/go-ios` | MIT | XCTest/testmanagerd-over-DTX technique, iOS17 tunnel constraints | S-2 |
| `doronz88/pymobiledevice3` | GPL-3.0 | iOS17 tunnel support matrix, XCTest flow | S-2 — facts only |
| `alibaba/tidevice` | MIT | instruments/process-control technique | S-2 |
| `SideStore` / `Feather` | AGPL-3.0 / GPL-3.0 | free-Apple-ID flow limits (anisette, 7-day expiry) | S-6 — facts only |
| `AOSP platform/packages/modules/adb` | Apache-2.0 | ADB pairing protocol (SPAKE2 over TLS 1.3, exporter `adb-label`, AES-GCM framing, PeerInfo), mDNS service names | S-1 |
| `BoringSSL` | Apache-2.0 | `spake25519` = SPAKE2 draft-02 (M/N points, scalar hack, SHA-512 transcript) - facts only; the shipped implementation uses `libadb`, not a port | S-1 |
| `webosose/ares-cli` | Apache-2.0 | webOS command strings (luna services), install/upload flow, IPK layout | S-3 |
| `webosbrew/ares-cli-rs` | Apache-2.0 | webOS dev target (`prisoner:9922`), `/webos_rsa` getkey, luna over SSH | S-3 |
| `webosbrew/dev-manager-desktop` | Apache-2.0 | webOS key validation and device setup prior art | S-3 |
| `react-tv/node-webos` | **no license** | `GET /webos_rsa` reference | S-3 — facts only |
