import 'service_extension_base.dart';

/// Web has no VM service or loopback sockets; Layer A (the semantics DOM) is
/// the acquisition path there.
bool registerImpresarioServiceExtension(ImpresarioSnapshotProvider provider) {
  return false;
}

void unregisterImpresarioServiceExtension() {}
