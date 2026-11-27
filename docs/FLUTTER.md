# Flutter integration

The inspector supports Flutter through three layers. This page documents what
works today, what the opt-in `impresario_flutter` package provides, and what is
still missing.

| Layer | What it uses | Status |
|---|---|---|
| A — semantics tree | The platform accessibility tree (`AccessibilityNodeInfo`, iOS accessibility, Flutter web `flt-semantics`) | Wired into the inspector; detection and `FLUTTER_SEMANTICS_OFF` diagnosis are implemented |
| B — in-app snapshot | The `impresario_flutter` package (`ext.impresario.snapshot` or a loopback bridge) | Package, example and tests implemented; Rust host adapter (INS-6.2) and on-device acceptance pending |
| C — widget inspector | `ext.flutter.inspector.*` over the VM service | Not implemented (INS-6.3) |

The package lives at `sdk/flutter/impresario_flutter`. Its README is the API
reference; this page covers the inspector integration.

## Layer A — what the inspector sees today

Flutter paints its own pixels, so the native tree is empty unless semantics are
enabled. With semantics on, the existing acquisition paths work unchanged:

- **Android**: the UiAutomator agent reads the Flutter semantics tree as
  `AccessibilityNodeInfo`. The agent detects the `flutterEmbedding` manifest
  marker, so a release build needs no instrumentation.
- **iOS/tvOS**: the XCUITest runner reads the accessibility elements Flutter
  publishes.
- **Web**: the connector sees Flutter's `flt-semantics` DOM after accessibility
  is activated. Automatic `flt-semantics-placeholder` activation is not
  implemented yet (INS-6.2), so the semantics layer must be activated manually
  for now; an empty tree reports `FLUTTER_SEMANTICS_OFF`.

Raw nodes carry `framework: "flutter" | "native"`. A detected Flutter tree with
no addressable semantics (no name, text, description, label, placeholder,
value, test id, focus, scroll action or non-generic role) adds
`FLUTTER_SEMANTICS_OFF` to `degraded[]`. The Web UI shows the Flutter banner
only for that reason, and the MCP snapshot exposes the same entry.

Layer A is release-safe and needs no app change beyond enabling semantics, but
its roles are only as good as the app's own `Semantics` widgets, and it has no
automation ids unless the app uses `Semantics(identifier:)` or the package.

## Layer B — the opt-in package

Add the package to a QA build and start a session:

```dart
import 'package:impresario_flutter/impresario_flutter.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await Impresario.start(); // bridge on 127.0.0.1:7312 by default
  runApp(const MyApp());
}
```

`Impresario.start()`:

1. calls `SemanticsBinding.instance.ensureSemantics()` and keeps the handle
   until `stop()`;
2. registers `ext.impresario.snapshot` in debug/profile builds;
3. starts the loopback TCP bridge when `bridgePort` is not `null` (default
   `7312`; `0` selects an ephemeral port reported by `session.bridgePort`).

The bridge is the release path because it needs no Dart VM service. It is
read-only and speaks newline-delimited JSON; the protocol and the snapshot
fields are documented in the package README.

### Minimum versions and fallback

| Requirement | Minimum | Why |
|---|---|---|
| Flutter | `3.38.0` | `SemanticsData.flagsCollection`, `SemanticsBinding.addSemanticsEnabledListener` |
| Dart | `3.10.0` | Language/SDK floor of the package |

For older Flutter versions:

- use `Semantics(identifier: 'login.submit', child: ...)` directly if that
  Flutter version exposes identifiers; the inspector maps it to `testid=`;
- otherwise use a label-based fallback (`Semantics(label: 'login.submit',
  container: true)`) and a `label=` locator. This changes the accessible name,
  so keep it in QA builds.

`ImpresarioTag` uses the identifier form. `ImpresarioTag.labelFallback` is the
QA-only label form; do not use it in production UI.

### Tagging guidance

```dart
ImpresarioTag(
  id: 'checkout.pay',
  child: ElevatedButton(onPressed: pay, child: const Text('Pay')),
)
```

- Use stable, unique ids (`<screen>.<control>`), independent of user-facing
  text.
- Do not use `Key`/`ValueKey` for locators; keys do not cross the native
  accessibility boundary.
- Prefer `Semantics(identifier:)` for ids and `label` for user-facing names.
- Tag interactive controls and important read-only values. An untagged widget
  can still be found by `role`/`label`, but ids are the stable contract.

### Host adapter status

The package's snapshot JSON intentionally mirrors `InspectorNode` /
`InspectorSnapshotResponse`, so the host adapter can forward it without a second
normalization pass. That adapter is **not implemented yet** (INS-6.2): the
master does not call `ext.impresario.snapshot` and does not connect to the
bridge. Until it lands, the inspector continues to use Layer A, and the package
is useful for app-side testing and for preparing the integration.

