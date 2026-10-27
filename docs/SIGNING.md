# Packaging and signing

This document describes the packaging and signing paths implemented by
Impresario. They are **pure Rust** and never start `ares-*`, `tizen`, `sdb`,
`openssl`, `ar`, `tar`, `gzip` or any other platform tool at runtime
(D-19/I-138...I-145).

Verification vocabulary is deliberate:

- **fixture-verified** means unit/replay tests passed on the development host
  against generated keys, byte fixtures or a fake server;
- **hardware-verified** means a real TV accepted the package and the app ran.

The Tizen and webOS paths are fixture-verified. Tizen and webOS TVs became
reachable on 2026-10-25. The Tizen TV accepted the SDB upload and install
command but rejected a temporary self-signed certificate chain at install; a
Samsung-issued profile is still missing. The webOS TV's Developer Mode
passphrase is missing. None of the hardware steps below is claimed as
hardware-verified; the pending checks are `HD-3`, `HD-4` and `HD-7` in
`docs/STATUS.md`.

The Apple path is now pure Rust too: RemotePairing/RSD, DDI mounting, bundle
re-signing, installation, XCUITest startup, Apple ID authentication and
development provisioning all run in-process. Apple development provisioning is
offline/fixture-tested but has not completed a live Apple ID login/2FA against
Apple's services. The long bundle ids passed the physical tvOS P0 run
`20261027-063121-00d6`; the short ids on `master` still need fresh profiles and
a hardware re-run. See the Apple section below and `docs/DEVICES.md` A3.

## Tizen

### Signing profiles

A Tizen signing profile consists of an **author** PKCS#12 file and a
**distributor** PKCS#12 file. Profiles are imported, never created here:
creating them needs Samsung certificate services and a Samsung account, which
is an external, optional flow (S-7). Import validates:

- the PKCS#12 password and private-key chain;
- that each private key matches its leaf certificate;
- the leaf certificate validity period;
- the distributor certificate's `URN:tizen:deviceid=<DUID>` URI SANs.

