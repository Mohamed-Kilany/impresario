import 'dart:convert';
import 'dart:ui' as ui;

import 'package:flutter/semantics.dart';

/// One Flutter semantics node, detached from the live [SemanticsNode] tree.
///
/// The inspector never sends a live `SemanticsNode` over the wire: the package
/// copies the fields it needs so the snapshot walker can run in tests and in
/// release builds without retaining framework objects.
class ImpresarioSemanticsNode {
  const ImpresarioSemanticsNode({
    required this.semanticsId,
    required this.rect,
    this.transform,
    this.identifier = '',
    this.label = '',
    this.value = '',
    this.hint = '',
    this.tooltip = '',
    this.isButton = false,
    this.isTextField = false,
    this.isLink = false,
    this.isHeader = false,
    this.isImage = false,
    this.isSlider = false,
    this.isInMutuallyExclusiveGroup = false,
    this.isChecked,
    this.isToggled,
    this.isExpanded,
    this.isSelected = false,
    this.isEnabled = true,
    this.isFocused = false,
    this.isHidden = false,
    this.isReadOnly = false,
    this.isRequired = false,
    this.isObscured = false,
    this.isMultiline = false,
    this.hasImplicitScrolling = false,
    this.canScrollBackward = false,
    this.canScrollForward = false,
    this.scrollAxis = '',
    this.isMergedIntoParent = false,
    this.isInvisible = false,
    this.flags = const <String>[],
    this.actions = const <String>[],
    this.children = const <ImpresarioSemanticsNode>[],
  });

  /// Copies a live Flutter semantics subtree into plain values.
  factory ImpresarioSemanticsNode.fromFlutter(SemanticsNode node) {
    final data = node.getSemanticsData();
    final flags = data.flagsCollection;
    final actions = data.actions;
    final actionNames = impresarioActionNames(actions);
    final flagNames = impresarioFlagNames(flags);
    final children = <ImpresarioSemanticsNode>[];
    node.visitChildren((SemanticsNode child) {
      children.add(ImpresarioSemanticsNode.fromFlutter(child));
      return true;
    });
    return ImpresarioSemanticsNode(
      semanticsId: node.id,
      rect: node.rect,
      transform: node.transform,
      identifier: data.identifier,
      label: data.label,
      value: data.value,
      hint: data.hint,
      tooltip: data.tooltip,
      isButton: flags.isButton,
      isTextField: flags.isTextField,
      isLink: flags.isLink,
      isHeader: flags.isHeader,
      isImage: flags.isImage,
      isSlider: flags.isSlider,
      isInMutuallyExclusiveGroup: flags.isInMutuallyExclusiveGroup,
      isChecked: _checked(flags.isChecked),
      isToggled: _tristate(flags.isToggled),
      isExpanded: _tristate(flags.isExpanded),
      isSelected: flags.isSelected == ui.Tristate.isTrue,
      isEnabled: flags.isEnabled != ui.Tristate.isFalse,
      isFocused: flags.isFocused == ui.Tristate.isTrue,
      isHidden: flags.isHidden,
      isReadOnly: flags.isReadOnly,
      isRequired: flags.isRequired == ui.Tristate.isTrue,
      isObscured: flags.isObscured,
      isMultiline: flags.isMultiline,
      hasImplicitScrolling: flags.hasImplicitScrolling,
      canScrollBackward:
          _hasAction(actions, ui.SemanticsAction.scrollUp) ||
          _hasAction(actions, ui.SemanticsAction.scrollLeft),
      canScrollForward:
          _hasAction(actions, ui.SemanticsAction.scrollDown) ||
          _hasAction(actions, ui.SemanticsAction.scrollRight),
      scrollAxis: _scrollAxis(actions),
      isMergedIntoParent: node.isMergedIntoParent,
      isInvisible: node.isInvisible,
      flags: flagNames,
      actions: actionNames,
      children: children,
    );
  }

  /// Flutter's own node id; kept for diagnostics and the raw payload.
  final int semanticsId;
  final Rect rect;
  final Matrix4? transform;
  final String identifier;
  final String label;
  final String value;
  final String hint;
  final String tooltip;
  final bool isButton;
  final bool isTextField;
  final bool isLink;
  final bool isHeader;
  final bool isImage;
  final bool isSlider;
  final bool isInMutuallyExclusiveGroup;
  final bool? isChecked;
  final bool? isToggled;
  final bool? isExpanded;
  final bool isSelected;
  final bool isEnabled;
  final bool isFocused;
  final bool isHidden;
  final bool isReadOnly;
  final bool isRequired;
  final bool isObscured;
  final bool isMultiline;
  final bool hasImplicitScrolling;
  final bool canScrollBackward;
  final bool canScrollForward;
  final String scrollAxis;
  final bool isMergedIntoParent;
  final bool isInvisible;
  final List<String> flags;
  final List<String> actions;
  final List<ImpresarioSemanticsNode> children;

