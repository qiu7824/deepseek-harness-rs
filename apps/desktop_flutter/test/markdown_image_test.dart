import 'package:dsh_desktop/design/rich_content.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  test(
    'Markdown local image paths support Windows drives and encoded names',
    () {
      expect(
        markdownImageFilePath(Uri.parse('C:/%E5%9B%BE%20%E7%89%87/a%23b.png')),
        r'c:\图 片\a#b.png',
      );
      expect(markdownImageFilePath(Uri.parse(r'D:\图片\a.png')), r'd:\图片\a.png');
      expect(
        markdownImageFilePath(Uri.parse('file:///C:/images/a%2520b.png')),
        r'C:\images\a%20b.png',
      );
      expect(
        markdownImageFilePath(Uri.parse('file://localhost/C:/images/a.png')),
        r'C:\images\a.png',
      );
      expect(
        markdownImageFilePath(Uri.parse('file://server/share/a%20b.png')),
        r'\\server\share\a b.png',
      );
      expect(
        markdownImageFilePath(Uri.parse('images/a%20b.png'), windows: false),
        'images/a b.png',
      );
      expect(
        markdownImageFilePath(
          Uri.parse('file:///tmp/%E5%9B%BE%20%E7%89%87.png'),
          windows: false,
        ),
        '/tmp/图 片.png',
      );
    },
  );

  test('unsupported or malformed file URLs produce an image fallback', () {
    for (final value in [
      'file:///C:/image.png?token=1',
      'file:///C:/image.png#fragment',
      'https://example.com/image.png',
      'c:relative.png',
      'javascript:alert(1)',
    ]) {
      expect(markdownImageFilePath(Uri.parse(value)), isNull, reason: value);
    }
  });

  testWidgets(
    'Markdown image tap opens a zoomable preview and retains its context menu',
    (tester) async {
      final uri = Uri.parse(
        'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a7xQAAAAASUVORK5CYII=',
      );
      var secondaryTaps = 0;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox.square(
              dimension: 120,
              child: MarkdownImage(
                uri: uri,
                label: '本地图片',
                onSecondaryTap: (_) => secondaryTaps++,
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final gesture = await tester.startGesture(
        tester.getCenter(find.byType(MarkdownImage)),
        kind: PointerDeviceKind.mouse,
        buttons: kSecondaryMouseButton,
      );
      await gesture.up();
      await tester.pumpAndSettle();
      expect(secondaryTaps, 1);
      await tester.tap(find.byType(MarkdownImage));
      await tester.pumpAndSettle();
      expect(find.byType(InteractiveViewer), findsOneWidget);
      expect(find.text('本地图片'), findsOneWidget);
      await tester.tap(find.byTooltip('关闭图片预览'));
      await tester.pumpAndSettle();
      expect(find.byType(InteractiveViewer), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('malformed file image does not abort following Markdown blocks', (
    tester,
  ) async {
    await tester.pumpWidget(
      const ShadApp(
        home: Scaffold(
          body: DshMarkdown(
            data: '![损坏图片](file:///C:/image.png?token=1)\n\n后续正文',
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('损坏图片（图片无法显示）'), findsOneWidget);
    expect(find.text('后续正文'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
