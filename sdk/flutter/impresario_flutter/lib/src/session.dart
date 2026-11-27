import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';

import 'bridge.dart';
import 'bridge_base.dart';
import 'options.dart';
import 'service_extension.dart';
import 'snapshot.dart';

/// An opt-in inspection session.
///
/// It keeps the Flutter semantics tree enabled, registers the
/// `ext.impresario.snapshot` VM-service extension (debug/profile) and starts an
/// optional loopback TCP bridge (all platforms with `dart:io`). The bridge is
/// the release-build path because it does not need the VM service.
class ImpresarioSession {
  ImpresarioSession._({required this.options, required SemanticsHandle? handle})
    : _handle = handle;

  /// Starts a session.
  ///
  /// [enableSemantics] calls [SemanticsBinding.ensureSemantics] and keeps the
  /// returned handle until [stop]. [bridgePort] `null` disables the bridge;
  /// `0` binds an ephemeral port reported by [bridgePort].
  static Future<ImpresarioSession> start({
    ImpresarioSnapshotOptions options = const ImpresarioSnapshotOptions(),
    bool enableSemantics = true,
    bool enableServiceExtension = true,
    int? bridgePort = 7312,
    String bridgeHost = '127.0.0.1',
    String? bridgeToken,
  }) async {
    WidgetsFlutterBinding.ensureInitialized();
    final handle = enableSemantics
        ? SemanticsBinding.instance.ensureSemantics()
        : null;
    final session = ImpresarioSession._(options: options, handle: handle);
    session._attachOwner();
    SemanticsBinding.instance.addSemanticsEnabledListener(
      session._semanticsEnabledListener,
    );
    if (enableServiceExtension) {
      session._serviceExtensionRegistered = registerImpresarioServiceExtension(
        session._provider,
      );
    }
    if (bridgePort != null) {
      try {
        final bridge = createBridge(
          host: bridgeHost,
          port: bridgePort,
          token: bridgeToken,
          provider: session._provider,
        );
        await bridge.start();
        session._bridge = bridge;
        session._bridgePort = bridge.port;
      } catch (error) {
        session._bridgeError = error.toString();
      }
    }
    return session;
  }

  final ImpresarioSnapshotOptions options;
  final SemanticsHandle? _handle;
  SemanticsOwner? _owner;
  ImpresarioBridge? _bridge;
  bool _active = true;
  bool _serviceExtensionRegistered = false;
  int? _bridgePort;
  String? _bridgeError;
  int _changeRev = 0;

  late final VoidCallback _semanticsEnabledListener = _handleSemanticsEnabled;
  late final VoidCallback _semanticsChangedListener = _handleSemanticsChanged;

  bool get isActive => _active;

  /// Whether `ext.impresario.snapshot` was registered.
  bool get serviceExtensionRegistered => _serviceExtensionRegistered;

  /// Actual bridge port, or `null` when the bridge is disabled or unavailable.
  int? get bridgePort => _bridgePort;

  /// Why the bridge could not start, or `null`.
  String? get bridgeError => _bridgeError;

  /// Number of semantics-change events observed since [start].
  int get changeRev => _changeRev;

  /// Captures a snapshot now.
  ///
  /// Throws [StateError] after [stop]. The VM-service extension and bridge call
  /// the same path, so their `rev` values stay monotonic.
  ImpresarioSnapshot snapshot({ImpresarioSnapshotOptions? options}) {
    if (!_active) {
      throw StateError('ImpresarioSession is stopped');
    }
    _attachOwner();
    return ImpresarioSnapshot.capture(options: options ?? this.options);
  }

  /// Stops the bridge, detaches listeners and releases the semantics handle.
  Future<void> stop() async {
    if (!_active) {
      return;
    }
    _active = false;
    SemanticsBinding.instance.removeSemanticsEnabledListener(
      _semanticsEnabledListener,
    );
    _owner?.removeListener(_semanticsChangedListener);
    _owner = null;
    unregisterImpresarioServiceExtension();
    await _bridge?.stop();
    _bridge = null;
    _bridgePort = null;
    _handle?.dispose();
  }

  Map<String, Object?> _provider(Map<String, Object?> params) {
    if (!_active) {
      throw StateError('ImpresarioSession is stopped');
    }
    final snapshot = ImpresarioSnapshot.capture(
      options: options.mergeParams(params),
    );
    return snapshot.toJson();
  }

  void _attachOwner() {
    final owner = RendererBinding.instance.rootPipelineOwner.semanticsOwner;
    if (identical(owner, _owner)) {
      return;
    }
    _owner?.removeListener(_semanticsChangedListener);
    _owner = owner;
    _owner?.addListener(_semanticsChangedListener);
  }

  void _handleSemanticsEnabled() {
    _attachOwner();
    // The owner is created during the next semantics update, not inside the
    // binding's notification.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_active) {
        _attachOwner();
      }
    });
  }

  void _handleSemanticsChanged() {
    if (!_active) {
      return;
    }
    _changeRev += 1;
    _bridge?.notifyChanged(_changeRev);
  }
}