Supported private keys are RSA (PKCS#1 v1.5 signatures), NIST P-256 and NIST
P-384. RSA and ECDSA both use SHA-512 for the XML-DSig signature. ECDSA
signatures are emitted as the raw `r || s` concatenation required by XML-DSig,
not as ASN.1/DER.

Import stores:

```text
~/.impresario/signing/tizen/<profile>/author.p12
~/.impresario/signing/tizen/<profile>/distributor.p12
~/.impresario/signing/tizen/<profile>/profile.json
```

The PKCS#12 files remain encrypted; their passwords are written to the OS
keychain under service `impresario.tizen` with account
`profile:<name>:author` / `profile:<name>:distributor`. `profile.json` contains
only public metadata (subjects, expiry, DUIDs, import time). No password is
ever written to YAML or JSON.

For a headless or test session, set `IMPRESARIO_TIZEN_AUTHOR_PASSWORD` and
`IMPRESARIO_TIZEN_DISTRIBUTOR_PASSWORD`; these override the keychain read.
`profile add` itself still needs the keychain to store the passwords (or fails
explicitly rather than leaving a half-imported profile).

```powershell
$env:IMPRESARIO_TIZEN_AUTHOR_PASSWORD = '...'
$env:IMPRESARIO_TIZEN_DISTRIBUTOR_PASSWORD = '...'
impresario sign tizen profile add living-room `
  --author C:\keys\author.p12 `
  --distributor C:\keys\distributor.p12

impresario sign tizen profile list
impresario sign tizen profile show living-room
impresario sign tizen profile remove living-room
```

`remove` deletes the profile directory and its keychain entries. The profile
name is referenced from a device as `auth.profile: living-room`.

The attached Tizen 9.0 TV reports DUID `P7CIFS3MU3C26` and requires signing.
Its distributor certificate must carry that DUID in a
`URN:tizen:deviceid=<DUID>` SAN. A self-signed profile with the right DUID
passes `device check` and reaches the TV's installer, but the TV rejects the
chain with `[118, -12] Invalid certificate chain with certificate in
signature`; only a Samsung-issued author/distributor profile can install
(I-146, I-147).

### `.wgt` package and signatures

`impresario package tizen <dir>` builds a deterministic unsigned ZIP:

- entries are sorted by path;
- timestamps are fixed at 1980-01-01, permissions are fixed;
- `config.xml` is required at the root and supplies the widget `id` and
  `version`;
- existing `author-signature.xml` / `signature1.xml` entries are dropped.

Signing follows the Tizen layout:

1. `author-signature.xml` signs every original entry plus the fixed `#prop`
   property object;
2. `signature1.xml` signs `author-signature.xml`, every original entry and its
   own `#prop` object;
3. the signed ZIP is written with both signature files first, then the
   original entries in sorted order.

Each reference digest is SHA-512. `SignedInfo` is canonicalized with
exclusive XML C14N; RSA uses
`http://www.w3.org/2001/04/xmldsig-more#rsa-sha512` and ECDSA uses
`http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha512`. File references are
percent-encoded as RFC 3986 path segments; the same-document `#prop` fragment
is deliberately left as `#prop`.

The validator re-reads every ZIP entry, verifies every digest, canonicalizes
the parsed `SignedInfo`, checks the signature with the leaf certificate and
checks the fixed `#prop` digests. `impresario sign tizen check` reports only
after all of those checks pass.

```powershell
impresario package tizen .\build\web
impresario sign tizen package .\build\web.wgt `
  --profile living-room --duid 1234567890 -o .\build\web-signed.wgt
impresario sign tizen check .\build\web-signed.wgt
```

Signing cache entries live under `~/.impresario/cache/tizen-sign/` and are keyed
by the package SHA-256, profile name, profile import time and DUID. Re-importing
a profile under the same name therefore invalidates its old cache entries. A cached file is reused only
after it validates again; it is retained after install. This makes repeated
runs deterministic without rewriting the app.

### Install-time policy

The transport reads `platform_version` from the SDB `capability:` service:

- **Tizen 7.0 and newer** require a profile, the TV's DUID must be covered by
  the distributor certificate, and the package is re-signed;
- **older Tizen** installs the original package without requiring a profile;
- if the capability service is missing `platform_version`, the transport uses
  `9.0` (the conservative, signing-required branch).

The package is pushed to `{sdk_toolpath}/app.wgt`, falling back to
`/home/owner/share/tmp/sdk_tools/tmp`, then installed with
`0 vd_appinstall <app-id> <remote>`. The transport does not trust the install
command's text: after it returns, it polls `0 vd_applist` until the app is
reported, with a bounded timeout. The staged file is intentionally retained
(the unverified `0 rmfile` command is not used).

Debug launch uses `0 debug <app-id>` to obtain the inspector port, then a
native SDB `tcp:<port>` forward. Teardown uses the verified `0 was_kill` verb;
failure to stop an app is logged as a warning, not reported as success.

## webOS

`impresario package webos <dir>` builds a deterministic `.ipk`:

- `debian-binary` contains `2.0\n`;
- `control.tar.gz` contains the Debian control file;
- `data.tar.gz` contains `usr/palm/applications/<id>/...` plus
  `usr/palm/packages/<id>/packageinfo.json`;
- `appinfo.json` supplies the id and version, and its id/version rules are
  validated;
- entries are sorted, tar/ar metadata and gzip headers are fixed, and symlinks
  are refused.

The install path uploads the `.ipk` to `/media/developer/temp` (SFTP, with the
exec `cat` fallback), calls
`com.webos.appInstallService/dev/install` with `subscribe: true`, streams the
reply until a terminal state, then verifies the app through
`com.webos.applicationManager/dev/listApps`. A pre-existing app is not accepted
as proof of an update: if the subscription ends without a terminal state and
the app was already installed, the install fails. The remote upload is cleaned
up best-effort after the result is known.

webOS `.ipk` signing is not implemented in this phase: the Developer Mode
install flow used by Impresario is the one described above, and no `ares-*`
packager/signer is invoked. Packaging and installation remain fixture-tested
only (`HD-3`).

## Apple

The Apple path is pure Rust. It never starts `xcrun`, `xcodebuild`, `simctl`,
`devicectl`, `openssl`, Python or a helper binary. Re-signing uses the
`apple-codesign`/`p12-keystore` libraries; provisioning uses Apple's developer
services HTTP APIs directly (I-152/I-153).

### One global Apple ID session

```powershell
impresario account login          # Apple ID, password, team, 2FA
impresario account status --check
impresario account logout
```

The login is global, not workspace-scoped, and writes one encrypted session
under `~/.impresario/apple/`. The password is entered interactively with
`rpassword`, zeroized, and never stored or logged. The session and anisette
state are AES-256-GCM blobs; the 32-byte key lives in the OS credential store
under service `impresario.apple`.

2FA accepts either a trusted-device code or an SMS code. The console prompt
offers both; `--two-factor device|sms|ask` and `--two-factor-number <id>`
select a method/number non-interactively. SMS delivery lists the trusted phone
numbers Apple returns and can resend, go back or cancel. Offline tests cover
the state machine and SMS number selection; a live Apple login/2FA has not
been exercised yet.

### Pairing and developer-device name

The host presents itself as **`impresario`** to RemotePairing, and the same
name is used when the device is registered with the Apple Developer portal
(`IMPRESARIO_DEVICE_NAME`). It is deliberately not the computer name, the
Apple TV/iPhone name, or a legacy `pyatv` value.

```powershell
impresario apple pair --host <apple-tv-host>
# or import an existing record:
impresario apple import-pair <pair.plist> --save <udid>
```

The pair record is machine-global under `~/.impresario/apple/pairing/`. A
record created earlier by another tool keeps its old host identity; re-pair to
change the name the device displays. Provisioning also renames an existing
developer-portal device record to `impresario`; `--device-name` overrides that
for a deliberate exception.

### Provisioning a signing set

```powershell
# Generate host + runner + nested test profiles (no stub):
impresario apple signing provision --name default --platform tvos

# Add the fixture stub for explicit stub plays:
impresario apple signing provision --name default --platform tvos --with-stub

# Inspect or revoke development certificates:
impresario apple signing certificates --platform tvos
impresario apple signing provision --name default --platform tvos --revoke <serial>
```

Provisioning registers the UDID and App IDs, creates/reuses an RSA-2048 key
and development certificate, builds the PKCS#12 identity, downloads one
development profile per requested bundle, validates team/device/platform/
`get-task-allow`/expiry, and installs a normal signing set at:

```text
~/.impresario/apple/signing/<name>/
  team-id
  identity.p12
  bundles/<bundle-id>/profile.mobileprovision
  bundles/<bundle-id>/entitlements.plist
```

By default the requested bundles are the agent host, the XCUITest runner and
its nested `.xctest`; the fixture stub is **not** provisioned or required. The
existing signing set's identity and key are reused when they still have an
active certificate; `--new-identity` starts over and `--identity <p12>` reuses
a specific identity. When Apple's certificate limit is full, the command lists
the team certificates and prompts for a serial to revoke (or accepts
`--revoke`). The PKCS#12 password is generated locally and stored in the OS
credential store under `impresario.apple`/`signing:<name>`; the
`IMPRESARIO_APPLE_P12_PASSWORD` environment variable is only for headless
sessions. Passwords and private keys are never printed. If a profile download
or validation fails after Apple issued a new certificate, provisioning
revokes that unused certificate instead of leaking a slot; a reused
certificate is left in place.

The legacy import path remains available for an existing identity and
profiles:

```powershell
impresario apple signing import ./signing-source --name default
impresario apple signing verify --name default --products current
```

### Actual apps and the opt-in stub

`AppleTransport` always signs/installs the agent host and the XCUITest runner.
It signs/installs the fixture stub only when either:

- `impresario apple session --with-stub` (or `signing verify --with-stub`) asks
  for it, or
- an app play targets the product stub's bundle id and supplies **no package**.

An actual/release app is installed from its configured `package` and never
needs a stub profile. A package supplied for the stub id also wins over the
product stub, so an explicitly packaged app is never silently replaced.
During clean teardown the runner and any session-installed stub are uninstalled
through InstallationProxy; the agent host stays installed. The runner is
visible in the Apple TV drawer while it runs; tvOS has no supported API for
hiding an installed app.

### Verification state

Pure-Rust provisioning, PKCS#12/CSR creation, profile validation, stub
opt-in and teardown have unit/offline tests. On 2026-10-27 a signing set
containing only the long-id host, runner and nested profiles signed the
product set without the stub (`signing verify` exit 0); adding `--with-stub`
failed with the expected missing-profile error. The main `doctor --plain`
probe then reported the physical Apple TV `READY` while listing the missing
optional stub profile as a device detail, not a degradation.

The physical P0 run `20261027-063121-00d6` used the **long** tvOS bundle ids
and profiles created outside this new flow; it is the 105-step
behaviour/visual evidence. The short-id `master` bundles, live Apple ID
login/2FA, `updateDevice` renaming and fresh short-id profiles have not yet
run against Apple/hardware. Existing profiles expire `2026-11-02`. No
simulator evidence is claimed.

## Error codes

Signing and packaging failures use the `IMP-SIGN-*` family:

| Code | Meaning |
|---|---|
| `IMP-SIGN-001` | package/ZIP layout error |
| `IMP-SIGN-002` | signing profile not found |
| `IMP-SIGN-003` | PKCS#12 import failed |
| `IMP-SIGN-004` | certificate expired or not yet valid |
| `IMP-SIGN-005` | unsupported signing key or key/certificate mismatch |
| `IMP-SIGN-006` | DUID not covered by the distributor certificate |
| `IMP-SIGN-007` | XML-DSig validation failed (digest, canonicalization or signature) |
| `IMP-SIGN-008` | profile password unavailable from keychain or environment |
| `IMP-SIGN-009` | invalid profile/config metadata |
| `IMP-SIGN-010` | signing I/O error |

## Hardware runbook

The exact TV steps and the evidence to record are in `docs/DEVICES.md` §1.3
(webOS) and §1.4 (Tizen). For both, record the TV model/firmware, the
`device check` output, the package hash, the install terminal state and the
observed app launch. Do not mark `HD-3`/`HD-4`/`HD-7` verified until those
steps have run on real hardware.