When the adapter is added it should:

- prefer Layer B when a session/bridge is reachable, otherwise fall back to
  Layer A;
- map `test_id` to the existing `testid=` locator and `raw.semantics_id` only
  for diagnostics;
- preserve `degraded[]` (`FLUTTER_SEMANTICS_OFF`, `PARTIAL`) and `truncated`;
- keep the viewport-only contract and the existing budgets.

## Platform notes

### Android

The bridge listens on the device's loopback. Forward the port from the host
with the usual Android debugging tooling, or run the inspector on the device
network if that is available. The bridge is not reachable from another machine
without forwarding. Layer A remains available when the app is a release build
with no bridge.

### iOS and tvOS

The bridge uses `dart:io`; the app must be able to open a local listening
socket. There is no VM service in release, so use the bridge for release QA
builds. Layer A works for semantics-only apps.

### Web

`dart:io` is unavailable, so the bridge is a no-op and `bridgePort` is `null`.
Use Layer A: Flutter web publishes `flt-semantics` nodes after accessibility is
activated (manual activation for now; automatic placeholder activation is
INS-6.2). `ext.impresario.snapshot` is not available in a browser.

### Desktop

`dart:io` is available; the bridge behaves like Android/iOS. Layer A depends on
the platform's accessibility bridge.

## Snapshot options and pruning

| Option | Default | Effect |
|---|---|---|
| `minVisibleFraction` | `0.0` | Drop nodes whose visible area is below the fraction |
| `maxNodes` | `5000` | Stop after this many emitted nodes; sets `truncated` |
| `maxDepth` | `80` | Stop descending; sets `truncated` |
| `includeRaw` | `false` | Add Flutter semantics id, flags and actions |
| `includeSecureValues` | `false` | Include secure-field values (do not enable in production) |
| `includeSystemUi` | `false` | Reserved; no-op |

Flutter already clips semantics rects to the paint/semantics clip, so most
on-screen nodes report `visible_fraction` `1.0`. The threshold still matters
for transformed/overflowing content where Flutter exposes an unclipped rect,
and fully hidden subtrees are pruned regardless. Ancestors of included nodes
and the focused node are kept so the tree stays connected.

`raw` is a JSON string (`semantics_id`, `flags`, `actions`) when enabled. Flags
and actions use `SemanticsFlag`/`SemanticsAction` names so
`impresario-locator::role_from_flutter_flags` can consume them unchanged.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `FLUTTER_SEMANTICS_OFF` in `degraded[]` | Semantics tree absent | Call `Impresario.ensureSemantics()` or start a session in a QA build; on web activate the semantics placeholder manually (automatic activation is pending) |
| No `testid` nodes | App has no identifiers | Add `ImpresarioTag`/`Semantics(identifier:)`; older Flutter uses the label fallback |
| Roles are `generic` | App's `Semantics` does not declare a role | Add `button: true` etc. in the app; the inspector does not invent roles |
| Bridge connection refused | Wrong port, app not in a session, or release build without the bridge | Check `session.bridgePort`/`bridgeError`; forward the port on Android |
| `UNAUTHORIZED` | Token-enabled bridge without a token | Send the same `bridgeToken` in the request |
| Empty tree in a release build | No VM service and no bridge | Use the bridge path; Layer A needs semantics enabled |
| Values of password fields are empty | Secure values are omitted by default | Leave them omitted; only use `include_secure_values` in a controlled QA run |

## Tests and CI

The package has unit/widget tests for the semantics mapping, viewport pruning,
identifier and label-fallback behavior, raw mode, session lifecycle and the
loopback bridge (including token rejection). The example app has a widget test
that captures a snapshot and exercises a tagged control.

```sh
cd sdk/flutter/impresario_flutter
flutter pub get
dart format --output=none --set-exit-if-changed lib test
flutter analyze
flutter test

cd example
flutter pub get
dart format --output=none --set-exit-if-changed lib test
flutter analyze
flutter test
```

CI runs the same commands in the `flutter` job on Flutter `3.47.5`; run
`46049803499` at `feb54ce` passed all nine jobs. Hardware acceptance is still
pending: the Android Flutter device was unreachable on the last attempt, and
there is no iOS/web app available for Layer B. No on-device Layer B result is
claimed here.

## Remaining work

| Item | Status |
|---|---|
| INS-6.2 host adapter + normalizer | Pending |
| INS-6.3 widget-inspector enrichment | Pending |
| INS-6.4 visual fallback | Pending |
| On-device Layer B acceptance (Android/iOS/web) | Blocked on hardware |
| Release-build bridge packaging/forwarding guidance in the host UI | Pending |
