import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/desktop_diagnostics.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:pdfrx/pdfrx.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class SoakPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

/// Deterministic files are delivered through the same scoped Host API that the
/// product uses. This fixture never connects to an actual Host or writes drafts.
class ResourceFixtureClient extends DshClient {
  ResourceFixtureClient(this.image, this.pdf50, this.pdf200)
    : super('http://127.0.0.1:1');
  final Uint8List image, pdf50, pdf200;
  final seenSessions = <String>{};
  int terminalReads = 0;
  int closeActions = 0;

  void authorize(String? session, RequestScope? scope) {
    if (scope?.cancelled == true) throw DshException('cancelled', 'Cancelled');
    if (session == null || !session.startsWith('resource-fixture-')) {
      throw StateError('Fixture session authorization failed');
    }
    seenSessions.add(session);
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    final uri = Uri.parse(path);
    authorize(
      (body?['sessionId'] ?? uri.queryParameters['sessionId']) as String?,
      scope,
    );
    switch (uri.path.split('/').last) {
      case 'list':
        return {'path': '', 'entries': <Json>[]};
      case 'source':
        return {
          'text': List.generate(
            3000,
            (index) => 'Source line $index: fixed fixture text.',
          ).join('\n'),
        };
      case 'git-status':
        return {'branch': 'fixture', 'entries': <Json>[]};
      case 'terminal-list':
        return {
          'entries': [
            {'id': 'fixture-terminal', 'name': 'Fixture terminal'},
          ],
        };
      case 'terminal-read':
        terminalReads++;
        return {
          'totalLines': 3000,
          'text': List.generate(3000, (index) => 'output $index').join('\n'),
        };
      case 'terminal-action':
        if (body?['action'] == 'close') closeActions++;
        return {};
      default:
        throw StateError('Unexpected fixture request: ${uri.path}');
    }
  }

  @override
  Future<Uint8List> bytes(
    String path, {
    Json? body,
    RequestScope? scope,
    int maxBytes = 16 * 1024 * 1024,
    bool mutation = false,
  }) async {
    final uri = Uri.parse(path);
    authorize(uri.queryParameters['sessionId'], scope);
    final name = uri.queryParameters['path']!;
    final content = name.endsWith('50.pdf')
        ? pdf50
        : name.endsWith('200.pdf')
        ? pdf200
        : image;
    if (content.length > maxBytes) {
      throw StateError('Fixture exceeds the Host response budget');
    }
    // Distinct downloads deliberately do not share a Uint8List identity.
    return Uint8List.fromList(content);
  }
}

class ResourceFixtureController extends DesktopController {
  ResourceFixtureController(this.fixture) : super(SoakPreferences()) {
    selectedId = 'resource-fixture-0';
    sessions = [
      for (var index = 0; index <= 20; index++)
        SessionSummary.fromJson({
          'sessionId': 'resource-fixture-$index',
          'cwd': '/fixture',
        }),
    ];
  }
  final ResourceFixtureClient fixture;
  @override
  DshClient get client => fixture;
}

class ResourceFixtureWorkbench extends StatefulWidget {
  const ResourceFixtureWorkbench({super.key, required this.controller});
  final ResourceFixtureController controller;
  @override
  State<ResourceFixtureWorkbench> createState() =>
      ResourceFixtureWorkbenchState();
}

class ResourceFixtureWorkbenchState extends State<ResourceFixtureWorkbench> {
  String tab = 'files';
  FileOpenRequest? request;
  int generation = 0;
  void open(String value, {String? path}) => setState(() {
    tab = value;
    request = path == null ? null : FileOpenRequest(path);
    generation++;
  });

  @override
  Widget build(BuildContext context) => ShadApp(
    home: Scaffold(
      body: WorkbenchPanel(
        key: ValueKey(widget.controller.selectedId),
        controller: widget.controller,
        onClose: () {},
        initialTab: tab,
        openRequest: generation,
        fileRequest: request,
        onFileRequestHandled: (handled) {
          if (identical(request, handled)) request = null;
        },
      ),
    ),
  );
}

Future<Uint8List> create4kImage() async {
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder);
  for (var row = 0; row < 24; row++) {
    for (var column = 0; column < 40; column++) {
      canvas.drawRect(
        Rect.fromLTWH(column * 96.0, row * 90.0, 96, 90),
        Paint()
          ..color = Color.fromARGB(
            255,
            (row * 11) % 256,
            (column * 7) % 256,
            ((row + column) * 13) % 256,
          ),
      );
    }
  }
  final picture = recorder.endRecording();
  try {
    final image = await picture.toImage(3840, 2160);
    try {
      final data = await image.toByteData(format: ui.ImageByteFormat.png);
      return data!.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes);
    } finally {
      image.dispose();
    }
  } finally {
    picture.dispose();
  }
}

