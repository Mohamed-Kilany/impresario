import 'dart:convert';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';

import 'options.dart';
import 'semantics.dart';

/// A viewport-pruned, normalized Flutter semantics snapshot.
///
/// The JSON shape mirrors the inspector's `InspectorSnapshotResponse` /
/// `InspectorNode` fields (snake_case) so the host adapter can forward it
/// without a second normalization pass. Node ids are sequential and positive
/// with `parent_id == 0` for the root; Flutter's own semantics id is carried
/// in `raw` when `include_raw` is enabled.
class ImpresarioSnapshot {
  ImpresarioSnapshot._({
    required this.rev,
    required this.platform,
    required this.screen,
    required this.nodes,
    required this.degraded,
    required this.truncated,
    required this.focusedId,
    required this.acquisitionMs,
    required this.timestampUnixMs,
  });

  static int _nextRev = 0;

  /// Monotonic revision for this isolate.
  final int rev;

  /// `android`, `ios`, `web`, `macos`, `windows`, `linux` or `fuchsia`.
  final String platform;

  /// Logical/pixel geometry of the captured view.
  final ImpresarioScreenInfo screen;

  /// Normalized nodes in pre-order.
  final List<Map<String, Object?>> nodes;

  /// Structured `CODE: message` entries.
  final List<String> degraded;

  /// True when the node or depth budget was reached.
  final bool truncated;

  /// Sequential id of the first focused node, or `0`.
  final int focusedId;

  final int acquisitionMs;
  final int timestampUnixMs;

  Map<String, Object?> toJson() {
    return <String, Object?>{
      'rev': rev,
      'framework': 'flutter',
      'platform': platform,
      'screen': screen.toJson(),
      'nodes': nodes,
      'degraded': degraded,
      'truncated': truncated,
      'focused_id': focusedId,
      'timestamp_unix_ms': timestampUnixMs,
      'acquisition_ms': acquisitionMs,
    };
  }

  /// Captures the current view.
  ///
  /// Call [WidgetsFlutterBinding.ensureInitialized] (or `runApp`) first. If
  /// semantics are disabled the result is an empty snapshot with a
  /// `FLUTTER_SEMANTICS_OFF` degradation; call `Impresario.ensureSemantics()`
  /// or start an `ImpresarioSession`.
  static ImpresarioSnapshot capture({
    ImpresarioSnapshotOptions options = const ImpresarioSnapshotOptions(),
  }) {
    final stopwatch = Stopwatch()..start();
    final timestamp = DateTime.now().millisecondsSinceEpoch;
    final platform = _platformName();
    WidgetsFlutterBinding.ensureInitialized();
    final renderer = RendererBinding.instance;
    final views = renderer.renderViews.toList(growable: false);
    if (views.isEmpty) {
      return _degraded(
        code: 'NO_VIEW',
        message: 'The Flutter binding has no RenderView yet.',
        platform: platform,
        timestamp: timestamp,
        stopwatch: stopwatch,
      );
    }
    final view = views.first;
    final size = view.size;
    final dpr = view.flutterView.devicePixelRatio;
    final physical = view.flutterView.physicalSize;
    final padding = view.flutterView.padding;
    final screen = ImpresarioScreenInfo(
      logicalWidth: size.width,
      logicalHeight: size.height,
      pixelWidth: physical.width.round(),
      pixelHeight: physical.height.round(),
      scale: dpr,
      safeInsets: ImpresarioInsets(
        top: padding.top / dpr,
        right: padding.right / dpr,
        bottom: padding.bottom / dpr,
        left: padding.left / dpr,
      ),
    );
    if (size.isEmpty) {
      return _degraded(
        code: 'NO_VIEW',
        message: 'The Flutter view has no size yet.',
        platform: platform,
        timestamp: timestamp,
        stopwatch: stopwatch,
        screen: screen,
      );
    }
    final owner =
        view.owner?.semanticsOwner ?? renderer.rootPipelineOwner.semanticsOwner;
    final root = owner?.rootSemanticsNode;
    if (root == null) {
      return _degraded(
        code: 'FLUTTER_SEMANTICS_OFF',
        message:
            'Flutter semantics are disabled. Call Impresario.ensureSemantics() '
            'or start an ImpresarioSession in a QA build.',
        platform: platform,
        timestamp: timestamp,
        stopwatch: stopwatch,
        screen: screen,
      );
    }

    final rootMatrix = view.hasConfiguration
        ? view.configuration.toMatrix()
        : Matrix4.diagonal3Values(dpr, dpr, 1.0);
    final rootInverse = Matrix4.copy(rootMatrix)..invert();
    final viewport = Rect.fromLTWH(0, 0, size.width, size.height);
    final walker = _Walker(options: options, viewport: viewport);
    final candidate = walker.visit(root, null, rootInverse, viewport, 0);

    final nodes = <Map<String, Object?>>[];
    var nextId = 0;
    var focusedId = 0;
    void emit(_Candidate node, int parentId, int depth) {
      if (nodes.length >= options.maxNodes) {
        walker.truncated = true;
        return;
      }
      final id = ++nextId;
      if (focusedId == 0 &&
          node.data.flagsCollection.isFocused == ui.Tristate.isTrue) {
        focusedId = id;
      }
      nodes.add(_nodeJson(node, id, parentId, depth, options));
      for (final child in node.children) {
        emit(child, id, depth + 1);
      }
    }

    if (candidate != null) {
      emit(candidate, 0, 0);
    }
    final degraded = <String>[];
    if (walker.truncated) {
      degraded.add(
        'PARTIAL: node or depth budget reached; increase max_nodes/max_depth',
      );
    }
    stopwatch.stop();
    _nextRev += 1;
    return ImpresarioSnapshot._(
      rev: _nextRev,
      platform: platform,
      screen: screen,
      nodes: nodes,
      degraded: degraded,
      truncated: walker.truncated,
      focusedId: focusedId,
      acquisitionMs: stopwatch.elapsedMilliseconds,
      timestampUnixMs: timestamp,
    );
  }

