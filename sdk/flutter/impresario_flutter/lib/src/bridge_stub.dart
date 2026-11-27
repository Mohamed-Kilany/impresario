import 'bridge_base.dart';
import 'service_extension_base.dart';

/// Web has no loopback sockets; Layer A (the semantics DOM) is used instead.
ImpresarioBridge createBridge({
  required String host,
  required int port,
  required String? token,
  required ImpresarioSnapshotProvider provider,
}) {
  return _NoopBridge();
}

class _NoopBridge implements ImpresarioBridge {
  @override
  Future<void> start() async {}

  @override
  Future<void> stop() async {}

  @override
  int? get port => null;

  @override
  void notifyChanged(int rev) {}
}