/// A valid, unencrypted PDF with native text and vector graphics on every page.
/// This is a document fixture, not an Office conversion or raster screenshot.
Uint8List createPdf(int pages) {
  final objects = <String>[
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Count $pages /Kids [${List.generate(pages, (i) => '${4 + i * 2} 0 R').join(' ')}] >>',
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
  ];
  for (var index = 0; index < pages; index++) {
    final content =
        'BT /F1 18 Tf 36 800 Td (Resource fixture page ${index + 1}) Tj ET\n'
        '0.15 0.3 0.6 rg 36 600 480 120 re f\n';
    objects.add(
      '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] '
      '/Resources << /Font << /F1 3 0 R >> >> /Contents ${5 + index * 2} 0 R >>',
    );
    objects.add(
      '<< /Length ${ascii.encode(content).length} >>\nstream\n${content}endstream',
    );
  }
  final document = StringBuffer('%PDF-1.4\n');
  final offsets = <int>[0];
  for (var index = 0; index < objects.length; index++) {
    offsets.add(document.length);
    document.write('${index + 1} 0 obj\n${objects[index]}\nendobj\n');
  }
  final xref = document.length;
  document.write('xref\n0 ${objects.length + 1}\n0000000000 65535 f \n');
  for (final offset in offsets.skip(1)) {
    document.write('${offset.toString().padLeft(10, '0')} 00000 n \n');
  }
  document.write(
    'trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n$xref\n%%EOF\n',
  );
  return Uint8List.fromList(ascii.encode(document.toString()));
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  final output =
      Platform.environment['DSH_PERF_OUTPUT'] ??
      const String.fromEnvironment('DSH_PERF_OUTPUT');
  final exitWhenDone = Platform.environment['DSH_PERF_EXIT'] == '1';
  final idleSeconds =
      int.tryParse(Platform.environment['DSH_SOAK_IDLE_SECONDS'] ?? '') ?? 60;
  final includePdf = Platform.environment['DSH_SOAK_PDF'] != '0';
  final samples = <Map<String, Object>>[];
  final report = <String, Object?>{
    'schemaVersion': 1,
    'fixture': 'experience-resource-soak-v1',
    'processId': pid,
    'buildMode': kReleaseMode
        ? 'release'
        : kProfileMode
        ? 'profile'
        : 'debug',
    'dartVersion': Platform.version,
    'operatingSystem': Platform.operatingSystemVersion,
    'processors': Platform.numberOfProcessors,
    'startedAt': DateTime.now().toUtc().toIso8601String(),
    'state': 'running',
    'coverage': {
      'generatedImageSize': [3840, 2160],
      'dailyImages': 10,
      'stressImagesPerRound': 20,
      'pdfPages': includePdf ? [50, 200] : <int>[],
      'pdfVisits': 'first, middle and final pages',
      'terminalLines': 3000,
      'rounds': 20,
      'warmupRounds': 5,
      'idleSecondsAfterClose': idleSeconds,
      'privateBytesMeasured': false,
      'realHost': false,
      'computerUseFiveMinutes': false,
      'installedPackage': false,
    },
    'samples': samples,
  };

  Future<void> save() async {
    if (output.isEmpty) return;
    final file = File(output);
    await file.parent.create(recursive: true);
    await file.writeAsString(
      const JsonEncoder.withIndent('  ').convert(report),
      flush: true,
    );
  }

  tearDownAll(() async {
    report['state'] = binding.failureMethodsDetails.isEmpty
        ? 'passed'
        : 'failed';
    report['failures'] = binding.failureMethodsDetails
        .map((failure) => failure.details)
        .toList();
    report['finishedAt'] = DateTime.now().toUtc().toIso8601String();
    await save();
    if (exitWhenDone) {
      Timer(
        const Duration(seconds: 1),
        () => exit(binding.failureMethodsDetails.isEmpty ? 0 : 1),
      );
    }
  });

  testWidgets('twenty native preview, terminal and session lifecycle rounds', (
    tester,
  ) async {
    expect(
      output,
      isNotEmpty,
      reason: 'Set DSH_PERF_OUTPUT to the measurement report path.',
    );
    expect(idleSeconds, inInclusiveRange(0, 300));
    await save();
    await tester.binding.setSurfaceSize(const Size(1280, 800));
    PaintingBinding.instance.imageCache.maximumSize = 128;
    PaintingBinding.instance.imageCache.maximumSizeBytes = 48 * 1024 * 1024;
    final api = ResourceFixtureClient(
      await create4kImage(),
      createPdf(50),
      createPdf(200),
    );
    final controller = ResourceFixtureController(api);
    final monitor = DesktopDiagnostics(controller, File(output));
    report['fixtureBytes'] = {
      'image': api.image.length,
      'pdf50': api.pdf50.length,
      'pdf200': api.pdf200.length,
    };
    addTearDown(() async {
      await tester.pumpWidget(const SizedBox());
      monitor.close();
      controller.dispose();
      await api.close();
    });

    Future<void> until(bool Function() ready) async {
      final deadline = DateTime.now().add(const Duration(seconds: 30));
      while (!ready() && DateTime.now().isBefore(deadline)) {
        await tester.pump(const Duration(milliseconds: 50));
      }
      expect(
        ready(),
        isTrue,
        reason: 'The native fixture did not become ready.',
      );
      expect(tester.takeException(), isNull);
    }

    Future<void> record(int round, String phase) async {
      samples.add({
        'round': round,
        'phase': phase,
        ...monitor.snapshot(),
        'currentRssBytes': ProcessInfo.currentRss,
        'maxRssBytes': ProcessInfo.maxRss,
      });
      await save();
    }

    Future<void> loadImages(
      ResourceFixtureWorkbenchState panel,
      int count,
    ) async {
      for (var image = 0; image < count; image++) {
        panel.open('files', path: 'image-$image.png');
        await tester.pump();
        await until(
          () => find
              .descendant(
                of: find.byType(NativeFileViewer),
                matching: find.byType(Image),
              )
              .evaluate()
              .isNotEmpty,
        );
        final imageWidget = tester.widget<Image>(
          find
              .descendant(
                of: find.byType(NativeFileViewer),
                matching: find.byType(Image),
              )
              .first,
        );
        final stream = imageWidget.image.resolve(ImageConfiguration.empty);
        final ready = Completer<void>();
        final listener = ImageStreamListener(
          (_, _) {
            if (!ready.isCompleted) ready.complete();
          },
          onError: (Object error, StackTrace? stack) {
            if (!ready.isCompleted) ready.completeError(error, stack);
          },
        );
        stream.addListener(listener);
        try {
          await ready.future.timeout(const Duration(seconds: 30));
          await tester.pump(const Duration(milliseconds: 40));
        } finally {
          stream.removeListener(listener);
        }
      }
    }

    Future<void> loadPdf(ResourceFixtureWorkbenchState panel, int pages) async {
      if (!includePdf) return;
      panel.open('files', path: 'document-$pages.pdf');
      await tester.pump();
      await until(
        () =>
            find.byType(PdfViewer).evaluate().isNotEmpty &&
            tester
                .widget<PdfViewer>(find.byType(PdfViewer))
                .controller!
                .isReady,
      );
      final pdf = tester.widget<PdfViewer>(find.byType(PdfViewer)).controller!;
      await tester.pump(const Duration(milliseconds: 100));
      expect(pdf.pageCount, pages);
      for (final page in [1, pages ~/ 2, pages]) {
        await pdf.goToPage(pageNumber: page, duration: Duration.zero);
        await tester.pump(const Duration(milliseconds: 150));
      }
    }

    await record(0, 'empty');
    for (var round = 0; round <= 20; round++) {
      controller.selectedId = 'resource-fixture-$round';
      final key = GlobalKey<ResourceFixtureWorkbenchState>();
      await tester.pumpWidget(
        ResourceFixtureWorkbench(key: key, controller: controller),
      );
      await tester.pump();
      await loadImages(key.currentState!, round == 0 ? 10 : 20);
      await record(round, 'images');
      await loadPdf(key.currentState!, round == 0 ? 50 : 200);
      await record(round, includePdf ? 'pdf' : 'pdf-skipped');
      key.currentState!.open('terminal');
      await tester.pump();
      await until(() {
        final lines =
            (monitor.snapshot()['owners'] as Map)['terminalBufferLines'];
        return lines is int && lines >= 2900;
      });
      await record(round, 'terminal');
      key.currentState!.open('git');
      await tester.pump();
      final reads = api.terminalReads;
      await tester.pump(const Duration(milliseconds: 600));
      expect(
        api.terminalReads,
        reads,
        reason: 'Hidden terminal polling must stop.',
      );
      await record(round, 'hidden-terminal');
      await tester.pumpWidget(const SizedBox());
      await tester.pump(const Duration(milliseconds: 100));
      expect(monitor.snapshot()['owners'], isEmpty);
      expect(
        api.closeActions,
        0,
        reason: 'Disposing a terminal view must not stop its job.',
      );
      if (round > 0 && idleSeconds > 0) {
        await tester.pump(Duration(seconds: idleSeconds));
      }
      await record(round, 'closed');
    }
    expect(api.seenSessions.length, 21);
    final closed = samples
        .where((sample) => sample['phase'] == 'closed')
        .toList();
    final baseline =
        closed.firstWhere((sample) => sample['round'] == 5)['currentRssBytes']
            as int;
    final finalRss = closed.last['currentRssBytes'] as int;
    report['rssPlateau'] = {
      'round5Bytes': baseline,
      'round20Bytes': finalRss,
      'allowedIncreaseBytes': math.max(
        (baseline * .1).round(),
        32 * 1024 * 1024,
      ),
      'withinTarget':
          finalRss <=
          baseline + math.max((baseline * .1).round(), 32 * 1024 * 1024),
      'release60SecondIdleEligible': kReleaseMode && idleSeconds >= 60,
      'metric': 'current RSS; Windows Private Bytes require separate process sampling',
    };
    expect(tester.takeException(), isNull);
    await save();
  }, timeout: const Timeout(Duration(minutes: 90)));
}
