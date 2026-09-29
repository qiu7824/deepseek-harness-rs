import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

Json record([String kind = 'after']) => {
  'id': 'reminder-1',
  'kind': kind,
  'title': '检查项目',
  'prompt': '报告当前状态',
  'scheduledAt': '2030-01-01T09:00:00.000Z',
  if (kind == 'after') 'afterSeconds': 60,
  if (kind == 'every') 'everySeconds': 600,
  if (kind == 'daily' || kind == 'weekly') 'time': '09:00:00',
  if (['daily', 'weekly', 'cron'].contains(kind)) 'timeZone': 'Asia/Shanghai',
  if (kind == 'weekly') 'weekdays': [1, 3, 5],
  if (kind == 'cron') 'expression': '0 9 * * *',
};

void main() {
  late HttpServer server;
  late DshClient client;
  late ScheduleApi api;
  final requests = <Json>[];
  late Object? Function(Json) respond;
  Json? failure;
  setUp(() async {
    requests.clear();
    failure = null;
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    client = DshClient('http://127.0.0.1:${server.port}');
    api = ScheduleApi(client);
    respond = (_) => <String, dynamic>{};
    server.listen((request) async {
      final body = object(jsonDecode(await utf8.decoder.bind(request).join()));
      requests.add(body);
      expect(request.uri.path, '/api/${body['method']}');
      expect(body['type'], 'client-request');
      final value = respond(body);
      request.response.write(
        jsonEncode({
          'type': 'server-response',
          'rpcId': body['rpcId'],
          'result': failure == null
              ? {'ok': true, 'value': value}
              : {'ok': false, 'error': failure},
        }),
      );
      await request.response.close();
    });
  });
  tearDown(() async {
    await client.close();
    await server.close(force: true);
  });

  test(
    'catalog parses the actual top-level array without changing object RPCs',
    () async {
      respond = (request) => request['method'] == 'schedule.catalog'
          ? [
              for (final kind in [
                'after',
                'at',
                'every',
                'daily',
                'weekly',
                'cron',
              ])
                {...record(kind), 'sessionId': 's', 'status': 'active'},
            ]
          : {'still': 'an object'};
      final entries = await api.catalog();
      expect(entries.length, 6);
      expect(entries.map((entry) => entry.record.kind), [
        'after',
        'at',
        'every',
        'daily',
        'weekly',
        'cron',
      ]);
      expect(entries.first.record.toJson(), record());
      expect(await client.rpc('host.describe'), {'still': 'an object'});
    },
  );
  test('six create selectors keep wire casing and bind the chosen original session', () async {
    final timings = <Json>[
      {'after_seconds': 1},
      {
        'at': {
          'date': '2030-01-01',
          'time': '09:00',
          'time_zone': 'Asia/Shanghai',
        },
      },
      {'every_seconds': 60},
      {
        'daily': {'time': '09:00', 'time_zone': 'UTC'},
      },
      {
        'weekly': {
          'time': '09:00',
          'time_zone': 'Europe/Paris',
          'weekdays': [1, 5],
        },
      },
      {
        'cron': {'expression': '0 9 * * *', 'time_zone': 'America/New_York'},
      },
    ];
    respond = (_) => record();
    for (final timing in timings) {
      await api.create(
        sessionId: 'bound-session',
        title: 'title',
        prompt: 'prompt',
        timing: timing,
      );
      expect(requests.last['payload'], {
        'sessionId': 'bound-session',
        'title': 'title',
        'prompt': 'prompt',
        ...timing,
      });
    }
    expect(
      requests.every((request) => request['method'] == 'schedule.create'),
      isTrue,
    );
  });
  test(
    'CAS expected is complete, detached, and excludes catalog metadata',
    () async {
      final raw = {...record('weekly'), 'sessionId': 's', 'status': 'active'};
      final entry = ScheduleEntry.fromJson(raw);
      (raw['weekdays'] as List)[0] = 7;
      final exported = entry.record.toJson();
      (exported['weekdays'] as List).clear();
      respond = (_) => {
        'id': 'reminder-1',
        'updated': false,
        'code': 'schedule_conflict',
      };
      final result = await api.update(
        sessionId: 's',
        expected: entry.record,
        title: 'draft',
        change: {
          'kind': 'daily',
          'daily': {'time': '10:00', 'time_zone': 'UTC'},
        },
      );
      expect(result.code, 'schedule_conflict');
      expect(requests.single['payload'], {
        'sessionId': 's',
        'id': 'reminder-1',
        'expected': record('weekly'),
        'title': 'draft',
        'change': {
          'kind': 'daily',
          'daily': {'time': '10:00', 'time_zone': 'UTC'},
        },
      });
    },
  );
  test(
    'unchanged update is success and a mismatched record identity is rejected',
    () async {
      respond = (_) => {
        'id': 'reminder-1',
        'updated': false,
        'record': record(),
      };
      final result = await api.update(
        sessionId: 's',
        expected: ScheduleRecord.fromJson(record()),
      );
      expect(result.updated, false);
      expect(result.code, isNull);
      expect(result.record, isNotNull);
      respond = (_) => {'id': 'wrong', 'updated': true, 'record': record()};
      await expectLater(
        api.update(sessionId: 's', expected: ScheduleRecord.fromJson(record())),
        throwsA(isA<DshException>()),
      );
    },
  );
  test(
    'history keeps exclusive opaque cursor, retention and pruning evidence',
    () async {
      respond = (_) => {
        'id': 'reminder-1',
        'records': [
          {
            'scheduledAt': '2030-01-01T09:00:00Z',
            'deliveredAt': '2030-01-01T09:00:01Z',
            'messageId': 'opaque/2',
            'prompt': 'delivered prompt',
          },
        ],
        'nextBefore': 'opaque/2',
        'earlierRecordsUnavailable': true,
        'earlierRecordsPruned': true,
        'retention': {'days': 30, 'records': 200},
      };
      final first = await api.history(
        sessionId: 's',
        id: 'reminder-1',
        limit: 1,
      );
      final next = await api.history(
        sessionId: 's',
        id: 'reminder-1',
        before: first.nextBefore,
      );
      expect(requests.last['payload'], {
        'sessionId': 's',
        'id': 'reminder-1',
        'limit': 20,
        'before': 'opaque/2',
      });
      expect(next.earlierRecordsPruned, true);
      expect(next.earlierRecordsUnavailable, true);
      expect(next.retentionDays, 30);
      expect(next.records.single.prompt, 'delivered prompt');
      await expectLater(
        api.history(sessionId: 's', id: 'reminder-1', limit: 101),
        throwsRangeError,
      );
      expect(requests.length, 2);
    },
  );
  test(
    'business missing results and explicit retry are distinct from execution',
    () async {
      respond = (request) => switch (request['method']) {
        'schedule.history' => {
          'id': 'reminder-1',
          'code': 'schedule_not_found',
        },
        'schedule.delete' => {
          'id': 'reminder-1',
          'deleted': false,
          'code': 'schedule_not_found',
        },
        _ => {'requested': true, 'enabled': false},
      };
      expect(
        (await api.history(sessionId: 's', id: 'reminder-1')).code,
        'schedule_not_found',
      );
      expect(await api.delete(sessionId: 's', id: 'reminder-1'), false);
      await api.retry();
      expect(requests.last['payload'], isEmpty);
    },
  );
  test(
    'invalid catalog cannot silently become an empty successful directory',
    () async {
      respond = (_) => {'items': []};
      await expectLater(
        api.catalog(),
        throwsA(isA<DshException>().having((e) => e.code, 'code', 'protocol')),
      );
    },
  );
  test(
    'server reason and explicit archive consent survive the RPC envelope',
    () async {
      failure = {
        'code': 'agent-busy',
        'message': 'active reminders',
        'details': {'reason': 'active-schedules'},
      };
      await expectLater(
        client.archiveSession('s'),
        throwsA(
          isA<DshException>().having(
            (e) => e.details['reason'],
            'reason',
            'active-schedules',
          ),
        ),
      );
      expect(requests.single['payload'], {'sessionId': 's'});
      failure = null;
      await client.archiveSession('s', stopSchedules: true);
      expect(requests.last['payload'], {
        'sessionId': 's',
        'stopSchedules': true,
      });
    },
  );
  test('cancelled scopes never dispatch mutations or retries', () async {
    final scope = RequestScope()..cancel();
    await expectLater(
      api.create(
        sessionId: 's',
        title: 't',
        prompt: 'p',
        timing: {'after_seconds': 5},
        scope: scope,
      ),
      throwsA(isA<DshException>().having((e) => e.code, 'code', 'cancelled')),
    );
    await expectLater(api.retry(scope: scope), throwsA(isA<DshException>()));
    expect(requests, isEmpty);
  });
  test(
    'bad mutation data is outcome unknown and no automatic retry occurs',
    () async {
      respond = (_) => {'id': 'created-but-incomplete'};
      await expectLater(
        api.create(
          sessionId: 's',
          title: 't',
          prompt: 'p',
          timing: {'after_seconds': 5},
        ),
        throwsA(
          isA<DshException>().having((e) => e.outcomeUnknown, 'unknown', true),
        ),
      );
      expect(requests.length, 1);
    },
  );
  test(
    'timing cannot override the original session or include several selectors',
    () async {
      for (final timing in <Json>[
        {'sessionId': 'another'},
        {'after_seconds': 1, 'every_seconds': 60},
        {},
      ]) {
        await expectLater(
          api.create(sessionId: 's', title: 't', prompt: 'p', timing: timing),
          throwsArgumentError,
        );
      }
      expect(requests, isEmpty);
    },
  );
}
