# impresario_flutter

Opt-in Flutter support for the Impresario element inspector.

The package does three things:

- keeps Flutter's semantics tree enabled without a screen reader;
- tags widgets with a stable automation identifier (`Semantics(identifier:)`);
- captures a viewport-pruned, normalized semantics snapshot and serves it over
  `ext.impresario.snapshot` (debug/profile) or a loopback TCP bridge (the
  release path).

It is deliberately small: Flutter plus the Dart SDK, no other dependencies.

> Status: the package, its example and its tests are implemented. The Impresario
> host-side adapter (INS-6.2), widget-inspector enrichment (INS-6.3) and the
> visual fallback (INS-6.4) are not part of this package yet, and on-device
> Layer B acceptance is still pending hardware. Today the inspector continues to
> read Flutter apps through Layer A (the platform accessibility tree).

## Requirements

| Component | Minimum |
|---|---|
| Flutter | `3.38.0` |
| Dart | `3.10.0` |

The minimum is set by `SemanticsData.flagsCollection` and
`SemanticsBinding.addSemanticsEnabledListener`, which the snapshot walker uses.
Older Flutter versions cannot build this package. They can still be inspected
through Layer A:

- use `Semantics(identifier: 'login.submit', child: ...)` where their Flutter
  version exposes identifiers; or
- wrap the widget in `Semantics(label: 'login.submit', container: true)` and
  match it with a `label=` locator. This changes the accessible name, so use it
  only in QA/automation builds. `ImpresarioTag.labelFallback` implements this
  fallback for supported Flutter versions.

Do not rely on `Key` or `ValueKey` for native-side lookup: keys are not exposed
to the platform accessibility layer.

## Install

The package is not published. Add it as a path or git dependency in a QA build:

```yaml
dependencies:
  impresario_flutter:
    path: ../sdk/flutter/impresario_flutter
```

Gate the calls behind a build flag, flavor or `kDebugMode`-style check so a
production build does not start a listener.

## Quick start

```dart
import 'package:flutter/material.dart';
import 'package:impresario_flutter/impresario_flutter.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await Impresario.start(); // semantics + service extension + 127.0.0.1:7312
  runApp(const MyApp());
}

class MyApp extends StatelessWidget {
  const MyApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      home: Scaffold(
        body: Center(
          child: ImpresarioTag(
            id: 'login.submit',
            child: ElevatedButton(
              onPressed: () {},
              child: const Text('Log in'),
            ),
          ),
        ),
      ),
    );
  }
}
```

`Impresario.start()` returns an `ImpresarioSession`; call `await session.stop()`
when the QA surface is torn down. If you only need semantics, call
`Impresario.ensureSemantics()` and dispose the returned handle.

## Tagging

`ImpresarioTag` wraps `Semantics(identifier: id, container: true)`. The
identifier becomes `testid=<id>` in the inspector and maps to:

| Platform | Native field |
|---|---|
| Android | `resource-id` |
| iOS / tvOS | `accessibilityIdentifier` |
| Web | Flutter's semantics identifier attribute |

`container: true` (the default) guarantees the identifier gets its own node even
when the child is a plain `Text`. Set `explicitChildNodes: true` when the child
already produces its own semantics nodes and you want a strict hierarchy.

`ImpresarioTag.labelFallback` sets `label: id` instead of the identifier. It
changes the accessible name and is intended only for QA builds or Flutter
versions/native layers that do not expose identifiers. The inspector matches it
as `label=<id>`.

## Sessions

```dart
final session = await Impresario.start(
  options: const ImpresarioSnapshotOptions(maxNodes: 2000),
  bridgePort: 7312,       // null disables the bridge; 0 picks an ephemeral port
  bridgeToken: 'secret',  // optional; every request must echo it
);
final snapshot = session.snapshot();
await session.stop();
```

| Mode | Where it works | How the host reaches it |
|---|---|---|
| `ext.impresario.snapshot` | debug/profile with a VM service | VM-service extension call |
| Loopback TCP bridge | debug/profile/release on platforms with `dart:io` | newline-delimited JSON on `127.0.0.1:<port>` |
| Web | all modes | Layer A only; the bridge is a no-op |

The bridge binds loopback only (`127.0.0.1`, `localhost` or `::1`) and refuses
other hosts. It has no TLS and no authentication beyond the optional token, so
do not expose the port. On Android, forward the device port to the host with
your usual debugging tooling when needed.

### Bridge protocol

One JSON object per line. The server sends a `hello` event on connect, then one
response per request and unsolicited `semantics` events after tree changes.

```text
<- {"event":"hello","protocol":1,"framework":"flutter","methods":["snapshot","ping","capabilities"]}
-> {"id":1,"method":"snapshot","params":{"max_nodes":500,"min_visible_fraction":0.1}}
<- {"id":1,"ok":true,"result":{"rev":7,"framework":"flutter",...}}
<- {"event":"semantics","rev":8}
```

