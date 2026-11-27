import 'package:flutter/material.dart';
import 'package:impresario_flutter/impresario_flutter.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // Opt in explicitly. The default session keeps semantics enabled, registers
  // the `ext.impresario.snapshot` VM-service extension and listens on
  // 127.0.0.1:7312 for the Impresario inspector.
  await Impresario.start();
  runApp(const ImpresarioExampleApp());
}

class ImpresarioExampleApp extends StatelessWidget {
  const ImpresarioExampleApp({super.key});

  @override
  Widget build(BuildContext context) {
    return const MaterialApp(
      title: 'Impresario Flutter example',
      home: ExampleHomePage(),
    );
  }
}

class ExampleHomePage extends StatefulWidget {
  const ExampleHomePage({super.key});

  @override
  State<ExampleHomePage> createState() => _ExampleHomePageState();
}

class _ExampleHomePageState extends State<ExampleHomePage> {
  int _count = 0;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Impresario Flutter example')),
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: <Widget>[
            ImpresarioTag(id: 'counter.value', child: Text('Count: $_count')),
            const SizedBox(height: 16),
            ImpresarioTag(
              id: 'counter.increment',
              child: ElevatedButton(
                onPressed: () => setState(() => _count += 1),
                child: const Text('Increment'),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
