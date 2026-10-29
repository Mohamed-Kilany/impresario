# Packaging and signing

This document describes the packaging and signing paths implemented by
Impresario. They are **pure Rust** and never start `ares-*`, `tizen`, `sdb`,
`openssl`, `ar`, `tar`, `gzip` or any other platform tool at runtime
(D-19/I-138...I-145).

Verification vocabulary is deliberate:

- **fixture-verified** means unit/replay tests passed on the development host
  against generated keys, byte fixtures or a fake server;
- **hardware-verified** means a real TV accepted the package and the app ran.

The Tizen and webOS paths are hardware-verified on real TVs (2026-10-29), in
addition to their fixture/replay coverage. The Tizen TV rejected a temporary
self-signed certificate chain at install and then accepted a native
Samsung-issued, DUID-bound profile created without Tizen Studio; the shared
105-step stub installed, launched and passed on both TVs. The webOS Developer
Mode passphrase was supplied for the session only. Hardware steps below are
claimed only where the run is recorded; `HD-3`, `HD-4` and `HD-7` are closed in
`docs/STATUS.md`.

The Apple path is now pure Rust too: RemotePairing/RSD, DDI mounting, bundle
re-signing, installation, XCUITest startup, Apple ID authentication and
development provisioning all run in-process. The live free-team Apple ID
login/2FA completed on 2026-10-28, and short-ID tvOS profiles were provisioned
for `master`. The short-ID ordinary host/no-stub run then passed
(`20261028-100103-b9ab`, 2/2 steps). The long-ID run
`20261027-063121-00d6` remains the earlier 105-step P0 behaviour/visual
evidence; the full short-ID 105-step stub run then passed
(`20261028-110828-b477`, 105/105, 5m51s, 13 screenshots). See the Apple section
below and `docs/DEVICES.md` §3.3.

## Tizen

### Signing profiles

A Tizen signing profile consists of an **author** PKCS#12 file and a
**distributor** PKCS#12 file. Profiles can be imported from existing files or
created through Samsung's certificate service with the guided
`impresario sign tizen profile create` flow. That flow discovers Samsung's
current Certificate Extension endpoints, opens the normal Samsung account
login in a browser, and does the certificate work natively; Tizen Studio, Java,
`openssl` and the Samsung password are not used. Import validates:

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

`remove` deletes the profile directory and its keychain entries. Runs use the
`samsung` profile by default; set `IMPRESARIO_TIZEN_PROFILE` to choose another
imported profile. Profiles are not configured per device.

### Creating a Samsung-issued profile

Tizen Studio's Certificate Manager is a Java front end around a Samsung account
login and Samsung's certificate APIs. The same flow is available without Tizen
Studio:

```powershell
impresario sign tizen profile create --name samsung `
  --duid P7CIFS3MU3C26
```

The wizard:

1. reads Samsung's current extension index, downloads the Certificate
   Extension if the seven-day cache under
   `~/.impresario/cache/tizen-samsung.json` is missing or stale, and extracts
   the current service URLs and TV CA certificates;
2. prints the Samsung account login URL and opens the default browser. The
   callback listener is `http://localhost:4794/signin/callback`; complete the
   login and any 2FA in the browser. Close Tizen Studio's Certificate Manager
   first if it is using port 4794;
3. generates two RSA-2048 keys and SHA-512 CSRs: the author subject is
   `CN=<email>`; the distributor subject is `CN=TizenSDK` plus the account
   email RDN, with `URN:tizen:packageid=` and one
   `URN:tizen:deviceid=<DUID>` SAN per `--duid`;
4. requests the author and distributor certificates from Samsung, checks that
   the distributor certificate covers every requested DUID, and writes the two
   legacy PBE-SHA1-3DES PKCS#12 files;
5. imports the profile with the same validation and atomic keychain-backed
   storage as `profile add`.

`--duid` is repeatable and accepts a comma-separated list (maximum 50 DUIDs).
`--email` is normally unnecessary: the login response supplies the account
email. Some accounts' callbacks omit it (the 2026-10-29 test account did), so
pass `--email` for those.
`--no-browser` prints the URL instead of opening a browser, `--refresh`
bypasses the seven-day endpoint cache, and `--extension-url` points at a
specific Certificate Extension ZIP. The login timeout defaults to 300 seconds.

The Samsung account login is the only external step. The access token is kept
in memory and zeroized; the random PKCS#12 password is stored only by the
existing OS-keychain profile import. No Samsung password is read or stored.