  static ImpresarioSnapshot _degraded({
    required String code,
    required String message,
    required String platform,
    required int timestamp,
    required Stopwatch stopwatch,
    ImpresarioScreenInfo? screen,
  }) {
    stopwatch.stop();
    _nextRev += 1;
    return ImpresarioSnapshot._(
      rev: _nextRev,
      platform: platform,
      screen:
          screen ??
          const ImpresarioScreenInfo(
            logicalWidth: 0,
            logicalHeight: 0,
            pixelWidth: 0,
            pixelHeight: 0,
            scale: 1,
            safeInsets: ImpresarioInsets(),
          ),
      nodes: const <Map<String, Object?>>[],
      degraded: <String>['$code: $message'],
      truncated: false,
      focusedId: 0,
      acquisitionMs: stopwatch.elapsedMilliseconds,
      timestampUnixMs: timestamp,
    );
  }
}

String _platformName() {
  if (kIsWeb) {
    return 'web';
  }
  return defaultTargetPlatform.name.toLowerCase();
}

class _Walker {
  _Walker({required this.options, required this.viewport});

  final ImpresarioSnapshotOptions options;
  final Rect viewport;
  bool truncated = false;

  _Candidate? visit(
    SemanticsNode node,
    _Candidate? parent,
    Matrix4 parentTransform,
    Rect inheritedClip,
    int depth,
  ) {
    if (depth > options.maxDepth) {
      truncated = true;
      parent?.hiddenChildren += 1;
      return null;
    }
    // Merged nodes are already represented by their parent's data. Their
    // descendants are merged too, so the subtree can be skipped.
    if (node.isMergedIntoParent) {
      return null;
    }
    final data = node.getSemanticsData();
    final flags = data.flagsCollection;
    if (flags.isHidden) {
      return null;
    }
    final transform = node.transform;
    final global = transform == null
        ? parentTransform
        : (Matrix4.copy(parentTransform)..multiply(transform));
    final rect = _finiteRect(MatrixUtils.transformRect(global, node.rect));
    var clip = inheritedClip;
    final parentClip = node.parentSemanticsClipRect;
    if (parentClip != null) {
      clip = clip.intersect(
        _finiteRect(MatrixUtils.transformRect(global, parentClip)),
      );
    }
    final visible = rect.intersect(clip);
    final candidate = _Candidate(
      node: node,
      data: data,
      rect: rect,
      visible: visible,
      depth: depth,
      parent: parent,
    );
    if (!node.mergeAllDescendantsIntoThisNode) {
      node.visitChildren((SemanticsNode child) {
        final included = visit(child, candidate, global, clip, depth + 1);
        if (included != null) {
          candidate.children.add(included);
        }
        return true;
      });
    }
    final focused = flags.isFocused == ui.Tristate.isTrue;
    final fraction = _visibleFraction(rect, visible);
    final hasVisibleChild = candidate.children.isNotEmpty;
    candidate.visibleFraction = fraction;
    candidate.included =
        focused ||
        hasVisibleChild ||
        (!visible.isEmpty && fraction >= options.minVisibleFraction);
    if (!candidate.included) {
      parent?.hiddenChildren += 1;
      return null;
    }
    return candidate;
  }
}

class _Candidate {
  _Candidate({
    required this.node,
    required this.data,
    required this.rect,
    required this.visible,
    required this.depth,
    required this.parent,
  });

  final SemanticsNode node;
  final SemanticsData data;
  final Rect rect;
  final Rect visible;
  final int depth;
  final _Candidate? parent;
  final List<_Candidate> children = <_Candidate>[];
  bool included = false;
  double visibleFraction = 0;
  int hiddenChildren = 0;
}

