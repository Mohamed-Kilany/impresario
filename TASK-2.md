# TASK-2.md — impresario, second generation

Status: normative delta spec. Read `IMPRESARIO.md`, `README.md`, `TASK.md`, `docs/DECISIONS.md`
(all of D-01…D-18, I-01…I-90) and `docs/STATUS.md` **before touching anything**.

Where this file conflicts with `TASK.md` or an earlier decision, **this file wins**, and the
conflict is recorded as a new decision in `docs/DECISIONS.md` (continue numbering at `I-91`; new
binding decisions of this spec are `D-19…`, listed in §2). Do not silently rewrite old decisions:
mark them `Superseded by I-nn` the way I-59/I-60 were.

Everything that makes the current system honest stays: fail loudly (`UNSUPPORTED_ACTION`), no
silent no-ops, `PARITY.md` generated from `impresario-core::support`, `STATUS.md` records what
is verified on real hardware and what is not, every deviation is a numbered decision, Rust gates
(`fmt --check`, `clippy -D warnings`, `test --workspace`) stay green at every commit.

---

## 0. Goals in one paragraph

impresario becomes a **self-contained, installable product**: one binary (plus embedded/managed
agent payloads) that works on Windows, macOS and Linux **without** `adb`, Xcode tools, `ares-*`,
`sdb` or the `tizen` CLI installed. It has a persistent, modern interactive terminal UI, a
global home folder, fully CLI-editable configuration with guided device setup, a clean split
between **devices** (where) and **apps** (what), built-in signing/re-signing for Apple and
Tizen, concurrent and background runs, and a redesigned theatrical DSL (scene / theatre / play)
with first-class LSP **and** a standalone (LSP-free) editor extension.

## 1. Non-negotiable working rules

1. **Spike before commit.** Work package W-DEP (§6) starts with time-boxed feasibility spikes
   whose go/no-go results are written to `docs/SPIKES.md` *before* any large implementation
   depends on them. A failed spike triggers the fallback ladder in §6.0, never a silent shortcut.
2. **No unverified claims.** Anything that cannot be exercised on this host's real hardware
   (known: Windows host + WSL, Galaxy A51, TCL Android TV, Realme RMX3997; **no** Mac, **no**
   webOS/Tizen/iOS/tvOS hardware unless the user says otherwise) is unit/fixture-tested against
   recorded protocol traces, gated behind `#[ignore]` hardware tests with exact instructions,
   and listed in `docs/STATUS.md` as *unverified*. Never write "works" for those.
3. **Protocol facts must be checked, not recalled.** Every wire-level detail named in this file
   (ports, service names, command strings) is a *starting hint from memory*. Verify it against
   the reference implementations named in §6 before coding, and cite the source in the decision.
4. **Licenses.** Before adding any crate or porting any logic, record its license in
   `docs/THIRD_PARTY.md`. Our crates are `MIT OR Apache-2.0`; weak-copyleft (e.g. MPL-2.0)
   dependencies are allowed only as unmodified dependencies and must be flagged to the user.
   Reference code under GPL/LGPL may be *studied for protocol facts* but not copied.
5. **Secrets.** Passphrases, Apple ID credentials, signing keys, tokens go to the OS keychain
   (`keyring` crate or equivalent) or an encrypted file in the global home — never plaintext in
   YAML, never in logs/reports (extend the existing redaction, spec 5.6).
6. **Commit per work package**, each with tests, docs, decisions and STATUS updated. No work
   package is "done" with a stub that returns success.
7. **Backwards compatibility is not required** for the DSL, `init`, or config schema (pre-1.0),
   but a one-shot `impresario migrate` must convert an existing workspace (§8.9).

## 2. New binding decisions (copy into DECISIONS.md as D-19…)

