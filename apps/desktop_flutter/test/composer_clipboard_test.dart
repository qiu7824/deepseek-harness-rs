import 'dart:async';
import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/composer_clipboard.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class _Preferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class _Controller extends DesktopController {
  _Controller() : super(_Preferences()) {
    selectedId = 'first';
  }
  List<Json>? sent;
  String? sentText;
  @override
  bool get connected => true;
  @override
  Future<String?> sendParts(
    String text,
    List<Json> attachments, {
    String mode = 'queue',
  }) async {
    sent = attachments;
    sentText = text;
    return selectedId;
  }
}

class _DelayedFile extends XFile {
  _DelayedFile(this.bytes) : super('waiting.png');
  final Completer<Uint8List> bytes;
  @override
  Future<int> length() async => 70;
  @override
  Future<Uint8List> readAsBytes() => bytes.future;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final png = base64Decode(
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==',
  );
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  tearDown(() {
    messenger.setMockMethodCallHandler(ComposerClipboard.channel, null);
    messenger.setMockMethodCallHandler(SystemChannels.platform, null);
  });

  Future<_Controller> mount(WidgetTester tester) async {
    final controller = _Controller();
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: controller)),
      ),
    );
    await tester.pumpAndSettle();
    return controller;
  }

  Future<void> dispose(WidgetTester tester, _Controller controller) async {
    await tester.pumpWidget(const SizedBox());
    controller.dispose();
    expect(tester.takeException(), isNull);
  }

  Future<void> pasteKey(WidgetTester tester) async {
    await tester.tap(find.byKey(const Key('prompt-input')));
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
  }

  testWidgets('Ctrl+V imports a PNG thumbnail and sends its original bytes', (
    tester,
  ) async {
    messenger.setMockMethodCallHandler(
      ComposerClipboard.channel,
      (_) async => {'png': png},
    );
    final c = await mount(tester);
    await tester.enterText(
      find.byKey(const Key('prompt-input')),
      'describe this',
    );
    await pasteKey(tester);
    expect(find.byType(InputChip), findsOneWidget);
    expect(find.byType(Image), findsOneWidget);
    await tester.tap(find.byKey(const Key('send-message')));
    await tester.pumpAndSettle();
    expect(c.sentText, 'describe this');
    expect(c.sent, hasLength(1));
    expect(c.sent!.single['type'], 'image');
    expect(c.sent!.single['mediaType'], 'image/png');
    expect(base64Decode(c.sent!.single['data'] as String), png);
    expect(find.byType(InputChip), findsNothing);
    await dispose(tester, c);
  });

  testWidgets(
    'ordinary text paste replaces the selection and saves the draft',
    (tester) async {
      messenger.setMockMethodCallHandler(
        ComposerClipboard.channel,
        (_) async => {},
      );
      messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
        if (call.method == 'Clipboard.getData') return {'text': '粘贴'};
        if (call.method == 'Clipboard.hasStrings') return {'value': true};
        return null;
      });
      final c = await mount(tester);
      await tester.enterText(
        find.byKey(const Key('prompt-input')),
        'before after',
      );
      final input = tester
          .widget<TextField>(find.byKey(const Key('prompt-input')))
          .controller!;
      input.selection = const TextSelection(baseOffset: 7, extentOffset: 12);
      // Invoke the same overridable action used by keyboard shortcuts without
      // moving the selection through a pointer tap.
      Actions.invoke(
        tester.element(find.byType(EditableText)),
        const PasteTextIntent(SelectionChangedCause.keyboard),
      );
      await tester.pumpAndSettle();
      expect(input.text, 'before 粘贴');
      expect(c.preferences.drafts['first'], input.text);
      expect(find.byType(InputChip), findsNothing);
      await dispose(tester, c);
    },
  );

  testWidgets(
    'context menu offers paste when clipboard contains only an image',
    (tester) async {
      messenger.setMockMethodCallHandler(
        ComposerClipboard.channel,
        (_) async => {'png': png},
      );
      messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
        if (call.method == 'Clipboard.hasStrings') return {'value': false};
        return null;
      });
      final c = await mount(tester);
      await tester.tap(
        find.byKey(const Key('prompt-input')),
        buttons: kSecondaryMouseButton,
      );
      await tester.pumpAndSettle();
      final toolbar = tester.widget<AdaptiveTextSelectionToolbar>(
        find.byType(AdaptiveTextSelectionToolbar),
      );
      final paste = toolbar.buttonItems!.singleWhere(
        (item) => item.type == ContextMenuButtonType.paste,
      );
      paste.onPressed!();
      await tester.pumpAndSettle();
      expect(find.byType(InputChip), findsOneWidget);
      await dispose(tester, c);
    },
  );

  testWidgets('a late clipboard result cannot enter a newly selected task', (
    tester,
  ) async {
    final response = Completer<Map<String, dynamic>>();
    messenger.setMockMethodCallHandler(
      ComposerClipboard.channel,
      (_) => response.future,
    );
    final c = await mount(tester);
    final dynamic state = tester.state(find.byType(Conversation));
    final Future<void> importing = state.paste();
    await tester.pump();
    c.selectedId = 'second';
    c.emit();
    await state.send();
    expect(state.importingAttachments, 0);
    response.complete({'png': png});
    await importing;
    await tester.pumpAndSettle();
    expect(find.byType(InputChip), findsNothing);
    expect(c.error, isNull);
    await dispose(tester, c);
  });

  testWidgets('a late file read cannot enter a newly selected task', (
    tester,
  ) async {
    final c = await mount(tester);
    final bytes = Completer<Uint8List>();
    final dynamic state = tester.state(find.byType(Conversation));
    final Future<void> importing = state.addFiles([_DelayedFile(bytes)]);
    await tester.pump();
    c.selectedId = 'second';
    c.emit();
    bytes.complete(png);
    await importing;
    await tester.pumpAndSettle();
    expect(find.byType(InputChip), findsNothing);
    await dispose(tester, c);
  });

  testWidgets(
    'an oversized attachment batch is rejected without a partial import',
    (tester) async {
      final c = await mount(tester);
      final dynamic state = tester.state(find.byType(Conversation));
      await state.addFiles(
        List.generate(9, (i) => XFile.fromData(png, name: '$i.png')),
      );
      await tester.pumpAndSettle();
      expect(find.byType(InputChip), findsNothing);
      expect(c.error, contains('最多添加 8'));
      await dispose(tester, c);
    },
  );

  testWidgets(
    'sending waits until the asynchronous attachment import finishes',
    (tester) async {
      final c = await mount(tester);
      await tester.enterText(
        find.byKey(const Key('prompt-input')),
        'with image',
      );
      final bytes = Completer<Uint8List>();
      final dynamic state = tester.state(find.byType(Conversation));
      final Future<void> importing = state.addFiles([_DelayedFile(bytes)]);
      await tester.pump();
      await state.send();
      expect(c.sent, isNull);
      bytes.complete(png);
      await importing;
      await tester.pumpAndSettle();
      await state.send();
      expect(c.sent, hasLength(1));
      await dispose(tester, c);
    },
  );

  testWidgets('concurrent imports cannot exceed the attachment count limit', (
    tester,
  ) async {
    final c = await mount(tester);
    final dynamic state = tester.state(find.byType(Conversation));
    await state.addFiles(
      List.generate(7, (i) => XFile.fromData(png, path: '$i.png')),
    );
    final first = Completer<Uint8List>(), second = Completer<Uint8List>();
    final Future<void> a = state.addFiles([_DelayedFile(first)]);
    final Future<void> b = state.addFiles([_DelayedFile(second)]);
    await tester.pump();
    first.complete(png);
    await a;
    second.complete(png);
    await b;
    await tester.pumpAndSettle();
    expect(state.attachments, hasLength(8));
    expect(c.error, contains('最多添加 8'));
    await dispose(tester, c);
  });

  test(
    'clipboard file paths take precedence over accompanying image data',
    () async {
      messenger.setMockMethodCallHandler(
        ComposerClipboard.channel,
        (_) async => {
          'files': ['C:\\图片\\照片.png'],
          'png': png,
        },
      );
      final files = await ComposerClipboard().readFiles();
      expect(files!.single.path, 'C:\\图片\\照片.png');
    },
  );

  test('unsupported platforms fall back to Flutter text clipboard', () async {
    messenger.setMockMethodCallHandler(
      ComposerClipboard.channel,
      (_) async => throw MissingPluginException(),
    );
    expect(await ComposerClipboard().readFiles(), isNull);
  });

  test('clipboard error is translated for the composer', () async {
    messenger.setMockMethodCallHandler(
      ComposerClipboard.channel,
      (_) async => throw PlatformException(code: 'clipboard-too-large'),
    );
    expect(
      ComposerClipboard().readFiles(),
      throwsA(
        isA<StateError>().having(
          (e) => e.message,
          'message',
          contains('16 MiB'),
        ),
      ),
    );
  });
}