On 2026-10-29 this flow created profile `samsung` for the attached TV's DUID,
with author and distributor certificates valid to 2027-10-29, and the TV
installed and launched the 105-step stub (`20261029-082529-96bd`). The TV
limits package ids to 10 characters; longer ids fail with `[118, -4] Load
archive info fail`, before signature validation. Certificate validation allows
300 seconds of clock skew because Samsung issues `notBefore` at the issuance
instant.

The attached Tizen 9.0 TV reports DUID `P7CIFS3MU3C26` and requires signing.
Its distributor certificate must carry that DUID in a
`URN:tizen:deviceid=<DUID>` SAN. A self-signed profile with the right DUID
passes `device check` and reaches the TV's installer, but the TV rejects the
chain with `[118, -12] Invalid certificate chain with certificate in
signature`; only a Samsung-issued author/distributor profile can install
(I-146, I-147, I-157). Use `profile create`, or `profile add` with a profile
exported from another machine.

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
packager/signer is invoked. Packaging and installation are hardware-verified on
the LG TV (2026-10-29, `HD-3` closed); signing remains fixture-tested.

## Apple

The Apple path is pure Rust. It never starts `xcrun`, `xcodebuild`, `simctl`,
`devicectl`, `openssl`, Python or a helper binary. Re-signing uses the
`apple-codesign`/`p12-keystore` libraries; provisioning uses Apple's developer
services HTTP APIs directly (I-152/I-153).

### One global Apple ID session

```powershell
impresario account login          # Apple ID, password, team, 2FA
impresario account status --online
impresario account logout
```

The login is global, not workspace-scoped, and writes one encrypted session
under `~/.impresario/apple/`. The password is entered interactively with
`rpassword`, zeroized, and never stored or logged. The session and anisette
state are AES-256-GCM blobs; the 32-byte key lives in the OS credential store
under service `impresario.apple`.

2FA accepts either a trusted-device code or an SMS code. The default is
`--two-factor auto`, which tries an SMS-capable trusted number first and falls
back to a trusted device; `--two-factor device|sms|ask` and
`--two-factor-number <id>` select a method/number explicitly. SMS delivery
lists the trusted phone numbers Apple returns and can resend, go back or
cancel. Voice-only numbers are not offered as SMS targets. Legacy `hsa` accounts, which list a single
SMS-capable device under `trustedDevices` instead of `trustedPhoneNumbers`, use
Apple's per-device security-code endpoint and accept 4-digit codes. Offline
tests cover the 2FA state machine, legacy-device detection and SMS number
selection; the live free-team Apple ID login/2FA completed on 2026-10-28.

