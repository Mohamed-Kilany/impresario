import 'package:flutter/semantics.dart';
import 'package:flutter/widgets.dart';

import 'src/options.dart';
import 'src/session.dart';

export 'src/options.dart';
export 'src/semantics.dart' show ImpresarioSemanticsNode;
export 'src/session.dart' show ImpresarioSession;
export 'src/snapshot.dart' show ImpresarioSnapshot;
export 'src/tag.dart' show ImpresarioTag;

/// Entry points for the opt-in Impresario Flutter package.
class Impresario {
  const Impresario._();

  /// Keeps the Flutter semantics tree enabled without a screen reader.
  ///
  /// Dispose the returned handle to release the request. Calling this more
  /// than once is safe; the framework reference-counts handles.
  static SemanticsHandle ensureSemantics() {
    WidgetsFlutterBinding.ensureInitialized();
    return SemanticsBinding.instance.ensureSemantics();
  }

  /// Whether the semantics tree is currently being generated.
  static bool get semanticsEnabled =>
      SemanticsBinding.instance.semanticsEnabled;

  /// Starts a snapshot session. See [ImpresarioSession.start].
  static Future<ImpresarioSession> start({
    ImpresarioSnapshotOptions options = const ImpresarioSnapshotOptions(),
    bool enableSemantics = true,
    bool enableServiceExtension = true,
    int? bridgePort = 7312,
    String bridgeHost = '127.0.0.1',
    String? bridgeToken,
  }) {
    return ImpresarioSession.start(
      options: options,
      enableSemantics: enableSemantics,
      enableServiceExtension: enableServiceExtension,
      bridgePort: bridgePort,
      bridgeHost: bridgeHost,
      bridgeToken: bridgeToken,
    );
  }
}
