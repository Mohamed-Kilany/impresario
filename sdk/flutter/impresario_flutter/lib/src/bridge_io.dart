import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'bridge_base.dart';
import 'service_extension_base.dart';

/// Creates the loopback TCP bridge.
///
/// The bridge speaks newline-delimited JSON:
///
/// ```text
/// -> {"id":1,"method":"snapshot","params":{"max_nodes":500}}
/// <- {"id":1,"ok":true,"result":{...}}
/// <- {"event":"semantics","rev":3}
/// ```
///
/// Only loopback addresses are accepted. If [token] is set, every request must
/// include a matching top-level `token` field.
ImpresarioBridge createBridge({
  required String host,
  required int port,
  required String? token,
  required ImpresarioSnapshotProvider provider,
}) {
  return _SocketBridge(
    host: host,
    port: port,
    token: token == null || token.isEmpty ? null : token,
    provider: provider,
  );
}

class _SocketBridge implements ImpresarioBridge {
  _SocketBridge({
    required this.host,
    required int port,
    required this.token,
    required this.provider,
  }) : _bindPort = port;

  final String host;
  final int _bindPort;
  final String? token;
  final ImpresarioSnapshotProvider provider;
  final Set<_BridgeClient> _clients = <_BridgeClient>{};
  ServerSocket? _server;
  int? _port;

  @override
  Future<void> start() async {
    if (_server != null) {
      return;
    }
    if (host != '127.0.0.1' && host != 'localhost' && host != '::1') {
      throw ArgumentError.value(
        host,
        'host',
        'The Impresario bridge only binds to loopback',
      );
    }
    final server = await ServerSocket.bind(host, _bindPort, shared: false);
    _server = server;
    _port = server.port;
    server.listen(_accept, onError: (Object _) {});
  }

  void _accept(Socket socket) {
    socket.setOption(SocketOption.tcpNoDelay, true);
    late final _BridgeClient client;
    client = _BridgeClient(
      socket: socket,
      token: token,
      provider: provider,
      onClosed: () => _clients.remove(client),
    );
    _clients.add(client);
    client.start();
  }

  @override
  Future<void> stop() async {
    for (final client in _clients.toList(growable: false)) {
      client.close();
    }
    _clients.clear();
    await _server?.close();
    _server = null;
    _port = null;
  }

  @override
  int? get port => _port;

  @override
  void notifyChanged(int rev) {
    for (final client in _clients) {
      client.notifyChanged(rev);
    }
  }
}

class _BridgeClient {
  _BridgeClient({
    required this.socket,
    required this.token,
    required this.provider,
    required this.onClosed,
  });

  static const int _maxRequestBytes = 1024 * 1024;

  final Socket socket;
  final String? token;
  final ImpresarioSnapshotProvider provider;
  final void Function() onClosed;
  final List<int> _buffer = <int>[];
  StreamSubscription<Uint8List>? _subscription;
  bool _closed = false;

  void start() {
    socket.write(
      '${jsonEncode(<String, Object?>{
        'event': 'hello',
        'protocol': 1,
        'framework': 'flutter',
        'methods': <String>['snapshot', 'ping', 'capabilities'],
      })}\n',
    );
    _subscription = socket.listen(
      _onData,
      onError: (Object _) => close(),
      onDone: close,
      cancelOnError: true,
    );
  }

  void _onData(Uint8List data) {
    if (_closed) {
      return;
    }
    _buffer.addAll(data);
    if (_buffer.length > _maxRequestBytes) {
      _sendError(null, 'REQUEST_TOO_LARGE', 'Request exceeds 1 MiB.');
      close();
      return;
    }
    while (true) {
      final index = _buffer.indexOf(10);
      if (index < 0) {
        break;
      }
      final line = utf8.decode(_buffer.sublist(0, index), allowMalformed: true);
      _buffer.removeRange(0, index + 1);
      _handleLine(line);
    }
  }

  void _handleLine(String line) {
    if (_closed || line.trim().isEmpty) {
      return;
    }
    Object? decoded;
    try {
      decoded = jsonDecode(line);
    } on FormatException {
      _sendError(null, 'BAD_REQUEST', 'Expected one JSON object per line.');
      return;
    }
    if (decoded is! Map<String, dynamic>) {
      _sendError(null, 'BAD_REQUEST', 'Expected a JSON object.');
      return;
    }
    final id = decoded['id'];
    if (token != null && decoded['token'] != token) {
      _sendError(id, 'UNAUTHORIZED', 'A valid bridge token is required.');
      close();
      return;
    }
    final method = decoded['method'];
    switch (method) {
      case 'ping':
        _sendResult(id, <String, Object?>{'pong': true, 'protocol': 1});
        break;
      case 'capabilities':
        _sendResult(id, <String, Object?>{
          'protocol': 1,
          'framework': 'flutter',
          'snapshot': true,
          'push': true,
          'input': false,
        });
        break;
      case 'snapshot':
        final rawParams = decoded['params'];
        final params = <String, Object?>{};
        if (rawParams is Map) {
          rawParams.forEach((Object? key, Object? value) {
            params[key.toString()] = value;
          });
        }
        try {
          _sendResult(id, provider(params));
        } catch (error) {
          _sendError(id, 'SNAPSHOT_FAILED', error.toString());
        }
        break;
      default:
        _sendError(id, 'UNKNOWN_METHOD', 'Unknown method: $method');
    }
  }

  void _sendResult(Object? id, Map<String, Object?> result) {
    _write(<String, Object?>{'id': id, 'ok': true, 'result': result});
  }

  void _sendError(Object? id, String code, String message) {
    _write(<String, Object?>{
      'id': id,
      'ok': false,
      'error': <String, Object?>{'code': code, 'message': message},
    });
  }

  void _write(Map<String, Object?> message) {
    if (_closed) {
      return;
    }
    try {
      socket.write('${jsonEncode(message)}\n');
    } catch (_) {
      close();
    }
  }

  void notifyChanged(int rev) {
    _write(<String, Object?>{'event': 'semantics', 'rev': rev});
  }

  void close() {
    if (_closed) {
      return;
    }
    _closed = true;
    _subscription?.cancel();
    _subscription = null;
    socket.destroy();
    onClosed();
  }
}
