import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:impresario_flutter/impresario_flutter.dart';

void main() {
  testWidgets('tagged button is selectable by test id', (tester) async {
    final handle = tester.ensureSemantics();
    try {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Center(
              child: ImpresarioTag(
                id: 'login.submit',
                child: ElevatedButton(
                  onPressed: () {},
                  child: const Text('Submit'),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pump();

      final snapshot = ImpresarioSnapshot.capture();
      expect(snapshot.degraded, isEmpty);
      expect(snapshot.screen.logicalWidth, greaterThan(0));
      expect(snapshot.screen.logicalHeight, greaterThan(0));

      final tagged = snapshot.nodes
          .where(
            (Map<String, Object?> node) => node['test_id'] == 'login.submit',
          )
          .toList();
      expect(tagged, hasLength(1));

      final buttons = snapshot.nodes
          .where((Map<String, Object?> node) => node['role'] == 'button')
          .toList();
      expect(buttons, isNotEmpty);
      expect(
        buttons.any((Map<String, Object?> node) => node['name'] == 'Submit'),
        isTrue,
      );
    } finally {
      handle.dispose();
    }
  });

  testWidgets('label fallback uses the accessible name', (tester) async {
    final handle = tester.ensureSemantics();
    try {
      await tester.pumpWidget(
        const MaterialApp(
          home: Scaffold(
            body: ImpresarioTag.labelFallback(
              id: 'legacy.tag',
              child: Text('Legacy'),
            ),
          ),
        ),
      );
      await tester.pump();

      final snapshot = ImpresarioSnapshot.capture();
      expect(
        snapshot.nodes.any(
          (Map<String, Object?> node) => node['test_id'] == 'legacy.tag',
        ),
        isFalse,
      );
      expect(
        snapshot.nodes.any(
          (Map<String, Object?> node) =>
              node['test_id'] == '' &&
              (node['name']! as String).contains('legacy.tag'),
        ),
        isTrue,
      );
    } finally {
      handle.dispose();
    }
  });

  testWidgets('offscreen subtrees are pruned', (tester) async {
    final handle = tester.ensureSemantics();
    try {
      await tester.pumpWidget(
        const MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: Column(
                children: <Widget>[
                  SizedBox(
                    height: 100,
                    child: ImpresarioTag(
                      id: 'visible.item',
                      child: Text('Visible'),
                    ),
                  ),
                  SizedBox(height: 1000),
                  SizedBox(
                    height: 100,
                    child: ImpresarioTag(
                      id: 'offscreen.item',
                      child: Text('Offscreen'),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      );
      await tester.pump();

      final snapshot = ImpresarioSnapshot.capture();
      expect(
        snapshot.nodes.any(
          (Map<String, Object?> node) => node['test_id'] == 'visible.item',
        ),
        isTrue,
      );
      expect(
        snapshot.nodes.any(
          (Map<String, Object?> node) => node['test_id'] == 'offscreen.item',
        ),
        isFalse,
      );
    } finally {
      handle.dispose();
    }
  });

  testWidgets('minVisibleFraction drops mostly offscreen nodes', (
    tester,
  ) async {
    final handle = tester.ensureSemantics();
    try {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Transform.translate(
              offset: const Offset(0, 550),
              child: const Align(
                alignment: Alignment.topLeft,
                child: SizedBox(
                  width: 800,
                  height: 100,
                  child: ImpresarioTag(
                    id: 'sliver.item',
                    child: SizedBox.expand(),
                  ),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pump();

      final all = ImpresarioSnapshot.capture();
      final strict = ImpresarioSnapshot.capture(
        options: const ImpresarioSnapshotOptions(minVisibleFraction: 0.75),
      );
      final allItem = all.nodes
          .where(
            (Map<String, Object?> node) => node['test_id'] == 'sliver.item',
          )
          .toList();
      expect(allItem, hasLength(1));
      expect(allItem.single['visible_fraction'], closeTo(0.5, 0.01));
      expect(
        strict.nodes.any(
          (Map<String, Object?> node) => node['test_id'] == 'sliver.item',
        ),
        isFalse,
      );
    } finally {
      handle.dispose();
    }
  });

  testWidgets('raw mode carries Flutter flags and semantics id', (
    tester,
  ) async {
    final handle = tester.ensureSemantics();
    try {
      await tester.pumpWidget(
        const MaterialApp(
          home: Scaffold(
            body: ImpresarioTag(
              id: 'raw.button',
              child: TextButton(onPressed: null, child: Text('Raw')),
            ),
          ),
        ),
      );
      await tester.pump();

      final snapshot = ImpresarioSnapshot.capture(
        options: const ImpresarioSnapshotOptions(includeRaw: true),
      );
      final tagged = snapshot.nodes.firstWhere(
        (Map<String, Object?> node) => node['test_id'] == 'raw.button',
      );
      expect(tagged['raw'], contains('"semantics_id"'));
    } finally {
      handle.dispose();
    }
  });

  testWidgets('session captures and stops', (tester) async {
    final handle = tester.ensureSemantics();
    try {
      await tester.pumpWidget(
        const MaterialApp(
          home: Scaffold(
            body: ImpresarioTag(id: 'session.tag', child: Text('Session')),
          ),
        ),
      );
      await tester.pump();

      final session = await ImpresarioSession.start(
        enableServiceExtension: false,
        bridgePort: null,
      );
      expect(session.isActive, isTrue);
      expect(session.bridgePort, isNull);
      expect(session.bridgeError, isNull);
      final snapshot = session.snapshot();
      expect(
        snapshot.nodes.any(
          (Map<String, Object?> node) => node['test_id'] == 'session.tag',
        ),
        isTrue,
      );
      await session.stop();
      expect(session.isActive, isFalse);
      expect(() => session.snapshot(), throwsStateError);
    } finally {
      handle.dispose();
    }
  });
}
