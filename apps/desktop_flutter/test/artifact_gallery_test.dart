import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/artifacts_view.dart';
import 'package:dsh_desktop/features/conversation/artifact_changes_view.dart';
import 'package:dsh_desktop/features/conversation/artifact_types.dart';
import 'package:dsh_desktop/features/conversation/permission_control.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'review_actions_test.dart' show ReviewApi, PermissionTestController;
import 'sidebar_interaction_repair_test.dart' show mountRepair;

class GalleryApi extends DshClient {
  GalleryApi() : super('http://127.0.0.1:1');
  final scans = <bool>[];
  final paths = <String>[];
  final scan = Completer<Json>();
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    expect(mutation, isFalse);
    paths.add(path);
    if (path == '/__dsh-artifacts/list') {
      final full = body?['refresh'] == true;
      scans.add(full);
      if (full) return scan.future;
      return {
        'workspaceScanned': false,
        'entries': [
          {
            'path': 'report.md',
            'change': 'presented',
            'source': 'delivery',
            'etag': 'a',
          },
          {
            'path': 'figure.png',
            'change': 'created',
            'source': 'tool',
            'etag': 'b',
          },
        ],
      };
    }
    if (Uri.parse(path).path == '/__dsh-preview/turn-changes') {
      return {
        'turn': 2,
        'files': [
          {
            'path': 'report.md',
            'kind': 'modified',
            'before': 'old line\n',
            'after': 'new line\n',
          },
        ],
      };
    }
    if (Uri.parse(path).path == '/__dsh-preview/git-status') {
      return {
        'branch': 'main',
        'entries': [
          {'path': 'check.py', 'group': 'unstaged', 'status': 'M'},
        ],
      };
    }
    if (Uri.parse(path).path == '/__dsh-preview/git-diff') {
      return {'diff': '@@ -1 +1 @@\n-old\n+new\n'};
    }
    return {};
  }
}

void main() {
  testWidgets('workspace differences use the read-only Git viewer', (
    tester,
  ) async {
    final api = GalleryApi()
      ..scan.complete({'entries': [], 'workspaceScanned': true});
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: ArtifactsView(api: api, session: 's'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('artifact-section-git')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('check.py'));
    await tester.pumpAndSettle();
    expect(
      api.paths.any(
        (path) => Uri.parse(path).path == '/__dsh-preview/git-diff',
      ),
      isTrue,
    );
    expect(find.text('+new'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    await api.close();
  });
  test('diff retains exact before and after lines, including bounded large replacements', () {
    for (final pair in <(String?, String?)>[
      (null, 'new\n'),
      ('gone\n', null),
      ('a\nb\nc\n', 'a\nchanged\nc\n'),
      ('a\r\nb\r\n', 'a\r\nx\r\n'),
      (
        List.generate(1100, (i) => 'before $i').join('\n'),
        List.generate(1100, (i) => 'after $i').join('\n'),
      ),
    ]) {
      final rows = artifactDiffLines(pair.$1, pair.$2);
      expect(
        rows.where((r) => r.kind != 'added').map((r) => r.text).join('\n') +
            ((pair.$1?.endsWith('\n') ?? false) ? '\n' : ''),
        (pair.$1 ?? '').replaceAll('\r\n', '\n'),
      );
      expect(
        rows.where((r) => r.kind != 'removed').map((r) => r.text).join('\n') +
            ((pair.$2?.endsWith('\n') ?? false) ? '\n' : ''),
        (pair.$2 ?? '').replaceAll('\r\n', '\n'),
      );
    }
    expect(
      artifactDiffLines(
        null,
        'new\n',
      ).where((row) => row.kind == 'added').length,
      1,
    );
  });
  test('artifact categories distinguish reader deliverables from code', () {
    expect(artifactCategory(r'报告\FINAL.XLSX'), 'sheets');
    expect(artifactCategory('proposal.md'), 'documents');
    expect(artifactCategory('slides.pptx'), 'slides');
    expect(artifactCategory('result.HTML'), 'pages');
    expect(artifactCategory('图像.JPG'), 'images');
    expect(artifactCategory('src/app.rs'), 'code');
  });
  testWidgets(
    'cached artifacts remain usable during a slow scan and show real turn diffs',
    (tester) async {
      final api = GalleryApi();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 470,
              child: ArtifactsView(api: api, session: 's'),
            ),
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));
      expect(api.scans, [false, true]);
      expect(
        find.byKey(const ValueKey('artifact-file-report.md')),
        findsOneWidget,
      );
      await tester.tap(
        find.byKey(const ValueKey('artifact-section-delivered')),
      );
      await tester.pump();
      expect(
        find.byKey(const ValueKey('artifact-file-figure.png')),
        findsNothing,
      );
      await tester.tap(find.byKey(const ValueKey('artifact-section-changes')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));
      expect(find.text('old line'), findsOneWidget);
      expect(find.text('new line'), findsOneWidget);
      expect(find.text('+1'), findsOneWidget);
      expect(find.text('−1'), findsOneWidget);
      api.scan.complete({'entries': []});
      await tester.pump();
      expect(find.text('new line'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      await api.close();
    },
  );
  testWidgets(
    'permission control exposes the product label instead of a preset identifier',
    (tester) async {
      final api = ReviewApi(), c = PermissionTestController(ReviewApi());
      c.projectionWindow.apply('permissions', {
        'currentValue': 'danger-full-access',
        'options': [
          {
            'value': 'danger-full-access',
            'name': 'permission.preset.danger-full-access',
          },
        ],
      }, 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(child: PermissionControl(controller: c)),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('完全访问'), findsOneWidget);
      expect(find.textContaining('permission.preset'), findsNothing);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await c.api.close();
      await api.close();
    },
  );
  testWidgets(
    'folder names toggle in both directions with an intermediate collapse frame',
    (tester) async {
      await mountRepair(tester);
      final children = find.byKey(const ValueKey('workspace-children-a'));
      final before = tester.getSize(children).height;
      expect(before, greaterThan(0));
      await tester.tap(find.byKey(const ValueKey('workspace-a')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 60));
      final during = tester.getSize(children).height;
      expect(during, greaterThan(0));
      expect(during, lessThan(before));
      await tester.pumpAndSettle();
      expect(tester.getSize(children).height, 0);
      await tester.tap(find.byKey(const ValueKey('workspace-a')));
      await tester.pumpAndSettle();
      expect(tester.getSize(children).height, greaterThan(0));
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets('large folders keep their offscreen session rows virtualized', (
    tester,
  ) async {
    final controller = await mountRepair(tester);
    controller.sessions = List.generate(
      2000,
      (i) => SessionSummary.fromJson({
        'sessionId': 'large-$i',
        'cwd': r'E:\a',
        'displayTitle': '会话 $i',
      }),
    );
    controller.workspaces.first['sessionIds'] = controller.sessions
        .map((s) => s.id)
        .toList();
    controller.emit();
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('session-large-0')), findsOneWidget);
    expect(find.byKey(const ValueKey('session-large-1999')), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
