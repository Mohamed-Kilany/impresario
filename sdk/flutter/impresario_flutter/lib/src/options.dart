import 'dart:ui' show Rect;

/// Options shared by the service extension, the socket bridge and
/// [ImpresarioSnapshot.capture].
class ImpresarioSnapshotOptions {
  const ImpresarioSnapshotOptions({
    this.minVisibleFraction = 0.0,
    this.maxNodes = 5000,
    this.maxDepth = 80,
    this.includeRaw = false,
    this.includeSecureValues = false,
    this.includeSystemUi = false,
  });

  /// A node must have at least this fraction of its area inside the viewport
  /// to be included. Ancestors of included nodes and the focused node are
  /// always kept so the tree stays connected.
  final double minVisibleFraction;

  /// Hard cap on emitted nodes after viewport pruning.
  final int maxNodes;

  /// Maximum traversal depth. `0` means the root only.
  final int maxDepth;

  /// Include the platform-native payload (Flutter semantics id, flags and
  /// actions) in `raw`.
  final bool includeRaw;

  /// Include values of secure text fields. Off by default; turning it on can
  /// expose passwords to the inspector.
  final bool includeSecureValues;

  /// Reserved for platforms with a system UI layer. Flutter semantics do not
  /// expose a separate system UI tree, so this is currently a no-op.
  final bool includeSystemUi;

  /// Parses service-extension/bridge parameters. Unknown keys are ignored and
  /// malformed values fall back to the current option.
  ImpresarioSnapshotOptions mergeParams(Map<String, Object?> params) {
    final visible = _double(params['min_visible_fraction']);
    final nodes = _int(params['max_nodes']);
    final depth = _int(params['max_depth']);
    return ImpresarioSnapshotOptions(
      minVisibleFraction: visible == null
          ? minVisibleFraction
          : visible.clamp(0.0, 1.0).toDouble(),
      maxNodes: nodes == null ? maxNodes : nodes.clamp(1, 50000).toInt(),
      maxDepth: depth == null ? maxDepth : depth.clamp(0, 1000).toInt(),
      includeRaw: _bool(params['include_raw']) ?? includeRaw,
      includeSecureValues:
          _bool(params['include_secure_values']) ?? includeSecureValues,
      includeSystemUi: _bool(params['include_system_ui']) ?? includeSystemUi,
    );
  }

  static double? _double(Object? value) {
    if (value is num) {
      return value.toDouble();
    }
    if (value is String) {
      return double.tryParse(value);
    }
    return null;
  }

  static int? _int(Object? value) {
    if (value is int) {
      return value;
    }
    if (value is num) {
      return value.toInt();
    }
    if (value is String) {
      return int.tryParse(value);
    }
    return null;
  }

  static bool? _bool(Object? value) {
    if (value is bool) {
      return value;
    }
    if (value is num) {
      return value != 0;
    }
    if (value is String) {
      switch (value.toLowerCase()) {
        case '1':
        case 'true':
        case 'yes':
        case 'on':
          return true;
        case '0':
        case 'false':
        case 'no':
        case 'off':
          return false;
      }
    }
    return null;
  }
}

/// Screen geometry in the same shape as the inspector core's `ScreenInfo`.
///
/// Flutter's logical coordinates already include rotation, so [rotation] is
/// always `0`; the host must not rotate the overlay again.
class ImpresarioScreenInfo {
  const ImpresarioScreenInfo({
    required this.logicalWidth,
    required this.logicalHeight,
    required this.pixelWidth,
    required this.pixelHeight,
    required this.scale,
    required this.safeInsets,
  });

  final double logicalWidth;
  final double logicalHeight;
  final int pixelWidth;
  final int pixelHeight;
  final double scale;
  final ImpresarioInsets safeInsets;

  Map<String, Object?> toJson() {
    return <String, Object?>{
      'logical_w': logicalWidth,
      'logical_h': logicalHeight,
      'pixel_w': pixelWidth,
      'pixel_h': pixelHeight,
      'scale': scale,
      'rotation': 0,
      'safe_insets': safeInsets.toJson(),
      'focus_model': 'POINTER',
    };
  }
}

/// Logical safe-area insets.
class ImpresarioInsets {
  const ImpresarioInsets({
    this.top = 0,
    this.right = 0,
    this.bottom = 0,
    this.left = 0,
  });

  final double top;
  final double right;
  final double bottom;
  final double left;

  Map<String, Object?> toJson() {
    return <String, Object?>{
      'top': top,
      'right': right,
      'bottom': bottom,
      'left': left,
    };
  }
}

/// The viewport rectangle in logical coordinates.
Rect impresarioViewport(ImpresarioScreenInfo screen) {
  return Rect.fromLTWH(0, 0, screen.logicalWidth, screen.logicalHeight);
}
