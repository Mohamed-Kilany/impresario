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
| `ratatui` | 0.30.2 | MIT | TUI (snapshot-testable via `TestBackend`) | chosen — I-92 |
| `crossterm` | 0.29.0 | MIT | terminal I/O backend | chosen — I-92 |
| `clap` | 4.6.7 | MIT OR Apache-2.0 | CLI parsing | chosen — I-92 |
| `clap_complete` | 4.6.11 | MIT OR Apache-2.0 | shell completions (5 shells incl. PowerShell) | chosen — I-92 |
| `interprocess` | 2.4.6 | 0BSD OR Apache-2.0 | daemon IPC (named pipes / UDS) | chosen — I-92 |
| `portable-pty` | 0.9.0 | MIT | ConPTY/PTY-based tests | chosen — I-92 |
| `keyring` | 4.2.0 | MIT OR Apache-2.0 | secrets (Windows Credential Manager etc.) | chosen — I-92 |
| `unicode-width` | 0.2.2 | MIT OR Apache-2.0 | display-width math (tables, reports) | chosen |
| `inquire` | 0.9.4 | MIT | interactive wizards | candidate (compiles; behaviour under test in work package) |
| `cliclack` | 0.5.6 | MIT | alternative prompt style | candidate |
| `indicatif` | 0.18.6 | MIT | progress bars | candidate |
| `idevice` | 0.1.68 | MIT | iOS/tvOS: usbmuxd client, lockdown, pairing, AFC, install, DDI, DVT, RSD | chosen — I-95 |
| `apple-codesign` | 0.29.0 | **MPL-2.0** | Mach-O / `.app` / `.ipa` signing, entitlements | chosen, **flagged to the user** (weak copyleft, unmodified dependency) — I-96 |
| `app-store-connect` | 0.7.0 | Apache-2.0 OR MIT | App Store Connect API (certs/profiles/devices) | candidate — I-96(b) |
| `isideload` | 0.5.0 | MIT | free-Apple-ID sideloading prior art (not a dependency — studied) | reference |
| `rusbmux` | 0.3.0 | MIT OR Apache-2.0 | pure-Rust usbmuxd over nusb (candidate replacement for Apple's service) | candidate, experimental (190 downloads; needs hardware) |
| `adb_client` | 3.2.3 | MIT | Android: pure-Rust adb client candidate | candidate (S-1 in flight) |
| `rusb` | 0.9.4 | MIT | Android USB transport candidate (Windows WinUSB) | candidate (S-1 in flight) |

## Managed payloads (ladder rung 2/3, TASK-2 §6.0)

None yet. Chromium builds / Apple DDI assets are expected here in Phases 3/7;
they ship only via the explicit, checksummed, cancellable download flow the user
approved (§12 question 4).

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
