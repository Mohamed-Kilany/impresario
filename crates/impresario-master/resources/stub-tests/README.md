# Impresario stub suite

The fixture app under test (`ImpresarioStub`) has one screen per scenario.
This suite drives every screen on every platform:

| File | What it covers |
|---|---|
| `scenes/Stub*.scene` | One scene per fixture screen, with web/Android/Apple selectors. |
| `theatres/stub-common.theatre` | Stages every platform runs, plus the browser key-log stage. |
| `theatres/stub-touch.theatre` | Touch long press and tap-only key log for Android and iOS. |
| `theatres/stub-remote.theatre` | Held-OK long press and D-pad key log for Android TV, tvOS, webOS and Tizen. |
| `plays/stub.play` | Ten acts: Home, Basic, Text, Scroll, Selection, Flags, Keys, Info, LongPress and Media. |

Run it against the fixture installed on a device:

```text
impresario run --play <this directory> --device <device name> --app stub
```

`App: stub` in the play must resolve to the fixture package on the selected
device (`com.impresario.stub` on iOS, Android, webOS and Tizen). Pass `--app`
when your workspace names the app differently. `--play <this directory>` also
loads the scenes and theatres beside the play, so the workspace needs no
stub-specific files of its own.
