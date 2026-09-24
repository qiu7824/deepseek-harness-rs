import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/attachment_view.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class FileApi extends DshClient {
  FileApi() : super('http://127.0.0.1:1');
  final reads = <({String path, String? session, RequestScope? scope})>[];
  final pending = Completer<Json>();
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    final uri = Uri.parse(path);
    if (uri.path.endsWith('/list')) return {'entries': []};
    reads.add((
      path: uri.queryParameters['path']!,
      session: uri.queryParameters['sessionId'],
      scope: scope,
    ));
    return pending.future;
  }
}

void main() {
  test(
    'preview uses workspace-relative paths without accepting other roots',
    () {
      expect(
        workspacePreviewPath(
          r'\\?\E:\project\.dsh-attachments\a\b\1.pdf',
          r'e:\project',
        ),
        '.dsh-attachments/a/b/1.pdf',
      );
      expect(
        workspacePreviewPath(
          r'\\?\UNC\server\share\p\a.pdf',
          r'\\server\share\p',
        ),
        'a.pdf',
      );
      expect(workspacePreviewPath('/project/a.pdf', '/project/'), 'a.pdf');
      for (final path in [
        r'E:\project-other\a.pdf',
        r'E:\project\..\a.pdf',
        r'F:\project\a.pdf',
      ]) {
        expect(workspacePreviewPath(path, r'E:\project'), path);
      }
      expect(workspacePreviewPath('notes.md', r'E:\project'), 'notes.md');
    },
  );
  testWidgets(
    'card sits outside bubble, preserves filename and size, and opens the exact path',
    (tester) async {
      const path = r'E:\project\.dsh-attachments\opaque\file.pdf';
      String? opened;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 700,
              child: MessageCard(
                item: TranscriptItem(
                  id: 'u',
                  kind: 'user',
                  text: '查看附件',
                  files: [const UploadedFileReceipt('文件.pdf', path, 1068760)],
                ),
                onOpenPath: (value) => opened = value,
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final card = find.byType(UploadedFileCard),
          bubble = find.byKey(const ValueKey('bubble-u'));
      expect(find.text('文件.pdf'), findsOneWidget);
      expect(find.text('1.0 MiB'), findsOneWidget);
      expect(find.text(path), findsNothing);
      expect(
        tester.getBottomRight(card).dy,
        lessThan(tester.getTopLeft(bubble).dy),
      );
      expect(tester.getSize(card).height, closeTo(36, 2));
      expect(opened, isNull);
      await tester.tap(find.text('文件.pdf'));
      expect(opened, path);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'file-only rows have no empty bubble, long filenames fit narrow panes',
    (tester) async {
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 260,
              child: MessageCard(
                item: TranscriptItem(
                  id: 'u',
                  kind: 'user',
                  text: '',
                  files: [UploadedFileReceipt('很长的文件名' * 40, 'path', 300)],
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('bubble-u')), findsNothing);
      expect(find.text('300 B'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'workbench opens requested file in its session and cancels a closed read',
    (tester) async {
      final api = FileApi();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: FilePanel(
              api: api,
              session: 'owner',
              cwd: r'E:\project',
              fileRequest: const FileOpenRequest(r'E:\project\notes.md'),
            ),
          ),
        ),
      );
      await tester.pump();
      expect(api.reads.single.path, 'notes.md');
      expect(api.reads.single.session, 'owner');
      final request = api.reads.single.scope!;
      await tester.pumpWidget(const SizedBox());
      expect(request.cancelled, isTrue);
      api.pending.complete({'text': 'late'});
      await tester.pump();
      expect(tester.takeException(), isNull);
      await api.close();
    },
  );
}
