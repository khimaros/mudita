import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:logging/logging.dart';
import 'package:mudita/core/log_history.dart';
import 'package:mudita/ui/log_card.dart';

final Logger _share = Logger('mudita.share');
final Logger _download = Logger('mudita.download');
final Logger _other = Logger('elsewhere');

const Key _kLog = Key('activity-log');
final RegExp _kLine = RegExp(r'^\d\d:\d\d:\d\d (INFO|WARNING|SEVERE) \S');

List<String> _messages(LogHistory history) => [
  for (final record in history.records) record.message,
];

String _shown(WidgetTester tester) =>
    tester.widget<SelectableText>(find.byKey(_kLog)).data!;

void main() {
  // FINE records only exist at all once the root logger lets them through.
  setUp(() => Logger.root.level = Level.ALL);
  tearDown(() => Logger.root.level = Level.INFO);

  test('a record is shown as its time of day, level and message', () {
    final line = formatRecord(
      LogRecord(Level.WARNING, 'route replaced', 'mudita.share'),
    );
    expect(line, matches(_kLine));
    expect(line, endsWith(' WARNING route replaced'));
  });

  test('history keeps INFO and above from the app loggers, oldest first', () {
    final history = LogHistory();
    addTearDown(history.dispose);
    _share.info('sharing a.bin');
    _share.fine('reply detail');
    _other.severe('not ours');
    _download.severe('download failed: no route');
    expect(_messages(history), ['sharing a.bin', 'download failed: no route']);
    expect(history.records.last.level, Level.SEVERE);
  });

  test('history tells its listeners and forgets its oldest records', () {
    final history = LogHistory();
    addTearDown(history.dispose);
    var notified = 0;
    history.addListener(() => notified++);
    for (var i = 0; i <= kMaxLogRecords; i++) {
      _share.info('line $i');
    }
    expect(notified, kMaxLogRecords + 1);
    expect(history.records, hasLength(kMaxLogRecords));
    expect(history.records.first.message, 'line 1');
  });

  test('a disposed history stops listening', () {
    final history = LogHistory()..dispose();
    _share.info('too late');
    expect(history.records, isEmpty);
  });

  testWidgets('the card shows both halves, newest first', (tester) async {
    final history = LogHistory();
    addTearDown(history.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: LogCard(history: history)),
      ),
    );
    expect(_shown(tester), 'nothing yet.');

    _share.info('sharing a.bin');
    _download.warning('piece 3 failed, retrying: timeout');
    await tester.pump();

    final lines = _shown(tester).split('\n');
    expect(lines, hasLength(2));
    expect(lines, everyElement(matches(_kLine)));
    expect(lines.first, endsWith(' WARNING piece 3 failed, retrying: timeout'));
    expect(lines.last, endsWith(' INFO sharing a.bin'));
  });

  testWidgets('a card told to fill takes the height it is given', (
    tester,
  ) async {
    final history = LogHistory();
    addTearDown(history.dispose);
    Future<double> heightOf({required bool fill}) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Column(
              children: [
                if (fill)
                  Expanded(child: LogCard(history: history, fill: true))
                else
                  LogCard(history: history),
              ],
            ),
          ),
        ),
      );
      return tester.getSize(find.byType(LogCard)).height;
    }

    final screen =
        tester.view.physicalSize.height / tester.view.devicePixelRatio;
    expect(await heightOf(fill: false), lessThan(screen / 2));
    expect(await heightOf(fill: true), screen);

    // with more lines than fit, it scrolls inside that height.
    for (var i = 0; i < 100; i++) {
      _share.info('line $i');
    }
    await tester.pump();
    expect(tester.getSize(find.byType(LogCard)).height, screen);
    expect(tester.takeException(), isNull);
  });
}