| ID | Decision |
|---|---|
| D-19 | **No external platform tools at runtime.** The product never shells out to `adb`, `xcodebuild`/`xcrun`/`simctl`/`devicectl`, `ideviceinstaller`, `ares-*`, `sdb`, `tizen`, `openssl`, `ssh`, `scp`. Everything is a Rust implementation or an embedded/managed payload (ladder in §6.0). The only tolerated host prerequisite is an OS driver/service we cannot legally or technically ship (e.g. Apple's USB multiplexer service on Windows, WinUSB for Android USB); each such case is explicit in `impresario doctor` with a fix hint. |
| D-20 | **Global home** `~/.impresario` (`%USERPROFILE%\.impresario` on Windows; override with `IMPRESARIO_HOME`). Created on first run. No `init` command. |
| D-21 | **Device ≠ App.** A *device* is only a name, platform and connection info. An *app* is a separate definition (ids, package path, launch options). A run binds play → app → device(s). |
| D-22 | **Three-part DSL**: `*.scene` (locators, tabular), `*.theatre` (reusable stages), `*.play` (acts composed of stages, parametrizable). Stage names are globally unique. |
| D-23 | **Config is editable from the CLI/TUI**, at two scopes (`global`, `workspace`), losslessly (comments and ordering of hand-edited files preserved). |
| D-24 | **Jobs.** A run is a *job* with an id. Jobs can run in the foreground, be detached to the background, be listed, attached, tailed and stopped. Multiple jobs run concurrently; a device is leased by at most one job at a time (single-session native agents, D-02). |
| D-25 | **Two editor modes** from one source of truth: the language server (primary) and a standalone mode needing no server process. Grammar/diagnostic parity between them is enforced by a shared conformance corpus. |

---

## 3. Work package W-CLI — a modern, persistent CLI

### 3.1 Product shape
- `impresario` with **no arguments** starts the interactive full-screen/inline TUI session
  (think Claude Code / Gemini CLI): persistent prompt, slash commands, history, live panels,
  status bar. Every capability is *also* a scriptable non-interactive subcommand
  (`impresario run …`, `impresario config set …`) with stable exit codes (I-21 stays) for CI.
- Non-TTY, `CI=1`, `--no-tui` or `--plain`: never start the TUI, print plain line output, no
  prompts (a missing required value is exit 2 with a precise message, not a hang).
- Suggested stack (verify and record in a decision): `ratatui` + `crossterm`, `inquire` or
  `cliclack` for wizards, `clap` derive + `clap_complete`, `tracing`, `owo-colors`/ratatui styles,
  `indicatif` for non-TUI progress. Must work in Windows Terminal, PowerShell, cmd, macOS
  Terminal/iTerm, and common Linux terminals; degrade to 16 colors and ASCII when needed.

### 3.2 TUI requirements
1. **Home screen**: workspace path, global home, devices (live reachability), apps, last run
   summary, running/background jobs, update/doctor warnings.
2. **Prompt with slash commands** and fuzzy completion: `/run`, `/jobs`, `/devices`, `/apps`,
   `/config`, `/tests`, `/open`, `/doctor`, `/fmt`, `/validate`, `/help`, `/quit`. Tab-complete
   device/app/play/act/stage/tag names. Persistent history (`~/.impresario/history`), Ctrl-R
   search, multi-line paste safe.
3. **Run view**: tree of devices → plays → acts → stages → steps with live status, elapsed time,
   current command, rolling log pane per device, screenshot path on failure, filter by
   status. Keys: `b` send to background, `x` stop, `f` follow log, `tab` switch device,
   `/` filter, `?` help. Terminal resize safe, no flicker, no corruption on panic (restore
   terminal in a panic hook).
4. **Test explorer** (§3.4) and **device/app/config editors** (§4, §5) are TUI screens with the
   same functions available as flags/subcommands.
5. **Persistence**: session state (last selection, last run profile, window layout) survives
   restarts (`~/.impresario/state.json`); a crashed TUI never loses a background job (§3.5).
6. Non-goals: mouse is optional-but-supported; no network calls for telemetry (none at all).

### 3.3 Global home layout
```
~/.impresario/
  config.yaml            # global config (defaults, endpoints, current workspace, ui prefs)
  devices.yaml           # global devices (reusable across workspaces)
  apps.yaml              # global apps
  keys/                  # generated adb keys, ssh keys (0600), signing identities (encrypted)
  agents/<version>/      # extracted/cached agent payloads (APKs, Apple runner bundle…)
  tools/<name>/<ver>/    # managed downloads when the ladder (§6.0) requires them, checksummed
  jobs/<job-id>/         # job state, logs, socket/pipe metadata
  history                # prompt history
  state.json             # TUI state
  logs/                  # rotated diagnostic logs
```
- Workspace discovery order: `--workspace <path>` → `IMPRESARIO_WORKSPACE` → `.impresario/` or
  `impresario.config` found walking up from CWD → `workspace` key in global config → CWD.
- The user can change the workspace path any time: `impresario workspace use <path>` (persists
  in global config), `workspace list|add|remove|current`. First run in an empty directory offers
  to create the workspace skeleton **from the TUI/wizard** (this replaces `init`; the example
  scaffold lives behind `impresario workspace new <path> [--example]`).
- Workspace config may live under `<workspace>/impresario.config` and device/app overrides in
  `<workspace>/impresario.devices.config`, `<workspace>/impresario.apps.config` (keep names;
  global ones are `~/.impresario/*.yaml`).
- **Precedence** (high → low): CLI flag → environment (`IMPRESARIO_*`) → workspace file →
  global file → built-in default. `impresario config get <key> --explain` prints the winning
  layer and the shadowed ones. Name collisions between global and workspace devices/apps:
  workspace wins and `devices list` marks `(overrides global)`.

### 3.4 Visibility and control over everything, including test files
- `impresario tests` (and the TUI explorer) lists scenes, theatres, plays, acts, stages, tags,
  platforms, parameters, the files they live in, and *where each is used*.
- Select what to run by any combination: file/glob, play, act (`--act "name"`), tag,
  `--exclude-tag`, platform, "only failed in last run", and an interactive checkbox tree.
  Show the resolved plan (`impresario run --dry-run`, `/plan`): act × device matrix, what is
  skipped and why (platform filter, unsupported action `IMP-DSL-013`), parameter bindings
  (secrets masked).
- Choose devices per selection: `--device a,b`, interactive picker, or a **run profile**
  (`impresario run --profile smoke`, `profiles:` in config; `impresario profile save|list|rm`).
- Test-file operations from the CLI: `new scene|theatre|play <name>` (templates), `show`, `open`
  (`$VISUAL`/`$EDITOR`, falling back to `code`), `rename` (updates every reference across
  the workspace), `fmt`, `validate`, `lint`, `grep <text>`, `where <stage|piece|scene>`.

### 3.5 Parallel and background execution
- A run = a **job** (id `YYYYMMDD-HHMMSS-xxxx`, as run ids today). `impresario run` is attached
  by default; `--detach` (or `b` in the TUI) hands the job to a **background supervisor**.
- Supervisor: a small per-user daemon (`impresario daemon`, auto-started on demand, exits when
  idle) reachable over a Unix domain socket / Windows named pipe (candidate: `interprocess`;
  verify). The daemon owns job execution, so closing the terminal does not kill detached jobs,
  and `Ctrl-C` in an attached view detaches-or-stops by explicit choice, never ambiguously.
- Commands: `jobs [--all]`, `attach <id>`, `logs <id> [-f]`, `stop <id>`, `rm <id>`,
  `report <id>`, `wait <id>` (exit code mirrors the job). Notifications on completion in the TUI
  (and OS notification if a crate makes that low-risk; optional).
- **Concurrency model**: any number of jobs; within a job devices run in parallel (today's
  behavior); across jobs a **device lease** prevents two jobs from driving one device. Conflict
  policy flag: `--on-busy wait|fail|queue` (default `wait` with visible queue position).
  Different devices, different platforms, even the same play on all of them at once must work
  (web + android + androidtv in one job is a test case).
- Exit codes and reporters are unchanged (I-21, I-23); detached jobs persist `report.json`
  exactly as foreground ones.
- Recovery: daemon crash/reboot marks interrupted jobs `interrupted`, never "running".

### 3.6 Acceptance for W-CLI
Scripted PTY tests (`portable-pty`/`expectrl`-style) for: first-run flow, slash command
completion, run view with a `MockAgent`, background detach → close → reattach → report;
snapshot tests of TUI frames (`ratatui` `TestBackend`). Manual checklist for the three Windows
shells in `docs/CLI.md`.

---

## 4. Work package W-CONFIG — everything editable from the CLI

### 4.1 Scope
All settings in both scopes are readable and writable from CLI **and** TUI:
`impresario config list|get|set|unset|edit|path|explain [--global|--workspace]`.
Keys are dotted (`reporters`, `timeout_default_ms`, `app_reset`, `ui.theme`, …); `set` validates
against the same schema the loader uses (unknown key = error, D-04) and refuses bad values with
the same diagnostics (`IMP-CFG-*`). `config edit` opens the file in `$EDITOR` and re-validates
on close (keeps the old file if invalid, shows the error, offers to reopen).

### 4.2 Lossless editing (hard requirement)
Edits made by the CLI must **preserve comments, key order and formatting** of hand-edited
files. Survey crates for round-trip YAML editing; if none is production-grade, record a
decision to move the **config format** to TOML (`toml_edit` is lossless) — migration included
in `impresario migrate`. This supersedes D-04's *format* only (snake_case, unknown-keys-are-errors
stay). Property-test: set then unset a key returns a byte-identical file.