Methods: `snapshot`, `ping`, `capabilities`. A token-enabled bridge requires a
top-level `"token"` field. Errors use `{"id":...,"ok":false,"error":{"code":...,
"message":...}}` with `BAD_REQUEST`, `UNAUTHORIZED`, `UNKNOWN_METHOD`,
`REQUEST_TOO_LARGE` or `SNAPSHOT_FAILED`. Requests are capped at 1 MiB. The
service extension reports `IMP-SESSION-STOPPED` and `IMP-SNAPSHOT-FAILED` in its
error payload.

`snapshot` parameters (all optional):

| Parameter | Default | Meaning |
|---|---|---|
| `min_visible_fraction` | `0.0` | Minimum visible area for a node |
| `max_nodes` | `5000` | Emitted-node cap |
| `max_depth` | `80` | Traversal depth cap |
| `include_raw` | `false` | Add the Flutter payload to `raw` |
| `include_secure_values` | `false` | Include secure-field values |
| `include_system_ui` | `false` | Reserved; no-op for Flutter |

## Snapshot shape

The JSON mirrors the inspector's `InspectorSnapshotResponse` /
`InspectorNode` fields:

```json
{
  "rev": 7,
  "framework": "flutter",
  "platform": "android",
  "screen": {
    "logical_w": 393.0,
    "logical_h": 852.0,
    "pixel_w": 1080,
    "pixel_h": 2340,
    "scale": 2.75,
    "rotation": 0,
    "safe_insets": {"top": 24.0, "right": 0.0, "bottom": 0.0, "left": 0.0},
    "focus_model": "POINTER"
  },
  "nodes": [
    {
      "id": 5,
      "parent_id": 4,
      "role": "button",
      "name": "Log in",
      "text": "Log in",
      "test_id": "login.submit",
      "bounds": {"x": 100.0, "y": 700.0, "w": 193.0, "h": 48.0},
      "viewport_bounds": {"x": 100.0, "y": 700.0, "w": 193.0, "h": 48.0},
      "visible_fraction": 1.0,
      "focused": false,
      "enabled": true,
      "selected": false,
      "checked": null,
      "expanded": null,
      "pressed": null,
      "readonly": false,
      "required": false,
      "hidden": false,
      "secure": false,
      "system_ui": false,
      "scrollable": false,
      "can_scroll_backward": false,
      "can_scroll_forward": false,
      "scroll_axis": "",
      "children_hidden_count": 0,
      "raw": "",
      "value": "",
      "label": "Log in",
      "description": "",
      "placeholder": "",
      "alt": "",
      "title": "",
      "css_selector": "",
      "xpath": "",
      "escapes_clip": false,
      "depth": 5,
      "text_truncated": false
    }
  ],
  "degraded": [],
  "truncated": false,
  "focused_id": 0,
  "timestamp_unix_ms": 1790000000000,
  "acquisition_ms": 3
}
```

- Node ids are sequential and positive; the root has `parent_id: 0`. Flutter's
  own semantics id is in `raw` when `include_raw` is on.
- Ids can change between snapshots. Prefer `testid`, `role`/`label` and text
  locators over ids.
- Nodes outside the viewport (including fully hidden subtrees) are pruned; the
  parent's `children_hidden_count` records skipped children.
- Secure-field values are omitted unless `include_secure_values` is enabled.
- `degraded` uses `CODE: message`; `FLUTTER_SEMANTICS_OFF` means the semantics
  tree is absent and `Impresario.ensureSemantics()` (or a session) is needed.
- `platform` is `android`, `ios`, `web`, `macos`, `windows`, `linux` or
  `fuchsia`. Flutter's logical coordinates already include rotation, so
  `screen.rotation` is always `0`.

## Example and tests

```text
example/                    small counter app with tagged controls
test/semantics_test.dart    role/raw mapping
test/snapshot_test.dart     viewport pruning, identifiers, fallback, raw mode
test/bridge_test.dart       loopback snapshot and token rejection
```

```sh
flutter pub get
flutter analyze
flutter test
cd example && flutter pub get && flutter analyze && flutter test
```

Repository CI runs the same commands in the `flutter` job on Flutter `3.47.5`.

## Limits

- The bridge is read-only: it serves snapshots, not input.
- Layer C (widget type, key, source location) and the visual fallback are not
  implemented.
- The inspector does not consume this bridge yet; the Rust host adapter is
  INS-6.2.
- The web target has no loopback bridge; use Layer A.
- `include_system_ui` is reserved and currently a no-op.
- On-device acceptance (Android/iOS/web) is pending; the package tests are
  widget/unit tests, not hardware evidence.
