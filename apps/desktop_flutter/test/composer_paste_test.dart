import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/rich_content.dart';
import 'package:dsh_desktop/features/conversation/attachment_view.dart';
import 'package:dsh_desktop/src/composer_attachments.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class PastePreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class PasteController extends DesktopController {
  PasteController() : super(PastePreferences()) {
    selectedId = 's';
  }
  final sent = <List<Json>>[];
  @override
  bool get connected => true;
  @override
  Future<String?> sendParts(
    String text,
    List<Json> attachments, {
    String mode = 'queue',
  }) async {
    sent.add(attachments);
    return 's';
  }
}

final onePixelPng = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
);

Future<void> pressPaste(WidgetTester tester) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
}

void mockClipboard(WidgetTester tester, {Object? native, String? text}) {
  final messenger = tester.binding.defaultBinaryMessenger;
  messenger.setMockMethodCallHandler(
    NativeClipboard.channel,
    (call) async => call.method == 'read' ? native : null,
  );
  messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
    if (call.method == 'Clipboard.hasStrings') {
      return {'value': text != null};
    }
    if (call.method == 'Clipboard.getData') {
      return text == null ? null : {'text': text};
    }
    return null;
  });
}

/// Lets real file reads started inside the fake-async test zone complete.
Future<void> settleFileReads(WidgetTester tester) async {
  for (var i = 0; i < 20; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 5)),
    );
    await tester.pump();
  }
  await tester.pumpAndSettle();
}

Future<PasteController> pumpComposer(WidgetTester tester) async {
  final c = PasteController();
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(body: Conversation(controller: c)),
    ),
  );
  await tester.pumpAndSettle();
  await tester.tap(find.byKey(const Key('prompt-input')));
  await tester.pump();
  return c;
}