Map<String, Object?> _nodeJson(
  _Candidate candidate,
  int id,
  int parentId,
  int depth,
  ImpresarioSnapshotOptions options,
) {
  final data = candidate.data;
  final flags = data.flagsCollection;
  final actions = data.actions;
  final secure = flags.isObscured;
  final label = _normalize(data.label);
  final value = secure && !options.includeSecureValues
      ? ''
      : _normalize(data.value);
  final name = _cap(label.isNotEmpty ? label : value, 200);
  final text = _cap(label, 2000);
  final raw = options.includeRaw
      ? jsonEncode(<String, Object?>{
          'semantics_id': candidate.node.id,
          'flags': impresarioFlagNames(flags),
          'actions': impresarioActionNames(actions),
        })
      : '';
  return <String, Object?>{
    'id': id,
    'parent_id': parentId,
    'role': _role(flags),
    'name': name,
    'text': text,
    'test_id': _cap(data.identifier, 200),
    'raw_id': '',
    'bounds': _rectJson(candidate.rect),
    'viewport_bounds': _rectJson(candidate.visible),
    'focused': flags.isFocused == ui.Tristate.isTrue,
    'enabled': flags.isEnabled != ui.Tristate.isFalse,
    'selected': flags.isSelected == ui.Tristate.isTrue,
    'checked': _checked(flags.isChecked),
    'expanded': _tristate(flags.isExpanded),
    'pressed': null,
    'readonly': flags.isReadOnly,
    'required': flags.isRequired == ui.Tristate.isTrue,
    'hidden': false,
    'secure': secure,
    'system_ui': false,
    'scrollable': _scrollable(actions),
    'can_scroll_backward': _canScrollBackward(actions),
    'can_scroll_forward': _canScrollForward(actions),
    'scroll_axis': _scrollAxis(actions),
    'children_hidden_count': candidate.hiddenChildren,
    'raw': raw,
    'value': value,
    'label': _cap(label, 2000),
    'description': _cap(_normalize(data.hint), 2000),
    'placeholder': '',
    'alt': '',
    'title': _cap(_normalize(data.tooltip), 2000),
    'css_selector': '',
    'xpath': '',
    'visible_fraction': candidate.visibleFraction,
    'escapes_clip': false,
    'depth': depth,
    'text_truncated': text.length < label.length,
  };
}

String _role(ui.SemanticsFlags flags) {
  if (flags.isButton) {
    return 'button';
  }
  if (flags.isTextField) {
    return 'textbox';
  }
  if (flags.isLink) {
    return 'link';
  }
  if (flags.isHeader) {
    return 'heading';
  }
  if (flags.isImage) {
    return 'image';
  }
  if (flags.isSlider) {
    return 'slider';
  }
  if (flags.isChecked != ui.CheckedState.none &&
      flags.isInMutuallyExclusiveGroup) {
    return 'radio';
  }
  if (flags.isToggled != ui.Tristate.none) {
    return 'switch';
  }
  if (flags.isChecked != ui.CheckedState.none) {
    return 'checkbox';
  }
  return 'generic';
}

bool? _checked(ui.CheckedState state) {
  if (state == ui.CheckedState.none) {
    return null;
  }
  return state == ui.CheckedState.isTrue;
}

bool? _tristate(ui.Tristate state) {
  if (state == ui.Tristate.none) {
    return null;
  }
  return state == ui.Tristate.isTrue;
}

bool _canScrollBackward(int actions) {
  return (actions & ui.SemanticsAction.scrollUp.index) != 0 ||
      (actions & ui.SemanticsAction.scrollLeft.index) != 0;
}

bool _canScrollForward(int actions) {
  return (actions & ui.SemanticsAction.scrollDown.index) != 0 ||
      (actions & ui.SemanticsAction.scrollRight.index) != 0;
}

bool _scrollable(int actions) {
  return _canScrollBackward(actions) || _canScrollForward(actions);
}

String _scrollAxis(int actions) {
  final vertical =
      (actions & ui.SemanticsAction.scrollUp.index) != 0 ||
      (actions & ui.SemanticsAction.scrollDown.index) != 0;
  final horizontal =
      (actions & ui.SemanticsAction.scrollLeft.index) != 0 ||
      (actions & ui.SemanticsAction.scrollRight.index) != 0;
  if (vertical && horizontal) {
    return 'both';
  }
  if (vertical) {
    return 'vertical';
  }
  if (horizontal) {
    return 'horizontal';
  }
  return '';
}

Map<String, Object?> _rectJson(Rect rect) {
  final finite = _finiteRect(rect);
  return <String, Object?>{
    'x': finite.left,
    'y': finite.top,
    'w': finite.width,
    'h': finite.height,
  };
}

Rect _finiteRect(Rect rect) {
  if (!rect.isFinite) {
    return Rect.zero;
  }
  return rect;
}

double _visibleFraction(Rect rect, Rect visible) {
  final area = rect.width * rect.height;
  if (area <= 0 || visible.isEmpty) {
    return 0;
  }
  return ((visible.width * visible.height) / area).clamp(0.0, 1.0).toDouble();
}

String _normalize(String value) {
  return value.replaceAll(RegExp(r'\s+'), ' ').trim();
}

String _cap(String value, int max) {
  if (value.length <= max) {
    return value;
  }
  return value.substring(0, max);
}
