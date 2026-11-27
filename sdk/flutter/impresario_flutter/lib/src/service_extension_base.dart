/// Provider used by the VM-service extension and the socket bridge.
///
/// It receives the raw request parameters (service-extension values are always
/// strings; bridge values are JSON scalars) and returns the snapshot JSON.
typedef ImpresarioSnapshotProvider =
    Map<String, Object?> Function(Map<String, Object?> params);