### 4.3 Device wizard (multi-step, modeled on `ares-setup-device`)
`impresario device add|edit|remove|list|show|check|rename|import|export` (+ TUI screen).
`ares-setup-device` flow to emulate: pick **platform** → name → host/port → credentials/key
strategy → (platform specific step) → connectivity check → summary → save, with `--list`,
`--modify`, `--remove`, `--default`, and a non-interactive flag form of every step for scripts.
Wizard rules:
1. Every step shows a default, validates immediately, and can go **back** (`←`/`b`) and **cancel**
   without writing anything. Final screen shows the YAML to be written and the scope
   (`global` default; `--workspace` to scope locally).
2. Platform-specific steps (details in §6/§7):
   - **web**: `launch[:headless]` or CDP endpoint; `page_match`.
   - **android / androidtv**: discover (USB + network scan + mDNS), pair (Android 11+ wireless
     pairing code **and** classic `connect ip:5555`), role detection phone/TV, key generation
     stored in `keys/`, authorization prompt guidance.
   - **webos**: host, SSH port (9922 default for dev mode), user (`prisoner`), the dev-mode
     **passphrase / key server** flow (fetch key from the TV's dev-mode key service, decrypt with
     the passphrase, store it), host-key pinning, test connection, "Dev Mode app expires" warning.
   - **tizen**: host + sdb port, Developer Mode IP check, certificate profile selection or import
     (§7.4), DUID retrieval and display, test connection.
   - **ios / tvos**: discover via USB/Wi-Fi/mDNS, trust/pairing file creation (§6.2), Developer
     Mode check, signing identity selection (§7.1–7.2), tvOS remote pairing.
3. Secrets typed in a wizard go to the keychain; YAML stores only a reference (`secret: keychain:<name>`).
4. A device definition contains **only**: `name`, `platform`, `connection` (host/port/serial/udid/
   transport/endpoint), `auth` references (key/profile/identity names), `labels`, and
   transport-level `options`. No bundle id, package path, app URL, or per-app reset flags
   (see §5). `device check` runs the real probe (I-32) and shows the capabilities.

### 4.4 Acceptance
Golden tests of the wizard against a scripted answer stream (including back/cancel), lossless-edit
property tests, `config get --explain` tests for every precedence layer, and a doc table of every
key (generated from the schema, like PARITY).

---

## 5. Work package W-APPS — app definitions (D-21)

`impresario.apps.config` (workspace) / `~/.impresario/apps.yaml` (global), edited with
`impresario app add|edit|remove|list|show|check` (+ wizard, same UX rules as §4.3):

```yaml
apps:
  - name: shop
    description: Customer shop app
    platforms:
      android:   { id: com.acme.shop, package: ./build/app-release.apk, launch_activity: null }
      androidtv: { id: com.acme.shop.tv, package: ./build/tv.apk }
      ios:       { id: com.acme.shop, package: ./build/Shop.ipa }
      tvos:      { id: com.acme.shop.tv, package: ./build/ShopTV.ipa }
      webos:     { id: com.acme.shop, package: ./build/shop.ipk }
      tizen:     { id: AbCdEf1234.Shop, package: ./build/shop.wgt }
      web:       { url: "https://staging.acme.com" }
    options:
      clear_data: true            # was device option clear_app_data (I-18/I-82/I-84)
      launch_params: {}           # platform specific, documented
```
- `id` = Android package, Apple bundle id, webOS app id, Tizen app id (`<pkgid>.<name>`).
  `package` = the artifact to install/re-sign (optional: attach to an already-installed app).
