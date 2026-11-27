# Changelog

## 0.1.0

- `Impresario.ensureSemantics()` enables the Flutter semantics tree without a
  screen reader.
- `ImpresarioTag` adds a stable automation identifier through
  `Semantics(identifier:)`.
- `Impresario.start()` registers the `ext.impresario.snapshot` VM-service
  extension and an optional loopback socket bridge.
- `ImpresarioSnapshot.capture()` produces the viewport-pruned normalized node
  tree used by the Impresario inspector's Flutter Layer B.