Apple app-specific passwords are not a developer-portal login method. Apple
scopes them to third-party apps that access iCloud data
(<https://support.apple.com/en-us/102654>), and the legacy `clientDAW.cgi`
endpoint is gone; the developer portal still requires 2FA, so Impresario does
not offer an app-specific-password login path. The interactive
Apple ID login above is the primary route, including for free personal teams.
Free teams cannot create App Store Connect API keys; the API-key route below is
an optional unattended path for paid developer-program teams only.

If no code arrives, try the SMS path explicitly:

```powershell
impresario account login --force --two-factor sms -v
```

Then check the trusted phone number in the Apple ID settings. Voice-only
numbers are not valid SMS targets, and Apple can refuse SMS with
`423 / -24056`; use a trusted device in that case. If the account has no
reachable trusted device or SMS number, use Apple's account-recovery flow or a
different Apple ID. Accounts that use a security key instead of a code are not
supported by this login.

### Optional unattended provisioning (paid teams)

A **Team Key** from App Store Connect is Apple's supported no-2FA route for
certificates and profiles. Create it once in the browser:

1. Sign in to [App Store Connect](https://appstoreconnect.apple.com) and open
   **Users and Access > Integrations > App Store Connect API > Team Keys**.
2. Add a key with the **Admin** role (certificate/profile management is not
   reliably available to lower roles).
3. Download `AuthKey_<KEYID>.p8` and keep it outside this repository. The file
   cannot be downloaded again. Note the **Issuer ID** and **Key ID**.

Then provision with flags:

```powershell
impresario apple signing provision --name default --platform tvos `
  --team GR2NDRW99B `
  --api-key "$env:USERPROFILE\.impresario\apple\AuthKey_<KEYID>.p8" `
  --issuer-id "<issuer-id>" --key-id "<key-id>"
```

or keep the values in the environment for subsequent runs:

```powershell
$env:IMPRESARIO_APPLE_API_KEY = "$env:USERPROFILE\.impresario\apple\AuthKey_<KEYID>.p8"
$env:IMPRESARIO_APPLE_API_ISSUER_ID = "<issuer-id>"
$env:IMPRESARIO_APPLE_API_KEY_ID = "<key-id>"
$env:IMPRESARIO_APPLE_TEAM_ID = "GR2NDRW99B"
impresario apple signing provision --name default --platform tvos
impresario apple signing certificates
```

The private key is read locally and used only to sign a short-lived ES256 JWT
(`aud=appstoreconnect-v1`, 20-minute maximum); it is never printed or stored
in the signing set, and only the JWT is sent to Apple's API. The API does not
expose the team identifier, so `--team` (or `IMPRESARIO_APPLE_TEAM_ID`) is
required for profile validation.

The API-key path renames the existing portal device record to `impresario`,
registers missing App IDs, reuses an existing identity when its certificate is
still active, creates an Apple Development certificate otherwise, requests
`TVOS_APP_DEVELOPMENT` (or `IOS_APP_DEVELOPMENT`) profiles with Apple's `IOS`
device/bundle resource platform, validates them like the interactive path, and
installs the same signing-set layout. A newly created PKCS#12 always gets a
fresh random password, so a new certificate does not carry over an old (or
exposed) signing-set password. If the certificate limit is full it lists the
team certificates and prompts for a serial to revoke; `--revoke <serial>` does
the same non-interactively. A certificate newly issued by a failed run is
revoked rather than leaking a slot.

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

The default provisioning route uses the stored Apple ID session from
`account login`; no API-key flags are needed for a free personal team.

```powershell
# Generate host + runner + nested test profiles (no stub):
impresario apple signing provision --name default --platform tvos

# Add the fixture stub for explicit stub plays:
impresario apple signing provision --name default --platform tvos --with-stub

# Inspect or revoke development certificates:
impresario apple signing certificates --platform tvos
impresario apple signing provision --name default --platform tvos --revoke <serial>
```

Apple's developer-services API exposes one shared `ios/` resource path for
iOS, tvOS and watchOS; there is no `tvos/` service path. tvOS profile requests
include `subPlatform: "tvOS"` in the request body, while iOS requests omit it.
The `tvos/listDevices.action` and `tvos/listAllDevelopmentCerts.action` URLs
return 404; using `ios/` plus the tvOS profile field is the live-verified
combination (I-156). Unit tests pin both the shared path and the profile body.

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
sessions; a newly generated identity uses it when set. Otherwise provisioning
verifies that the OS credential store can accept the generated password
before contacting Apple. Password and private key values are never printed.
If a profile download or validation fails after Apple issued a new
certificate, provisioning revokes that unused certificate instead of leaking
a slot; a reused certificate is left in place.

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

On 2026-10-28 the free-team Apple ID login/2FA completed and live tvOS
provisioning ran against Apple's services. `signing provision --name default
--platform tvos` registered `com.impresario.agent`,
`com.impresario.agent.tests.xctrunner` and `com.impresario.agent.tests`,
reused active certificate `F71CFA59C1654F6F00B2AB7081FA8AA`, and installed
profiles expiring `2026-11-04`; the portal device was renamed `impresario`.
`--with-stub` then added the optional `com.impresario.stub` profile. A
following `apple doctor` reported `ready` with the DDI mounted and all four
bundles covered.

The short-ID ordinary no-stub path also ran on the physical Apple TV:
`run 20261028-100103-b9ab` used the new `default` host/runner/nested profiles,
installed no stub, drove the real `com.impresario.agent` host UI and passed
2/2 steps in 35.4 s (`agent-host-shortid.png`). The selected app was the real
host, not the stub, so this exercises the product's actual-app branch rather
than the fixture. The full short-ID 105-step stub play then passed:
`run 20261028-110828-b477` exercised all 10 stages / 105 steps in 5m51s with
13 screenshots and 0 failures. An earlier attempt hit a transient
clear-readback race at step 24; the immediate rerun passed. A following
ordinary no-stub run re-passed (`20261028-112028-80fd`, 2/2, 34.9 s), with no
stub selected, and the `impresario apple restart` recovery command is
available if a killed run wedges testmanagerd.

The physical P0 run `20261027-063121-00d6` used the **long** tvOS bundle ids
and profiles created outside this new flow; it remains the earlier 105-step
behaviour/visual evidence. The App Store Connect client and API-key
provisioning path are implemented and unit-tested (ES256 JWT,
device/bundle/certificate/profile calls, platform/profile-type mapping,
certificate-limit detection and cleanup), but they have not been exercised
with a real paid-team key. The exposed legacy `p0test` signing set and its OS
credential were removed after the short-ID `default` set worked. No simulator
evidence is claimed.

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
observed app launch. The 2026-10-29 runs recorded these and passed the
105-step play on both TVs, closing `HD-3`/`HD-4`/`HD-7`; the runbook remains
for re-verification and for the negative cases (invalid package, wrong
passphrase, profile without the DUID, expired certificate).
