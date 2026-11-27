import 'dart:ui';

import 'package:flutter_test/flutter_test.dart';
import 'package:impresario_flutter/impresario_flutter.dart';

void main() {
  test('roles follow the shared Flutter mapping', () {
    expect(
      const ImpresarioSemanticsNode(
        semanticsId: 1,
        rect: Rect.zero,
        isButton: true,
      ).role,
      'button',
    );
    expect(
      const ImpresarioSemanticsNode(
        semanticsId: 1,
        rect: Rect.zero,
        isTextField: true,
      ).role,
      'textbox',
    );
    expect(
      const ImpresarioSemanticsNode(
        semanticsId: 1,
        rect: Rect.zero,
        isChecked: true,
      ).role,
      'checkbox',
    );
    expect(
      const ImpresarioSemanticsNode(
        semanticsId: 1,
        rect: Rect.zero,
        isChecked: true,
        isInMutuallyExclusiveGroup: true,
      ).role,
      'radio',
    );
    expect(
      const ImpresarioSemanticsNode(
        semanticsId: 1,
        rect: Rect.zero,
        isToggled: false,
      ).role,
      'switch',
    );
    expect(
      const ImpresarioSemanticsNode(semanticsId: 1, rect: Rect.zero).role,
      'generic',
    );
  });

  test('raw payload keeps the Flutter semantics id', () {
    const node = ImpresarioSemanticsNode(
      semanticsId: 42,
      rect: Rect.zero,
      flags: <String>['isButton'],
      actions: <String>['tap'],
    );
    expect(node.rawJson(), contains('"semantics_id":42'));
    expect(node.rawJson(), contains('"isButton"'));
    expect(node.rawJson(), contains('"tap"'));
  });
}
