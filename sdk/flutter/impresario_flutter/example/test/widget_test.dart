import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:impresario_flutter/impresario_flutter.dart';
import 'package:impresario_flutter_example/main.dart';

void main() {
  testWidgets('example exposes tagged controls', (tester) async {
    final handle = Impresario.ensureSemantics();
    try {
      await tester.pumpWidget(const MaterialApp(home: ExampleHomePage()));
      await tester.pump();

      final snapshot = ImpresarioSnapshot.capture();
      expect(
        snapshot.nodes.any(
          (Map<String, Object?> node) => node['test_id'] == 'counter.value',
        ),
        isTrue,
      );
      expect(
        snapshot.nodes.any(
          (Map<String, Object?> node) => node['test_id'] == 'counter.increment',
        ),
        isTrue,
      );

      await tester.tap(find.text('Increment'));
      await tester.pump();
      expect(find.text('Count: 1'), findsOneWidget);
    } finally {
      handle.dispose();
    }
  });
}
