# Shared TV stub

One HTML application for the webOS and Tizen hardware smokes. The UI mirrors the
Android TV and tvOS stubs, uses `logos/impresario-logo.png` on a black
background, and exposes stable `#stub_*` ids for scene selectors.

| Platform | App id | Package | Manifest |
|---|---|---|---|
| webOS | `com.impresario.stub` | `com.impresario.stub` | `appinfo.json` |
| Tizen | `impresario.stub` | `impresario` | `config.xml` |

The Tizen package id is exactly ten characters. A Samsung TV rejects longer
package ids at install time with `[118, -4] Load archive info fail`, even though
the same `.wgt` installs on the emulator. Keep the package id at ten characters
and make the app id `<package>.<suffix>`.

## Build

```powershell
./build.ps1
```

This stages the shared files plus the platform manifest and writes
`build/stub.ipk` and `build/stub.wgt`. The default binary is
`target/release/impresario.exe`; pass `-Impresario <path>` to override it.
`appinfo.json` is never included in the Tizen package, and `config.xml` is never
included in the webOS package.
