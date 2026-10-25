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
| `keyring` | 4.2.0 | MIT OR Apache-2.0 | secrets: webOS Developer Mode passphrase, Tizen PKCS#12 passwords (Windows Credential Manager/Secret Service) | chosen, shipped — I-99/I-141 |
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
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | managed Chromium archive SHA-256, webOS package cache, Tizen WGT digests and signing-cache keys | chosen - I-123/I-139/I-142 |
| `hkdf` | 0.13.0 | MIT OR Apache-2.0 | pairing: AES key derivation (S-1 plan) | **not used** - own pairing superseded (I-121) |
| `aes-gcm` | 0.11.1 | Apache-2.0 OR MIT | pairing: peer-info encryption (S-1 plan) | **not used** - own pairing superseded (I-121) |
| `rustls` | 0.23.46 | Apache-2.0 OR ISC OR MIT | pairing TLS 1.3 channel (S-1 plan; transitive via `libadb` only) | **not a direct dependency** - I-121 |
| `tokio-rustls` | 0.26.6 | MIT OR Apache-2.0 | async TLS glue (S-1 plan) | **not used** - I-121 |
| `rcgen` | =0.14.10 | MIT OR Apache-2.0 | generated test certificates for Tizen RSA/ECDSA signing tests (dev-dependency only) | chosen, test-only — I-141 |
| `mdns-sd` | 0.21.6 | Apache-2.0 OR MIT | discovery of `_adb-tls-pairing` / `_adb-tls-connect` | chosen - I-121 |
| `ureq` | 2.12.1 | MIT OR Apache-2.0 | managed Chromium HTTPS download (consented, pinned) | chosen - I-123 |
| `zip` | =7.3.0 | MIT | managed Chromium archive extraction; deterministic Tizen `.wgt` read/write | chosen - I-123/I-141 |
| `png` | =0.18.1 | MIT OR Apache-2.0 | connector screenshot-region coverage for `Showing` probes | chosen - TASK-3 probe |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | Windows AppContainer ACL grant for the managed browser; daemon `CreateProcess`/standard-handle inheritance and console features (Windows-only) | chosen - I-123/I-126 |
| `russh` | 0.63.3 | Apache-2.0 | webOS SSH transport (`ring` backend, no NASM/OpenSSL) | chosen — I-99 |
| `russh-sftp` | 3.0.1 | Apache-2.0 | webOS SFTP upload (exec `cat` fallback) | chosen — I-99 |
| `rsa` | =0.9.10 | MIT OR Apache-2.0 | Tizen RSA PKCS#1 v1.5/SHA-512 signing and SDB AUTH/SHA-1 tokens | chosen — I-140/I-141 |
| `rsa` (alias `rsa-russh`) | =0.10.0-rc.18 | MIT OR Apache-2.0 | webOS legacy PEM to russh `RsaKeypair` (same graph as `ssh-key`) | chosen — I-138 |
| `pkcs1` (alias `pkcs1-russh`) | =0.8.0-rc.4 | MIT OR Apache-2.0 | RSA PKCS#1 encoding/decoding for the webOS key fallback | chosen — I-138 |
| `p256` | =0.13.2 | MIT OR Apache-2.0 | Tizen ECDSA P-256 SHA-512 signing/verification | chosen — I-141 |
| `p384` | =0.13.1 | MIT OR Apache-2.0 | Tizen ECDSA P-384 SHA-512 signing/verification | chosen — I-141 |
| `aes` / `cbc` / `md-5` / `sha1` | 0.8.4 / 0.1.2 / 0.10.6 / 0.10.6 | MIT OR Apache-2.0 | legacy webOS PKCS#5 PEM decipher; SDB AUTH token | chosen — I-138/I-140 |
| `flate2` | =1.1.4 | MIT OR Apache-2.0 | deterministic gzip for webOS `.ipk` | chosen — I-139 |
| `tar` | =0.4.46 | MIT OR Apache-2.0 | deterministic ustar for webOS `.ipk` | chosen — I-139 |
| `ar` | =0.9.0 | MIT | deterministic `ar` archive for webOS `.ipk` | chosen — I-139 |
| `p12-keystore` | =0.4.1 | MIT OR Apache-2.0 | import Tizen author/distributor PKCS#12 profiles | chosen — I-141 |
| `ua-cryptography-xml-c14n` | =0.1.6 | MIT | exclusive XML C14N for Tizen XML-DSig | chosen — I-141 |
| `x509-parser` | =0.18.0 | MIT OR Apache-2.0 | Tizen certificate subject, validity and DUID SAN inspection | chosen — I-141 |
| `roxmltree` | =0.21.1 | MIT OR Apache-2.0 | Tizen `config.xml` and signature XML parsing | chosen — I-141 |
| `base64` | 0.23.1 | MIT OR Apache-2.0 | XML-DSig digests/signatures and SDB AUTH public-key blob | chosen — existing/I-141 |
| `rand` | =0.10.3 | MIT OR Apache-2.0 | fake SSH server/client nonces in webOS replay tests (dev-dependency only) | chosen, test-only — I-138 |
| `ssh2` | 0.9.6 | MIT OR Apache-2.0 | libssh2 binding; would drag OpenSSL/Perl into the Windows build | rejected — I-99 |

