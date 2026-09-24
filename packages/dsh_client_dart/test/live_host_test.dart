import 'dart:async';
import 'dart:io';
import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  final address = Platform.environment['DSH_TEST_HOST'];
  final cwd = Platform.environment['DSH_TEST_CWD'];
  group(
    'isolated Rust Host',
    () {
      late DshClient client;
      late EventChannel channel;
      late StreamSubscription<HostFrame> subscription;
      late List<HostFrame> frames;
      late String session;
      Future<void> until(bool Function() check) async {
        final deadline = DateTime.now().add(const Duration(seconds: 35));
        while (!check()) {
          if (DateTime.now().isAfter(deadline)) {
            throw TimeoutException(
              'Expected event not received: ${frames.map((f) => f.type).toSet().toList()}',
            );
          }
          await Future<void>.delayed(const Duration(milliseconds: 30));
        }
      }

      setUp(() async {
        client = DshClient(address!);
        expect((await client.describe()).home, contains('host-fixture'));
        session =
            (await client.call('session.create', {
                  'cwd': cwd!,
                  'agentPreset': 'standard',
                }, true))['sessionId']
                as String;
        frames = [];
        channel = client.events('mux');
        subscription = channel.frames.listen(frames.add);
        final ready = channel.states.firstWhere((v) => v);
        channel.start();
        await ready;
      });
      tearDown(() async {
        await subscription.cancel();
        await client.close();
      });
      test('create, model selection, streaming and durable history', () async {
        final models = await client.models(session);
        final model = models.choices.firstWhere(
          (m) => m.provider == 'desktop-fixture',
        );
        await client.selectModel(session, model);
        await client.prompt(
          session,
          'desktop-hello',
          requestId: newRequestId(),
        );
        await until(
          () => frames.any(
            (f) =>
                f.sessionId == session &&
                f.type == 'session/event' &&
                object(f.payload['event'])['type'] == 'turn/end',
          ),
        );
        expect(
          frames.any(
            (f) => object(f.payload['event'])['type'] == 'assistant/chunk',
          ),
          isTrue,
        );
        final history = await client.history(session);
        expect(
          projectTranscript(
            history.events,
          ).any((m) => m.text == '桌面客户端连接验证成功。'),
          isTrue,
        );
        final another = DshClient(address!);
        expect((await another.sessions()).any((s) => s.id == session), isTrue);
        expect(
          projectTranscript(
            (await another.history(session)).events,
          ).any((m) => m.text == 'desktop-hello'),
          isTrue,
        );
        await another.close();
      });
      test(
        'question batch and pending replay retain original correlation',
        () async {
          await client.prompt(
            session,
            'desktop-question',
            requestId: newRequestId(),
          );
          await until(
            () => frames.any(
              (f) => f.sessionId == session && f.type == 'question/requested',
            ),
          );
          final first = frames.lastWhere((f) => f.type == 'question/requested');
          final recovery = client.events('mux');
          final recovered = recovery.frames.firstWhere(
            (f) => f.type == 'question/requested' && f.sessionId == session,
          );
          recovery.start();
          final replay = await recovered.timeout(const Duration(seconds: 5));
          expect(replay.rpcId, first.rpcId);
          expect(
            await client.respond(replay, {
              'sessionId': session,
              'answer': {
                'answers': [
                  {
                    'id': 'choice',
                    'selected': ['方案一'],
                  },
                  {'id': 'detail', 'selected': <String>[], 'custom': '客户端验证'},
                ],
              },
            }),
            isTrue,
          );
          await until(
            () => frames.any(
              (f) => f.sessionId == session && f.type == 'question/resolved',
            ),
          );
          await until(
            () => frames.any(
              (f) =>
                  f.sessionId == session &&
                  f.type == 'session/event' &&
                  object(f.payload['event'])['type'] == 'turn/end',
            ),
          );
        },
      );
      test('approval explicitly resumes only the matching waiter', () async {
        await client.prompt(
          session,
          'desktop-approval',
          requestId: newRequestId(),
        );
        await until(
          () => frames.any(
            (f) => f.sessionId == session && f.type == 'approval/requested',
          ),
        );
        final approval = frames.lastWhere(
          (f) => f.type == 'approval/requested',
        );
        expect(
          await client.respond(approval, {
            'sessionId': session,
            'approvalId': approval.payload['approvalId'],
            'outcome': 'allowed-once',
          }),
          isTrue,
        );
        await until(
          () => frames.any(
            (f) => f.sessionId == session && f.type == 'approval/resolved',
          ),
        );
        await until(
          () => frames.any(
            (f) =>
                f.sessionId == session &&
                f.type == 'session/event' &&
                object(f.payload['event'])['type'] == 'turn/end',
          ),
        );
      });
      test('cancel stops an active streamed turn', () async {
        await client.prompt(session, 'desktop-slow', requestId: newRequestId());
        await until(
          () => frames.any(
            (f) =>
                f.sessionId == session &&
                object(f.payload['event'])['type'] == 'assistant/chunk',
          ),
        );
        await client.cancel(session);
        await until(
          () => frames.any(
            (f) =>
                f.sessionId == session &&
                object(f.payload['event'])['type'] == 'turn/end',
          ),
        );
        expect(
          (await client.sessions()).firstWhere((s) => s.id == session).running,
          isFalse,
        );
      });
    },
    timeout: const Timeout(Duration(seconds: 50)),
    skip: address == null || cwd == null
        ? 'Set DSH_TEST_HOST and DSH_TEST_CWD to an isolated fixture Host'
        : false,
  );
}