void main() {
  group('attachment media types', () {
    test('raster signatures win over a mislabeled extension', () {
      expect(attachmentMediaType('photo.png', onePixelPng), 'image/png');
      expect(
        attachmentMediaType(
          'photo.png',
          Uint8List.fromList([0xFF, 0xD8, 0xFF, 0xE0, 0, 0]),
        ),
        'image/jpeg',
      );
      expect(
        attachmentMediaType('x.bin', Uint8List.fromList(utf8.encode('GIF89a'))),
        'image/gif',
      );
      expect(
        attachmentMediaType(
          'x',
          Uint8List.fromList([
            ...utf8.encode('RIFF'),
            0,
            0,
            0,
            0,
            ...utf8.encode('WEBP'),
          ]),
        ),
        'image/webp',
      );
    });

    test('generic files keep an extension type and are never image parts', () {
      final text = Uint8List.fromList(utf8.encode('hello'));
      expect(attachmentMediaType('notes.MD', text), 'text/markdown');
      expect(attachmentMediaType('report.pdf', text), 'application/pdf');
      expect(attachmentMediaType('fake.png', text), 'application/octet-stream');
      expect(attachmentMediaType('shot.bmp', text), 'image/bmp');
      expect(imageMediaTypes.contains('image/bmp'), isFalse);
      expect(imageMediaTypes.contains('image/svg+xml'), isFalse);
    });

    test('pasted image names are ASCII and time ordered', () {
      expect(
        pastedImageName(DateTime(2026, 9, 6, 7, 8, 9)),
        'pasted-image-20260906-070809.png',
      );
    });
  });

  group('clipboard PNG streams', () {
    test('trailing clipboard memory is cut at IEND', () {
      final padded = Uint8List.fromList([...onePixelPng, 0, 0, 0, 0, 0, 0]);
      expect(trimPngStream(padded), onePixelPng);
      expect(trimPngStream(onePixelPng), same(onePixelPng));
    });

    test('streams without a complete chunk chain are rejected', () {
      expect(trimPngStream(Uint8List.sublistView(onePixelPng, 0, 40)), isNull);
      expect(trimPngStream(Uint8List.fromList(utf8.encode('text'))), isNull);
    });

    testWidgets('opaque BGRA pixels encode as a decodable PNG', (tester) async {
      await tester.runAsync(() async {
        final pixels = Uint8List.fromList([
          0, 0, 255, 255, // red in BGRA
          255, 0, 0, 255, // blue
        ]);
        final png = await pngFromBgra(pixels, 2, 1);
        expect(sniffImageType(png), 'image/png');
        final codec = await ui.instantiateImageCodec(png);
        final frame = await codec.getNextFrame();
        expect(frame.image.width, 2);
        expect(frame.image.height, 1);
        frame.image.dispose();
        codec.dispose();
      });
      expect(() => pngFromBgra(Uint8List(4), 2, 1), throwsA(isA<StateError>()));
    });
  });

  group('composer paste', () {
    testWidgets('an image-only clipboard becomes a PNG attachment', (
      tester,
    ) async {
      mockClipboard(tester, native: {'files': <Object>[], 'png': onePixelPng});
      final c = await pumpComposer(tester);
      await pressPaste(tester);
      await tester.pumpAndSettle();
      expect(find.byType(PendingAttachmentTile), findsOneWidget);
      final input = tester.widget<TextField>(
        find.byKey(const Key('prompt-input')),
      );
      expect(input.controller!.text, isEmpty);

      await tester.tap(find.byKey(const Key('send-message')));
      await tester.pumpAndSettle();
      expect(c.sent, hasLength(1));
      final part = c.sent.single.single;
      expect(part['type'], 'image');
      expect(part['mediaType'], 'image/png');
      expect(part['name'], startsWith('pasted-image-'));
      expect(base64Decode(part['data'] as String), onePixelPng);
      expect(find.byType(PendingAttachmentTile), findsNothing);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    });

    testWidgets('text copied with a bitmap pastes as text only', (
      tester,
    ) async {
      mockClipboard(
        tester,
        native: {'files': <Object>[], 'png': onePixelPng},
        text: 'A1\tB1',
      );
      final c = await pumpComposer(tester);
      await pressPaste(tester);
      await tester.pumpAndSettle();
      expect(find.byType(PendingAttachmentTile), findsNothing);
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('prompt-input')))
            .controller!
            .text,
        'A1\tB1',
      );
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    });

    testWidgets('a client without the clipboard channel still pastes text', (
      tester,
    ) async {
      mockClipboard(tester, text: 'plain');
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        NativeClipboard.channel,
        (call) async => throw MissingPluginException(),
      );
      final c = await pumpComposer(tester);
      await pressPaste(tester);
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('prompt-input')))
            .controller!
            .text,
        'plain',
      );
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    });

    testWidgets('copied files attach with sniffed types; folders are refused', (
      tester,
    ) async {
      final dir = await tester.runAsync(
        () => Directory.systemTemp.createTemp('dsh-paste-'),
      );
      final image = File('${dir!.path}${Platform.pathSeparator}shot.jpg');
      final notes = File('${dir.path}${Platform.pathSeparator}notes.md');
      await tester.runAsync(() async {
        await image.writeAsBytes(onePixelPng);
        await notes.writeAsString('# notes');
      });
      mockClipboard(
        tester,
        native: {
          'files': [image.path, notes.path],
        },
      );
      final c = await pumpComposer(tester);
      await pressPaste(tester);
      await settleFileReads(tester);
      expect(find.byType(PendingAttachmentTile), findsNWidgets(2));
      await tester.tap(find.byKey(const Key('send-message')));
      await tester.pumpAndSettle();
      final parts = c.sent.single;
      expect(parts[0]['type'], 'image');
      expect(parts[0]['mediaType'], 'image/png');
      expect(parts[1]['type'], 'file');
      expect(parts[1]['mediaType'], 'text/markdown');

      mockClipboard(
        tester,
        native: {
          'files': [dir.path],
        },
      );
      await tester.tap(find.byKey(const Key('prompt-input')));
      await pressPaste(tester);
      await settleFileReads(tester);
      expect(find.byType(PendingAttachmentTile), findsNothing);
      expect(c.error, contains('不能添加文件夹'));
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.runAsync(() => dir.delete(recursive: true));
    });
  });

  group('markdown local images', () {
    test('windows drive, encoded and relative sources', () {
      String? resolve(String source, [String? base = r'D:\work\repo']) =>
          markdownImageFile(Uri.parse(source), base, windows: true);
      expect(resolve(r'C:\Users\me\shot.png'), r'C:\Users\me\shot.png');
      expect(resolve('C:/Users/me/shot.png'), r'C:\Users\me\shot.png');
      expect(resolve('images/图 表.png'), r'D:\work\repo\images\图 表.png');
      expect(resolve('./out/%E5%9B%BE.png'), r'D:\work\repo\out\图.png');
      expect(resolve(r'.\out\a.png'), r'D:\work\repo\out\a.png');
      expect(resolve('file:///C:/x/a.png'), r'C:\x\a.png');
      expect(resolve('images/a.png', null), isNull);
      expect(resolve('https://example.com/a.png'), isNull);
      expect(resolve('data:image/png;base64,AAAA'), isNull);
    });

    test('posix absolute and relative sources', () {
      String? resolve(String source) =>
          markdownImageFile(Uri.parse(source), '/home/me/repo', windows: false);
      expect(resolve('/tmp/a.png'), '/tmp/a.png');
      expect(resolve('docs/a%20b.png'), '/home/me/repo/docs/a b.png');
      expect(resolve('./a.png'), '/home/me/repo/a.png');
      expect(resolve('file:///tmp/a.png'), '/tmp/a.png');
      expect(resolve('C:/x.png'), isNull);
    });
  });

  testWidgets('a relative workspace image renders and opens enlarged', (
    tester,
  ) async {
    final dir = await tester.runAsync(
      () => Directory.systemTemp.createTemp('dsh-md-image-'),
    );
    await tester.runAsync(
      () =>
          File('${dir!.path}${Platform.pathSeparator}shot.png')
              .writeAsBytes(onePixelPng),
    );
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: DshMarkdown(
            data: '![截图](shot.png)',
            imageBaseDirectory: dir!.path,
          ),
        ),
      ),
    );
    await settleFileReads(tester);
    final image = find.byWidgetPredicate(
      (widget) => widget is Image && widget.image is ResizeImage,
    );
    expect(image, findsOneWidget);
    expect(tester.getSize(image).width, greaterThan(0));
    final provider =
        (tester.widget<Image>(image).image as ResizeImage).imageProvider;
    expect(provider, isA<FileImage>());
    expect(
      (provider as FileImage).file.path,
      '${dir.path}${Platform.pathSeparator}shot.png',
    );
    await tester.tap(image);
    await tester.pumpAndSettle();
    expect(find.byType(InteractiveViewer), findsOneWidget);
    expect(find.text('截图'), findsOneWidget);
    await tester.tap(find.byTooltip('关闭'));
    await tester.pumpAndSettle();
    expect(find.byType(InteractiveViewer), findsNothing);
    await tester.pumpWidget(const SizedBox());
    await tester.runAsync(() => dir.delete(recursive: true));
  });
}
