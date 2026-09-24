import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:dsh_desktop/src/resource_diagnostics.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class SourceApi extends DshClient {
  SourceApi(this.text) : super('http://127.0.0.1');
  final String text;
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    final operation = Uri.parse(path).path;
    if (operation.endsWith('/list')) {
      return {
        'path': '',
        'entries': [
          {'name': 'huge.txt', 'path': 'huge.txt', 'kind': 'file'},
        ],
      };
    }
    if (operation.endsWith('/source')) return {'text': text};
    throw StateError('Unexpected operation: $operation');
  }
}

void main() {
  testWidgets('closing a long source releases its line index and cached text', (
    tester,
  ) async {
    final api = SourceApi('a\n' * 100000);
    await tester.binding.setSurfaceSize(const Size(1100, 800));
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: FilePanel(api: api, session: 'fixture', cwd: 'E:/fixture'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('huge.txt'));
    await tester.pumpAndSettle();
    final viewer =
        tester.state(find.byType(NativeFileViewer)) as ResourceDiagnostics;
    expect(viewer.resourceDiagnostics['indexedLinesBytes'], 100001 * 4);
    final panel = tester.state(find.byType(FilePanel)) as ResourceDiagnostics;
    expect(panel.resourceDiagnostics['openFileTabs'], 1);
    expect(panel.resourceDiagnostics['documentCacheBytes'], greaterThan(0));
    await tester.tap(find.byTooltip('关闭 huge.txt'));
    await tester.pumpAndSettle();
    expect(find.byType(NativeFileViewer), findsNothing);
    expect(panel.resourceDiagnostics['openFileTabs'], 0);
    expect(panel.resourceDiagnostics['documentCacheBytes'], 0);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    await api.close();
    await tester.binding.setSurfaceSize(null);
  });
}
