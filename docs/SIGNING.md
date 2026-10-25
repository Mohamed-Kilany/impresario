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

Apple identity management, entitlement-aware re-signing and provisioning are
**Phase 7** work (TASK-2 §7.1/§7.2). There is no `impresario sign identity`
command in this build; `AppleTransport` still uses the macOS tools documented
in `docs/STATUS.md` gap 10. The final signing document will be extended when
that phase lands. The current decision records remain I-95/I-96 (`idevice`,
`apple-codesign`, optional App Store Connect / free-Apple-ID sources).

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
