import 'package:dsh_desktop/design/select.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  testWidgets('a rejected selection retains its owner value until confirmed', (
    tester,
  ) async {
    final requests = <String>[];
    var value = 'a';
    late StateSetter rebuild;
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              rebuild = setState;
              return Center(
                child: DshSelect<String>(
                  options: const {'a': 'A', 'b': 'Longer option B'},
                  value: value,
                  onChanged: requests.add,
                ),
              );
            },
          ),
        ),
      ),
    );
    await tester.tap(find.byType(DshSelect<String>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Longer option B'));
    await tester.pumpAndSettle();
    expect(requests, ['b']);
    final select = tester.widget<ShadSelect<String>>(
      find.byType(ShadSelect<String>),
    );
    expect(select.controller!.value, {'a'});
    rebuild(() {});
    await tester.pump();
    expect(select.controller!.value, {'a'});
    rebuild(() => value = 'b');
    await tester.pump();
    expect(select.controller!.value, {'b'});
  });

  testWidgets('changing the option inventory closes its stale dropdown', (
    tester,
  ) async {
    var options = <String, String>{'a': 'A', 'b': 'B'};
    late StateSetter rebuild;
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (_, setState) {
              rebuild = setState;
              return Center(
                child: DshSelect<String>(
                  options: options,
                  value: 'a',
                  onChanged: (_) {},
                ),
              );
            },
          ),
        ),
      ),
    );
    await tester.tap(find.byType(DshSelect<String>));
    await tester.pumpAndSettle();
    expect(find.text('B'), findsOneWidget);
    rebuild(() => options = {'a': 'A'});
    await tester.pumpAndSettle();
    expect(find.text('B'), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
