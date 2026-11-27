import 'dart:convert';
import 'dart:developer' as developer;

import 'package:flutter/foundation.dart';

import 'service_extension_base.dart';

ImpresarioSnapshotProvider? _provider;
bool _registered = false;

/// Registers `ext.impresario.snapshot` once per isolate.
///
/// Returns `false` on web, in release builds (where the VM service is not
/// available) or when the name is already taken. The latest provider wins so a
/// stopped and restarted session does not need a new registration.
bool registerImpresarioServiceExtension(ImpresarioSnapshotProvider provider) {
  _provider = provider;
  if (_registered) {
    return true;
  }
  if (kReleaseMode) {
    return false;
  }
  try {
    developer.registerExtension('ext.impresario.snapshot', (
      method,
      parameters,
    ) async {
      final current = _provider;
      if (current == null) {
        return developer.ServiceExtensionResponse.error(
          developer.ServiceExtensionResponse.extensionError,
          jsonEncode(<String, Object?>{
            'code': 'IMP-SESSION-STOPPED',
            'message': 'The Impresario session has stopped.',
          }),
        );
      }
      try {
        final result = current(Map<String, Object?>.from(parameters));
        return developer.ServiceExtensionResponse.result(jsonEncode(result));
      } catch (error) {
        return developer.ServiceExtensionResponse.error(
          developer.ServiceExtensionResponse.extensionError,
          jsonEncode(<String, Object?>{
            'code': 'IMP-SNAPSHOT-FAILED',
            'message': error.toString(),
          }),
        );
      }
    });
    _registered = true;
    return true;
  } catch (_) {
    return false;
  }
}

/// Detaches the active provider. The VM-service registration itself cannot be
/// removed; subsequent calls return `IMP-SESSION-STOPPED`.
void unregisterImpresarioServiceExtension() {
  _provider = null;
}