  /// The normalized role name shared with `impresario-locator`.
  String get role {
    if (isButton) {
      return 'button';
    }
    if (isTextField) {
      return 'textbox';
    }
    if (isLink) {
      return 'link';
    }
    if (isHeader) {
      return 'heading';
    }
    if (isImage) {
      return 'image';
    }
    if (isSlider) {
      return 'slider';
    }
    if (isChecked != null && isInMutuallyExclusiveGroup) {
      return 'radio';
    }
    if (isToggled != null) {
      return 'switch';
    }
    if (isChecked != null) {
      return 'checkbox';
    }
    return 'generic';
  }

  /// Optional raw payload used by the inspector's `include_raw` mode.
  String rawJson() {
    return jsonEncode(<String, Object?>{
      'semantics_id': semanticsId,
      if (flags.isNotEmpty) 'flags': flags,
      if (actions.isNotEmpty) 'actions': actions,
    });
  }
}

/// Names of the Flutter semantics flags set on [flags].
///
/// The names match `SemanticsFlag` identifiers so the host can feed them to
/// `impresario-locator`'s `role_from_flutter_flags` unchanged.
List<String> impresarioFlagNames(ui.SemanticsFlags flags) {
  return <String>[
    if (flags.isButton) 'isButton',
    if (flags.isTextField) 'isTextField',
    if (flags.isLink) 'isLink',
    if (flags.isHeader) 'isHeader',
    if (flags.isImage) 'isImage',
    if (flags.isSlider) 'isSlider',
    if (flags.isInMutuallyExclusiveGroup) 'isInMutuallyExclusiveGroup',
    if (flags.isChecked != ui.CheckedState.none) 'hasCheckedState',
    if (flags.isChecked == ui.CheckedState.isTrue) 'isChecked',
    if (flags.isChecked == ui.CheckedState.mixed) 'isCheckStateMixed',
    if (flags.isToggled != ui.Tristate.none) 'hasToggledState',
    if (flags.isToggled == ui.Tristate.isTrue) 'isToggled',
    if (flags.isExpanded != ui.Tristate.none) 'hasExpandedState',
    if (flags.isExpanded == ui.Tristate.isTrue) 'isExpanded',
    if (flags.isSelected != ui.Tristate.none) 'hasSelectedState',
    if (flags.isSelected == ui.Tristate.isTrue) 'isSelected',
    if (flags.isEnabled != ui.Tristate.none) 'hasEnabledState',
    if (flags.isEnabled == ui.Tristate.isTrue) 'isEnabled',
    if (flags.isFocused == ui.Tristate.isTrue) 'isFocused',
    if (flags.isHidden) 'isHidden',
    if (flags.isReadOnly) 'isReadOnly',
    if (flags.isRequired != ui.Tristate.none) 'hasRequiredState',
    if (flags.isRequired == ui.Tristate.isTrue) 'isRequired',
    if (flags.isObscured) 'isObscured',
    if (flags.isMultiline) 'isMultiline',
    if (flags.hasImplicitScrolling) 'hasImplicitScrolling',
    if (flags.isLiveRegion) 'isLiveRegion',
    if (flags.isKeyboardKey) 'isKeyboardKey',
    if (flags.isAccessibilityFocusBlocked) 'isAccessibilityFocusBlocked',
    if (flags.scopesRoute) 'scopesRoute',
    if (flags.namesRoute) 'namesRoute',
  ];
}

/// Names of the Flutter semantics actions set in [actions].
List<String> impresarioActionNames(int actions) {
  return <String>[
    for (final entry in _knownActions)
      if (_hasAction(actions, entry.$2)) entry.$1,
  ];
}

const List<(String, ui.SemanticsAction)> _knownActions =
    <(String, ui.SemanticsAction)>[
      ('tap', ui.SemanticsAction.tap),
      ('longPress', ui.SemanticsAction.longPress),
      ('scrollLeft', ui.SemanticsAction.scrollLeft),
      ('scrollRight', ui.SemanticsAction.scrollRight),
      ('scrollUp', ui.SemanticsAction.scrollUp),
      ('scrollDown', ui.SemanticsAction.scrollDown),
      ('increase', ui.SemanticsAction.increase),
      ('decrease', ui.SemanticsAction.decrease),
      ('showOnScreen', ui.SemanticsAction.showOnScreen),
      ('setText', ui.SemanticsAction.setText),
      ('setSelection', ui.SemanticsAction.setSelection),
      ('focus', ui.SemanticsAction.focus),
      ('dismiss', ui.SemanticsAction.dismiss),
      ('customAction', ui.SemanticsAction.customAction),
    ];

bool _hasAction(int actions, ui.SemanticsAction action) {
  // SemanticsAction.index is already the bit mask, not a bit position.
  return (actions & action.index) != 0;
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

String _scrollAxis(int actions) {
  final vertical =
      _hasAction(actions, ui.SemanticsAction.scrollUp) ||
      _hasAction(actions, ui.SemanticsAction.scrollDown);
  final horizontal =
      _hasAction(actions, ui.SemanticsAction.scrollLeft) ||
      _hasAction(actions, ui.SemanticsAction.scrollRight);
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