- Binding: a play declares `App: shop` (header), overridable with `--app`; `app_reset`,
  `clear_data`, `app_path` (I-84) move from device options to app options. The planner reports
  `IMP-APP-*` errors (unknown app, no entry for the device's platform, package missing) before
  hardware is touched. A device never knows an app; an app never knows a device.
- `impresario app install <app> --device <d>` and `app uninstall`, `app check` (id present on
  device, signature valid for that device) are scriptable shortcuts that use the same
  install paths the runner uses.
- Update `docs/ERRORS.md` (new `IMP-APP-*`, `IMP-DEV-*`, `IMP-SIGN-*`, `IMP-JOB-*` families),
  `PROTOCOL.md` (handshake no longer carries per-device app options; carries the resolved app),
  and migrate every transport (`AdbTransport`, apple, connector spawn) to take app + device.

---

## 6. Work package W-DEP — eliminate the runtime environment (D-19)

### 6.0 The fallback ladder (apply per capability, in order, and record the rung chosen)
1. **Pure-Rust implementation** or a well-maintained crate (preferred, no payload).
2. **Embedded** minimal payload in the binary or release archive (`include_bytes!`/sidecar),
   extracted to `~/.impresario/agents|tools`, with SHA-256 verification.
3. **Managed download** on first use into `~/.impresario/tools/<name>/<version>/`, pinned
   version + checksum + signature where available, offline-installable by `impresario tools
   import <archive>`; always explicit and cancellable, never silent.
4. **System tool detection** as a last resort only for an optional convenience (never required
   for a test run); `impresario doctor` explains what is missing and why.
Each capability's chosen rung and rationale go in `docs/SPIKES.md` + a decision. Deliverable of
the work package: `impresario doctor` on a **clean VM with none of adb/Xcode/ares/sdb/tizen
installed** reports green for every platform whose hardware is reachable.

### 6.1 Android / Android TV (replace `adb`)
Needed capabilities (inventory from `AdbTransport`): device discovery (USB + TCP), `connect`,
wireless **pairing** (Android 11+ `adb pair`, SPAKE2 over TLS), RSA key auth (A_AUTH/A_STLS),
`shell` (+ exit status), `exec:` streams, APK **install** (`cmd package install` over streamed
`exec`/`sync` push), `am instrument` launch with output stream, `forward` fallback (I-62),
`logcat` streaming, `screencap`, `pm clear`/`force-stop`, `getprop`, `ip route` (agent endpoint
discovery).
- Candidates to evaluate in the spike: `adb_client` (pure-Rust ADB protocol client with direct
  TCP/USB device modes — verify current API, USB support on Windows, auth/pairing coverage),
  `mozdevice`, `rusb` for USB. Pairing/TLS may need our own implementation (`rustls`, a SPAKE2
  crate such as `spake2`); consult the AOSP `adb` sources (`pairing_auth`, `tls`) and the
  documented protocol (`OVERVIEW.TXT`, `SERVICES.TXT`, `SYNC.TXT`) for facts.
- Windows USB note: raw USB access needs a WinUSB-compatible driver; network ADB needs nothing.
  Support both, document the USB driver requirement honestly in `doctor`, and make the
  network path the default in the wizard.
- Keep the agent architecture (I-62): the master talks gRPC to the agent over the network;
  ADB-equivalent functions only install/start/log/screenshot.

### 6.2 iOS / tvOS from any PC (replace Xcode tools; user pointers: `idevice`, `apple-codesign`)
What `xcodebuild test` does for us today, and what must be reproduced:
1. device discovery + **pairing** (lockdown: usbmuxd, or network pairing for Wi-Fi/Apple TV),
2. **Developer Mode** check (cannot be enabled remotely: wizard instructs, `doctor` verifies),
3. **Developer Disk Image** mount (classic DDI ≤ iOS 16, personalized image iOS 17+, with the
   TSS/signing flow; fetch DDI assets as managed downloads),
4. **install** an `.ipa`/`.app` (installation_proxy + AFC upload),
5. **launch an XCTest runner without `xcodebuild`**: start the UI-test bundle through the
   developer services (testmanagerd / DTX instruments channel, `XCTestConfiguration`),
   the technique used by go-ios (`runtest`/`runwda`), pymobiledevice3 and tidevice for WDA.
   iOS 17+ needs the CoreDevice/RSD **tunnel** (remote pairing, QUIC/TCP tunnel to the device;
   on Windows a user-space/TUN approach — verify what the crate supports),
6. capture logs/syslog/screenshots as needed (we need little: gRPC does the rest),
7. **sign** everything (§7.1–7.2).
- Primary candidates: **`idevice`** (pure-Rust libimobiledevice-style crate — verify supported
  services: usbmuxd, lockdown, installation_proxy, AFC, mounter, DVT/instruments, testmanagerd,
  RSD/tunnel, remote pairing, tvOS) and **`apple-codesign`** (the library behind `rcodesign`:
  Mach-O/bundle/IPA signing, entitlements, provisioning profiles, certificate handling). Studying
  go-ios/pymobiledevice3 for protocol facts is encouraged (license rule §1.4).
- **The runner binary cannot be compiled without Xcode.** Therefore: the Xcode-built, *unsigned*
  runner bundle (`AgentUITests-iOS/-tvOS` + host app, from `agents/apple`) is produced **once per
  release in macOS CI** (GitHub macOS runner; add the job and the release artifact), embedded or
  fetched under the ladder (§6.0), and **re-signed on the user's machine** per device/team. Verify
  this is technically possible for a UI-test bundle + `XCTestConfiguration` (entitlements,
  `get-task-allow`, `com.apple.private.*` restrictions, injected frameworks signing order) in the
  spike, and write the exact, honest outcome.
- **Windows reality check:** USB access to iOS devices on Windows normally relies on Apple's
  **Apple Mobile Device Service** (iTunes / Apple Devices app) providing the usbmuxd socket.
  Evaluate: (a) use it when present (document as the one tolerated prerequisite, D-19),
  (b) a pure-Rust usbmuxd replacement over `rusb` (driver replacement implications), and
  (c) the network path (pairing file created once, then Wi-Fi lockdown + RSD). Decide and record.
  Linux: usbmuxd daemon or pure-Rust; macOS: system usbmuxd.
- Apple device acceptance without hardware: replay tests against recorded wire traces, a loopback
  fake lockdown/usbmuxd server for the transport state machine, and `swift test` stays as is.
  On-device steps become `#[ignore]` tests with a runbook in `docs/DEVICES.md`.
- Fall back honestly: if a spike proves (5) impossible for some OS versions, the CLI reports
  `IMP-DEV-xxx: requires macOS + Xcode for iOS ≥ N` with the reason, and `STATUS.md` says so.

### 6.3 webOS (replace `ares-*`)
`ares-setup-device / ares-package / ares-install / ares-launch / ares-inspect / ares-novacom`
are SSH + file copy + `luna-send` wrappers. Re-implement in Rust:
- **SSH/SFTP client** (`russh` + `russh-sftp`, or `ssh2`; evaluate the Windows story, pure-Rust
  preferred): key auth, dev-mode user, port 9922, host-key pinning, port forwarding for the
  inspector/CDP port, exec with exit status.
- **Dev-mode key retrieval**: the TV's Developer Mode app exposes a key service; the CLI fetches
  the encrypted private key and decrypts it with the passphrase (verify the cipher/KDF against the
  `ares-cli` sources, Apache-2.0 — **study the repository for facts**). Pure-Rust decryption (no
  `openssl` binary).
- **`.ipk` packaging** (`ar` archive containing `control.tar.gz` + `data.tar.gz` + `debian-binary`,
  with `appinfo.json` handling and the app id/version rules `ares-package` applies): implement with
  `ar`/`tar`/`flate2` crates; produce byte-compatible output; fixture-test against a package
  created by the official tool (checked into `fixtures/`, one-time creation by the user if needed).
- **Install/launch/close/list** via the same `luna-send`/`appinstalld` (or `ares-install`'s
  equivalent) commands over SSH — copy the *commands*, verified from sources.
- **Debug/inspect**: launch with the inspector enabled, forward the CDP port over SSH, hand
  `target_endpoint` to the existing connector. The connector stays the web/webOS/Tizen executor
  (D-03) and is spawned/embedded by the master as today (I-33) — now an embedded sidecar of the
  same release, not a PATH lookup.
- No hardware here: fixture tests with a **fake SSH server** (`russh` server side) replaying
  recorded `luna-send` transcripts; hardware tests `#[ignore]`d with a runbook.

### 6.4 Tizen TV (replace `sdb` and the `tizen` CLI)
- **sdb protocol** (derived from ADB; TV in Developer Mode listens on TCP 26101 — verify) implemented
  natively: connect/`devices`, `shell`, `push`/`pull` (sync), install, launch, debug-port query,
  forward. Source the facts from the open-source Tizen tooling sources and community
  implementations (e.g. TizenBrew, Samsung `tizen-sdb` sources if available) — verify, cite.
- **Package** (`.wgt` = zip) creation and **signing** natively (§7.4): `config.xml` handling,
  `author-signature.xml` + `signature1.xml` (W3C Widget Digital Signature / XML-DSig, RSA/ECDSA),
  with the author certificate and the **distributor certificate containing the device DUIDs**.
- Install (`vd_appinstall`/`pkgcmd`-equivalent on TVs — verify the exact on-device command),
  launch, **debug launch returning the inspector port**, forward to the connector's CDP endpoint.
- `device check` shows the DUID and whether the loaded distributor certificate covers it.

### 6.5 Web
Chrome discovery stays (I-35). Add **managed Chromium** (ladder rung 3) as an opt-in:
`impresario tools install chromium` (pinned build, checksum) so even a clean machine runs `web`.

### 6.6 `impresario doctor`
Per platform: what is needed, what was found, which rung provides it, and the exact fix for
anything missing (e.g. "Apple Devices service not running", "USB driver for Android not bound",
"TV Developer Mode off", "Developer Mode disabled on iPhone"). Machine-readable `--json`.
Doctor never installs without consent.

---

## 7. Work package W-SIGN — signing built into the CLI

### 7.1 Apple identity management
`impresario sign identity add|list|show|remove|renew` (+ wizard). Supported sources, in order of
implementation value (verify feasibility in the spike; record the legal/ToS notes):
1. **Existing Apple Developer identity**: import a `.p12` + provisioning profile(s), or
   App Store Connect API key (`.p8`, issuer, key id) to create/download certificates, App IDs,
   device registrations (UDID) and development profiles via the official API.
2. **Apple ID login (free or paid team)** using the provisioning flow of sideloading tools
   (AltServer/Sideloadly/SideStore-style: 2FA, anisette data, development certificate, wildcard or
   explicit App ID, profile for the device). Evaluate crates/prior art (`isideload`-style,
   `icloud-auth`, `apple-codesign`'s own App Store Connect support) and **warn clearly** about
   free-account limits (7-day expiry, app-ID and device quotas) and ToS risk. Apple ID password
   and session go to the keychain; anisette provider configurable.
Everything stored encrypted/keychain; expiry tracked; `doctor` and `devices check` warn
*N days before* a profile/cert expires and `sign renew` re-provisions.

### 7.2 What gets signed, automatically, at run time
For `ios`/`tvos` the planner's "prepare" step becomes: resolve identity → ensure device
registered → ensure profiles for **runner host app, UI-test bundle and the app under test** (the
latter only if we install it) → **re-sign** with `apple-codesign` (entitlements per target,
nested frameworks first, `get-task-allow`) → install → start runner. Re-sign output is cached
by (artifact hash, identity, device UDID) in `~/.impresario/cache/` to keep repeat runs fast.
Failure modes get dedicated `IMP-SIGN-*` codes with fixes (expired profile, device not in
profile, bundle id taken, 2FA required, entitlement not allowed on free team).
`impresario sign check <app> --device <d>` and `sign resign <ipa> --identity <i> --out <ipa>`
are exposed for manual use.

### 7.3 Tizen certificates
`impresario sign tizen profile add|list|show|remove` (author certificate + distributor
certificate + passwords, keychain-stored). Support: **import** existing profile files/`.p12`s
(always), and **create** an author certificate and obtain a distributor certificate bound to the
TV's DUID via Samsung's certificate services *if* the spike shows a documented/stable flow
(Samsung account auth); otherwise import-only, stated honestly.
### 7.4 Tizen repackage-and-sign pipeline
For each Tizen run: take the app's `.wgt`, **unpack → keep/replace signatures → re-sign with the
device's profile (DUID-matching distributor cert) → repack**, cache by (hash, profile, DUID),
install, verify. The pipeline must be a pure function with golden tests (signature files
structure, digest of every entry, deterministic zip ordering) and a validator that checks our
output against the W3C digsig rules. Errors: `IMP-SIGN-*` (DUID not in certificate, expired cert,
wrong author).
### 7.5 Android
Debug-signing of an unsigned APK when requested (`apksig`-equivalent in Rust or an embedded
debug keystore flow) — optional convenience; record decision.

---

## 8. Work package W-DSL — the theatrical DSL (D-22) and tooling

### 8.1 File types and meaning
| File | Theatrical name | Role |
|---|---|---|
| `*.scene` | the **set** | one screen: its locators (**pieces**), in a table |
| `*.theatre` | the **stagecraft** | named, reusable **stages**: sequences of actions/checks over scenes |
| `*.play` | the **performance** | **acts** = ordered sequences of stage references (with arguments) |
Rename internals accordingly (`Scene`, `Theatre`, `Stage`, `Play`, `Act`, `Piece`); keep terminology
consistent in errors, docs, reports, LSP and the TUI. Workspace layout: `scenes/`, `theatres/`,
`plays/` (configurable).

### 8.2 `.scene` — tabular locators
```
Scene: LoginScreen
Platforms: cross-platform

| Piece         | web                       | android          | ios                          | index |
|---------------|---------------------------|------------------|------------------------------|-------|
| LoginTitle    | h1.title                  | id:login_title   | accessibilityId:login_title  |       |
| UsernameInput | #username                 | id:username      | accessibilityId:username     |       |
| SubmitButton  | button[type='submit']     | id:login_btn     | accessibilityId:login        |       |
| PromoBanner   | .promo                    | -                | -                            | 2     |
```
- Header row defines columns: first column is the piece name, then any subset of the seven
  platform keys in any order (plus optional `index`, 0-based). Pipes delimit cells; a literal
  pipe is `\|`; cells are trimmed; quotes are optional and only needed to preserve edge spaces
  or when the cell would be misread (`"`/`'` accepted; same escapes as strings).
- **Absent value** = empty cell, `-`, `~`, or the words `none`/`null` (document one canonical
  spelling for the formatter: `-`). Absent means "no selector for this platform **here**": the
  fallback chain (D-07) then applies; if still unresolved, `IMP-DSL-012` is raised **only when a
  play step actually needs that piece on that device** (as today), plus a *hint* listing which
  platforms the piece does define. A fully absent row is a warning.
- A platform column missing from the table is equivalent to an all-absent column.
- Per-cell selector grammar is unchanged (§5.3 / `DSL.md`); the selector is validated with the
  correct prefix rules **per column** and diagnostics point at the **cell** (exact span).
- Uniqueness: scene name (workspace-wide), piece name (per scene), column name (per table).
- `Platforms:` stays (cross-platform | list) — but see 8.7: it may also be omitted (default
  cross-platform).

### 8.3 `.theatre` — stages
```
Theatre: Authentication

Stage: Login(user, password, remember = 'false')
    Description: Fills the login form and submits it.
    Scene: LoginScreen
        Check: LoginTitle
        Action: '${param.user}' => UsernameInput
        Action: '${param.password}' => PasswordInput
        Action: click => SubmitButton
    Scene: DashboardScreen
        Check: WelcomeBanner is visible timeout 20s

Stage: DashboardShown
    Timeout: 15s
    Scene: DashboardScreen
        Check: WelcomeBanner
```
- A theatre file defines **one or more stages**; `Theatre:` names the file's group (unique,
  workspace-wide). Stage names are **globally unique across all theatres** (`IMP-DSL-030`
  duplicate stage, with both locations in the diagnostic). Theatre-qualified references
  `Authentication.Login` are accepted for readability and required only if the formatter/LSP
  needs to disambiguate (it never needs to if uniqueness holds).
- Stage body = what `Stage:` contained before (`Timeout:`, scene blocks, actions, checks), now
  reusable. Stage **parameters**: `Name(param, param = 'default', …)`; reference as
  `${param.NAME}` inside strings, durations, counts where the grammar allows interpolation
  (decide the exact set; document). Missing required argument, unknown argument, duplicate
  parameter: `IMP-DSL-03x`.
- A stage may **call other stages** with `Perform: OtherStage(args)` (cycle detection →
  `IMP-DSL-03x`; depth limit documented). Keep this small but included — nested reuse is the
  point of stages.
- Platform applicability stays per scene block. A stage may declare `Platforms:` to restrict.

### 8.4 `.play` — acts built from stages
```
Play: User Authentication Feature
Description: Validating secure user entry across platforms.
App: shop
Var: USER = 'test_user_impresario'

Act: Successful login
Tags: smoke, auth
Platforms: android, ios, webos
    Perform: Login(user = '${var.USER}', password = '${env.TEST_PASSWORD}')
    Perform: DashboardShown

Act: Login rejects bad passwords
Tags: auth, negative
    Examples:
        | user      | password   |
        | alice     | wrong-1    |
        | bob       | wrong-2    |
    Perform: Login(user = '${example.user}', password = '${example.password}')
    Perform: LoginErrorShown
```
- An Act is **only** a sequence of `Perform:` lines (stage references with arguments). Inline
  actions/checks in an Act are not allowed — that is what theatres are for (the error explains
  and the LSP offers a quick-fix "extract to stage").
- **Parametrization** is first class: stage arguments (above), play `Var:` + `--var`, `${env.X}`,
  and **`Examples:` tables on an Act** (data-driven acts, one execution per row, each reported as
  `Act name [user=alice]` with secrets masked). Table cells use the same absent-value rules.
  Act-level parameters may also be declared `Act: Name(param, …)` and supplied from `Examples`
  or from CLI `--arg act.param=value`; decide, document, test.
- Uniqueness: act names within a play; stage references must resolve (`IMP-DSL-031` unknown
  stage with "did you mean"); `Perform` argument names/arity checked at validate time
  (`IMP-DSL-032`).
- Reports keep the hierarchy **play → act → stage → step** (stage nodes now come from the
  theatre; reports show the stage's source location). Update JUnit/JSON/HTML/console and
  `report.json` schema version.

### 8.5 Grammar and compatibility
Update `docs/DSL.md` (EBNF included), the scanner/parser in `impresario-core`, the AST (all nodes
keep `Span { file, line, col, len }`; table cells carry precise spans), planner expansion
(act × example rows × device), and `IMP-DSL-*` catalog (`ERRORS.md`). New file-kind mapping in
`impresario validate/parse/fmt`. Keep lexical rules (indent, comments `//`, strings, durations)
unless this section says otherwise; tables are the only new lexical construct and the scanner
must treat table lines as a block at depth 1 under `Examples:` or at depth 0 in scenes.

### 8.6 Formatter
`impresario fmt [--check] [PATH…]` for `.scene/.theatre/.play`: canonical keyword casing, indent
unit, blank-line rules, **column-aligned tables** (monospace-width aware: CJK/RTL safe — use
`unicode-width`; the project has Arabic content), `-` for absent cells, sorted nothing (never
reorders semantics), idempotent (property test: `fmt(fmt(x)) == fmt(x)`), preserves comments.
The same formatter powers LSP formatting.

### 8.7 Lints and validation additions
Unused scene/piece/stage/theatre/var, unused example column, shadowed var, Act without
`Platforms:` that uses a piece unavailable on some configured platform (list them), stage with
required param never supplied. All with stable codes, hints and (where mechanical) LSP quick-fixes.

### 8.8 Runtime/planner
Planner resolves play → acts → example rows → stage calls (params bound, nested expansion with
source-span chain for errors "in stage X called from act Y at line N") → per-device command list.
`IMP-DSL-013` still raised before hardware is touched. Secrets in params follow 5.6 redaction.

### 8.9 `impresario migrate`
Converts an existing workspace: scene piece lines → tables; each old `Stage:` inside an Act →
a stage in a generated theatre (named from the stage; collisions suffixed and reported) with the
Act referencing it; `init`-style config → global/workspace split; device `app_id`/options →
`apps` file. Idempotent, `--dry-run`, leaves a `.bak`. Golden tests on `examples/`.

---

## 9. Work package W-EDITOR — LSP and the standalone extension

### 9.1 LSP (primary, `impresario lsp`) — must cover all three file kinds
- **Diagnostics** (all codes of §8, live, per-cell for tables), **completion** (keywords by
  context; scene/piece/stage/theatre/act/param/var/env names; platform keys; selector prefixes per
  column; durations; `Perform:` argument names with defaults shown; `Examples:` column headers
  from the stage's params; app/device names in headers), **hover** (resolved selector per
  platform + fallback, stage signature + doc + where used, support-matrix verdict for the action
  on each platform), **go to definition / find references / rename** (pieces, scenes, stages,
  theatres, vars, params — workspace-wide, with prepare-rename), **document & workspace symbols**,
  **folding**, **semantic tokens**, **signature help** inside `Perform: Stage(`, **code actions**
  (extract inline steps to a stage, add missing platform column, fix did-you-mean, create missing
  stage/piece, make a duplicate unique, remove unused), **formatting / range formatting /
  on-type formatting** (table re-alignment as you type `|`), **inlay hints** (resolved params),
  **code lens** ("Run act on device…" → invokes the CLI/daemon, "N references").
- **Duplicate detection of anything that must be unique** (scene, piece, theatre, stage, act,
  param, column, `Var`, example column) is mandatory and reported on **every** occurrence with
  related-information links to the others. Cross-file uniqueness included, and updated live as
  files open/close/change on disk (watcher exists, I-76).
- Keep I-76 position-encoding behavior; extend the conformance tests per feature.

### 9.2 Standalone extension mode (no LSP)
`editors/vscode` gets a setting `impresario.mode: "lsp" | "standalone" | "auto"` (auto = LSP if the
`impresario` binary resolves, else standalone). Standalone must deliver the **same feature list**
above (diagnostics, completion, hover, definition/references/rename, symbols, folding, formatting,
code actions, duplicate detection) **inside the extension host**, with no child process.
- Preferred implementation: compile `impresario-core` (scanner, parser, validation, formatter,
  support matrix) to **WASM** and load it from the extension (one source of truth, zero drift).
  Acceptable fallback: a TypeScript implementation fed by a generated `language-spec.json`
  (`impresario spec --json`: keywords, platforms, selector prefixes, support tables, diagnostics
  catalog) — then a **conformance corpus** is mandatory (below). Record the choice (I-nn).
- **Parity gate**: `fixtures/lang-corpus/` holds `(file, expected-diagnostics.json,
  expected-format.txt, completion-probes.json)`. The Rust suite, the LSP suite and the extension's
  test suite (`@vscode/test-electron` + unit tests) all run the same corpus; CI fails on any
  divergence.
- The workspace index in standalone mode is built in the extension (file watcher via the VS Code
  API), debounced, incremental, and must stay responsive on 1,000+ files (benchmark in the tests).
- Packaging: `vsce` build, bundle with `esbuild`, strict TypeScript, ESLint, no runtime network
  access; syntax grammar updated for tables/new keywords; snippets for scene/theatre/play/act/stage.
- Also ship the same in `impresario` TUI where cheap: `fmt`, `validate` and duplicate checks are the
  shared core, no new logic.

---

## 10. Phasing (execute in this order; each phase ends green and documented)

| Phase | Content | Exit criteria |
|---|---|---|
| **0** | Read-in + **spikes** for §6.1–6.5 and §7.1/7.3 feasibility (`docs/SPIKES.md`): Android w/o adb (install + instrument + logcat on the A51/TCL TV from a clean PATH), `idevice`/`apple-codesign` capability matrix and iOS-without-Xcode verdict, SSH+luna-send for webOS, sdb protocol + signing for Tizen. Cross-platform TUI/daemon/IPC/lossless-config crate choices | Written go/no-go + chosen rung per capability + new decisions; user-visible summary of what is *not* possible and why |
| **1** | Global home, workspace resolution, config layering and lossless editing, `device`/`app` split + migration of transports, remove `init`, `workspace` commands | W-CONFIG/W-APPS acceptance; `migrate` for config part |
| **2** | New DSL (scene/theatre/play), parser, planner, formatter, validation, reports, `migrate` for DSL | DSL docs + golden corpus green; live E2E on web + phone + TV re-run with migrated workspace |
| **3** | Android and web fully tool-free (W-DEP 6.1, 6.5) incl. wizard | Live E2E on A51 + TCL TV with **no adb on PATH**; `doctor` green |
| **4** | TUI, jobs/daemon, parallel/background, test explorer | W-CLI acceptance, PTY tests, live multi-device concurrent run + detach/attach |
| **5** | LSP for the new DSL + standalone extension + shared corpus | W-EDITOR parity gate in CI |
| **6** | webOS and Tizen transports + packaging + signing (fixture/trace tested) | Unit/replay tests green; hardware runbooks; STATUS marks unverified |
| **7** | Apple: pairing/DDI/install/XCTest launch/sign via `idevice` + `apple-codesign`, macOS CI job producing the runner artifact | Replay + fake-lockdown tests green; honest STATUS; runbook for first real device |
| **8** | Docs sweep (`README`, `IMPRESARIO.md`, `DSL`, `PROTOCOL`, `DEVICES`, `PARITY`, `ERRORS`, `CLI`, `SIGNING`, `STATUS`), clean-VM `doctor` run, release packaging for Win/macOS/Linux (cargo-dist or equivalent), version bump | Definition of done below |

Phases 3 and 6/7 may be reordered by the Phase 0 verdicts, with the reason recorded.

## 11. Definition of done
1. A user on a clean Windows machine installs **one archive**, runs `impresario`, is walked through
   creating a device (Android over Wi-Fi) and an app, writes/edit a play using scene/theatre/play
   files with completion and duplicate detection (LSP **and** standalone), runs it, sends it to the
   background, starts a second job on another device, and reads both reports — with **no adb**, no
   Xcode, no ares, no sdb, no tizen CLI on the machine.
2. Apple and Tizen signing flows exist end to end in code, are covered by replay/fixture tests, and
   `docs/STATUS.md` states exactly which steps were and were not seen working on real hardware.
3. `impresario doctor` is accurate on a clean VM; every tolerated prerequisite is explicit.
4. All gates green: `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace`, Android JVM
   tests, `swift test` (Linux), extension tests, shared corpus parity, `parity_doc`.
5. `docs/DECISIONS.md` (D-19…, I-91…), `SPIKES.md`, `STATUS.md`, `ERRORS.md`, `THIRD_PARTY.md`
   updated; `IMPRESARIO.md` rewritten for the new shape.

## 12. Things the agent must ask the user instead of guessing
- Which Apple account type will be used (paid team / free Apple ID / existing `.p12`+profiles) and
  whether Apple ID automation (ToS-grey sideloading flow) is acceptable.
- Whether a Mac, an iPhone/Apple TV, a webOS TV or a Samsung Tizen TV can be made available for
  hardware verification, and when.
- Whether a managed download (ladder rung 3) is acceptable for Chromium/DDI assets or everything
  must be embedded in the release archive (size trade-off).
- Config format switch to TOML if lossless YAML editing proves unavailable (§4.2).
