import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:impresario_flutter/impresario_flutter.dart';

void main() {
  testWidgets('bridge serves snapshots over loopback', (tester) async {
    final handle = tester.ensureSemantics();
    try {
      await tester.pumpWidget(
        const MaterialApp(
          home: Scaffold(
            body: ImpresarioTag(id: 'bridge.tag', child: Text('Bridge')),
          ),
        ),
      );
      await tester.pump();

      await tester.runAsync(() async {
        final session = await ImpresarioSession.start(
          enableServiceExtension: false,
          bridgePort: 0,
        );
        try {
          expect(session.bridgeError, isNull);
          final port = session.bridgePort;
          expect(port, isNotNull);

          final socket = await Socket.connect('127.0.0.1', port!);
          try {
            final response = Completer<Map<String, dynamic>>();
            final subscription = socket
                .cast<List<int>>()
                .transform(utf8.decoder)
                .transform(const LineSplitter())
                .listen((String line) {
                  final decoded = jsonDecode(line);
                  if (decoded is Map<String, dynamic> && decoded['id'] == 1) {
                    if (!response.isCompleted) {
                      response.complete(decoded);
                    }
                  }
                });
            socket.write(
              '${jsonEncode(<String, Object?>{
                'id': 1,
                'method': 'snapshot',
                'params': <String, Object?>{'max_nodes': 500},
              })}\n',
            );
            await socket.flush();

            final message = await response.future.timeout(
              const Duration(seconds: 5),
            );
            expect(message['ok'], isTrue);
            final result = message['result'] as Map<String, dynamic>;
            final nodes = result['nodes'] as List<dynamic>;
            expect(
              nodes.any(
                (dynamic node) =>
                    node is Map<String, dynamic> &&
                    node['test_id'] == 'bridge.tag',
              ),
              isTrue,
            );
            await subscription.cancel();
          } finally {
            socket.destroy();
          }
        } finally {
          await session.stop();
        }
      });
    } finally {
      handle.dispose();
    }
  });

  testWidgets('bridge rejects requests without its token', (tester) async {
    final handle = tester.ensureSemantics();
    try {
      await tester.pumpWidget(
        const MaterialApp(home: Scaffold(body: SizedBox())),
      );
      await tester.pump();

      await tester.runAsync(() async {
        final session = await ImpresarioSession.start(
          enableServiceExtension: false,
          bridgePort: 0,
          bridgeToken: 'secret',
        );
        try {
          expect(session.bridgeError, isNull);
          final port = session.bridgePort;
          expect(port, isNotNull);

          final socket = await Socket.connect('127.0.0.1', port!);
          try {
            final response = Completer<Map<String, dynamic>>();
            final subscription = socket
                .cast<List<int>>()
                .transform(utf8.decoder)
                .transform(const LineSplitter())
                .listen((String line) {
                  final decoded = jsonDecode(line);
                  if (decoded is Map<String, dynamic> && decoded['id'] == 1) {
                    if (!response.isCompleted) {
                      response.complete(decoded);
                    }
                  }
                });
            socket.write(
              '${jsonEncode(<String, Object?>{'id': 1, 'method': 'snapshot'})}\n',
            );
            await socket.flush();

            final message = await response.future.timeout(
              const Duration(seconds: 5),
            );
            expect(message['ok'], isFalse);
            final error = message['error'] as Map<String, dynamic>;
            expect(error['code'], 'UNAUTHORIZED');
            await subscription.cancel();
          } finally {
            socket.destroy();
          }
        } finally {
          await session.stop();
        }
      });
    } finally {
      handle.dispose();
    }
  });
}
