# Shared TV stub

One HTML application for the webOS and Tizen hardware smokes. It is a
**remote-key-only TV app**, not a web app: there are no click, mouse, pointer,
or touch handlers, and CSS disables pointer input entirely. The D-pad moves
focus, OK activates the focused row, and Back returns to the home screen. Every
remote key is logged, including the numpad, media, colour, volume, channel,
info, guide, subtitle, audio, search and settings keys.

The UI intentionally uses a high-contrast focus ring (amber fill, white border)
so the focused row is obvious in a screenshot and at couch distance. The ring is
painted by both `:focus` and an explicit `.focused` class, and the focus helper
scrolls the active screen so the focused row stays visible on 720p panels. Stable
`#stub_*` ids remain available for scene selectors; `#stub_key_log`,
`#stub_focus`, and `#stub_status` report the last key, the focused element and
the current scenario status.

| Platform | App id | Package | Manifest |
|---|---|---|---|
| webOS | `com.impresario.stub` | `com.impresario.stub` | `appinfo.json` |
| Tizen | `impresario.stub` | `impresario` | `config.xml` |

The Tizen package id is exactly ten characters. A Samsung TV rejects longer
package ids at install time with `[118, -4] Load archive info fail`, even though
the same `.wgt` installs on the emulator. Keep the package id at ten characters
and make the app id `<package>.<suffix>`. The Tizen manifest declares the
`http://tizen.org/feature/tv.inputdevice` feature and the
`http://tizen.org/privilege/tv.inputdevice` privilege; `index.html` registers the
remote keys through `tizen.tvinputdevice`. webOS delivers the same keydown
events without a manifest privilege, and `appinfo.json` sets
`disableBackHistoryAPI` so the Back key reaches the app instead of the platform
history.

## Remote-only behaviour

- Home: D-pad up/down moves the focus ring; OK opens the focused scenario.
- Scenario screens: Back returns home; OK activates only the focused row.
- Scroller: D-pad up/down scrolls the 30-row list.
- Selection: OK opens a dialog whose rows are navigated only with the D-pad.
- Text: OK focuses the field; the TV's system IME supplies the characters.
- Playback: play, pause, play/pause, stop, rewind and fast-forward remote keys
  control the local video. The media keys are registered on Tizen.
- Every key press updates `#stub_key_log` and the in-app key history.

The key table recognises the native TV codes as well as the desktop DOM codes.
Tizen uses `Exit` 10182, `MediaPlayPause` 10252, `Caption` (subtitle) 10221,
`MTS` (audio) 10195, `Search` 10225 and `Menu` 18. webOS uses 461 for both Back
and Exit, so the stub checks `event.key` to distinguish them. The Tizen key
registration uses the documented `VolumeMute`, `Caption` and `MTS` names (with
the webOS aliases as fallbacks), and the mandatory D-pad/Enter/Back keys are not
registered because the platform delivers them automatically.

## Build

```powershell
./build.ps1
```

This stages the shared files plus the platform manifest and writes
`build/stub.ipk` and `build/stub.wgt`. The default binary is
`target/release/impresario.exe`; pass `-Impresario <path>` to override it.
`appinfo.json` is never included in the Tizen package, and `config.xml` is never
included in the webOS package.