## Apple Swift packages

The Apple agent is a Swift package (`agents/apple/Package.swift`); versions are
pinned by `Package.resolved`. The Xcode runner uses the same packages plus
Apple's XCTest frameworks, which are part of the platform SDK.

| Package | Version | License | Used for | Status |
|---|---|---|---|---|
| `swift-protobuf` | 1.39.0 (exact) | Apache-2.0 WITH Swift-exception | generated `ImpresarioWire` messages | chosen - I-83 |
| `grpc-swift` | 1.27.6 (exact) | Apache-2.0 | gRPC `Agent` service on SwiftNIO | chosen - I-83 |
| `swift-nio` | 2.65.0 or newer, lock-pinned | Apache-2.0 | event loops and future glue | chosen - I-83 |
| `swift-nio-http2` | 1.46.0 or newer, lock-pinned | Apache-2.0 | gRPC metadata (`NIOHPACK`) | chosen - I-83 |

`protoc` is used only at generation time (`scripts/gen-swift.sh`); its output is
committed. The `protoc-gen-swift` plugin is built from the pinned
`swift-protobuf` checkout. No Swift toolchain or Homebrew package is shipped or
invoked at runtime by Impresario itself.

## Media fixtures and reference streams

The local MP4/WebM/M4A/PNG files under `fixtures/` and the Apple/Android stub
assets were generated for this repository (ffmpeg for the encoded samples, a
small generated PNG gradient). They contain no third-party content and are
distributed under the repository's MIT OR Apache-2.0 terms. The Apple stub can
also open Apple's public BipBop HLS sample URL for manual testing; it is
referenced, not redistributed, and availability is outside this project's
control. No Netflix/DRM-provider asset, FairPlay certificate or private license
proxy is shipped or used by CI.

## VS Code extension dependencies (npm)

These are direct dependencies from `editors/vscode/package-lock.json`; the
transitive tree is pinned by that lock. Only `vscode-languageclient` is
bundled into the runtime extension; the rest are build/test tooling.

| Package | Version | License | Used for | Status |
|---|---|---|---|---|
| `vscode-languageclient` | 9.0.1 | MIT | LSP client bundled by esbuild | chosen, shipped - I-136 |
| `esbuild` | 0.25.12 | MIT | bundle `src/extension.ts` into `out/` | chosen, build-only - I-136 |
| `typescript` | 5.9.3 | Apache-2.0 | strict type-checking (`tsc --noEmit`) | chosen, build-only - I-136 |
| `eslint` | 9.39.5 | MIT | lint `src/`, tests and build scripts | chosen, build-only - I-136 |
| `typescript-eslint` | 8.72.0 | MIT | TypeScript ESLint parser/config | chosen, build-only - I-136 |
| `@vscode/vsce` | 3.9.2 | MIT | VSIX packaging | chosen, build-only - I-136 |
| `@vscode/test-electron` | 2.5.2 | MIT | real extension-host tests | chosen, test-only - I-136; downloads a VS Code test build, never shipped |
| `mocha` | 10.8.2 | MIT | extension-host test runner | chosen, test-only - I-136 |
| `@types/vscode` | 1.85.0 | MIT | VS Code API types (pinned to `engines.vscode`) | chosen, build-only - I-136 |
| `@types/node` | 20.19.43 | MIT | Node API types | chosen, build-only - I-136 |

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

## CI-only build tools (not shipped, not runtime dependencies)

| Tool | License | Used for |
|---|---|---|
| XcodeGen | MIT | generate the Apple Xcode project from `agents/apple/project.yml` in the `Apple Xcode` workflow; the `.xcodeproj` is not committed |
| GitHub-hosted `macos-15` runner + Xcode 16.4 | GitHub service terms / Apple SDK license | unsigned tvOS/iOS compile and artifact upload only; no LAN device access |
| Swift 6.4.1 on Ubuntu | Apache-2.0 with Runtime Library Exception | build and unit-test the Swift package in the `apple` CI job |

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
