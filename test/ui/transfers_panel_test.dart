// the button in the top bar that sums up the transfers, and the panel it
// opens, on their own: what they say, how the button fills, and how the
// panel is set out.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mudita/ui/app_shell.dart';
import 'package:mudita/ui/parts.dart';
import 'package:mudita/ui/transfers_panel.dart';

Transfer _transfer(
  String name, {
  bool running = true,
  int done = 0,
  int total = 10,
}) => (
  running: running,
  done: done,
  total: total,
  row: TransferRow(
    key: Key('row-$name'),
    icon: Icons.download,
    name: name,
    caption: 'a caption',
  ),
);

void main() {
  test('transfers are counted in words', () {
    expect(transfersLabel(0), 'no transfers');
    expect(transfersLabel(1), '1 transfer');
    expect(transfersLabel(3), '3 transfers');
  });

  test('the button says how far along the transfers are, when known', () {
    expect(transfersSummary(0, null), 'no transfers');
    expect(transfersSummary(2, 0.555), '55% of 2 transfers');
    expect(transfersSummary(1, 1), '100% of 1 transfer');
    expect(transfersSummary(2, null), '2 transfers');
  });

  test('the whole is the pieces done over the pieces of what is running', () {
    expect(
      transfersProgress([
        _transfer('a', done: 4),
        _transfer('b', done: 1),
        // what has ended is not part of what is left to do.
        _transfer('c', running: false, done: 10),
      ]),
      0.25,
    );
    expect(transfersProgress([_transfer('c', running: false)]), isNull);
    // one that does not know its size yet says nothing of the whole.
    expect(transfersProgress([_transfer('a', total: 0)]), isNull);
    expect(
      transfersProgress([_transfer('a', total: 0), _transfer('b', done: 5)]),
      0.5,
    );
  });

  Finder keyed(String key) => find.byKey(Key(key));

  group('the button', () {
    Future<void> pumpButton(
      WidgetTester tester, {
      required int running,
      double? progress,
      bool open = false,
      VoidCallback? onPressed,
    }) => tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: TransfersButton(
              running: running,
              progress: progress,
              open: open,
              onPressed: onPressed ?? () {},
            ),
          ),
        ),
      ),
    );

    Color borderOf(WidgetTester tester) {
      final box = tester.widget<Container>(keyed('transfers-button'));
      return ((box.decoration! as BoxDecoration).border! as Border).top.color;
    }

    testWidgets('says how many transfers are running, and fills by how far '
        'along they are', (tester) async {
      await pumpButton(tester, running: 3, progress: 0.4);

      expect(find.text('40% of 3 transfers'), findsOneWidget);
      expect(find.byIcon(Icons.swap_vert), findsOneWidget);
      expect(find.byTooltip('transfers'), findsOneWidget);
      final button = tester.getRect(keyed('transfers-button'));
      expect(button.height, kControlHeight);
      expect(button.width, kTransfersButtonWidth);
      final fill = tester.getRect(keyed('transfers-fill'));
      expect(fill.left, moreOrLessEquals(button.left, epsilon: 1.5));
      expect(fill.width, moreOrLessEquals(button.width * 0.4, epsilon: 1.5));
      expect(
        tester.widget<ColoredBox>(keyed('transfers-fill')).color,
        kColorAccentFill,
      );
      expect(
        tester.widget<Text>(find.text('40% of 3 transfers')).style!.color,
        kColorText,
      );
    });

    testWidgets('with nothing running it stays, empty and dim', (tester) async {
      await pumpButton(tester, running: 0);

      final label = tester.widget<Text>(find.text('no transfers'));
      expect(label.style!.color, kColorDim);
      expect(tester.getRect(keyed('transfers-fill')).width, 0);
      expect(borderOf(tester), kColorLineStrong);
    });

    testWidgets('one is "1 transfer", and an open panel shows in its border', (
      tester,
    ) async {
      var pressed = 0;
      await pumpButton(
        tester,
        running: 1,
        progress: 1,
        open: true,
        onPressed: () => pressed++,
      );

      expect(find.text('100% of 1 transfer'), findsOneWidget);
      expect(borderOf(tester), kColorAccent);
      await tester.tap(keyed('transfers-button'));
      expect(pressed, 1);
    });
  });

  group('the panel', () {
    Future<void> pumpPanel(
      WidgetTester tester, {
      List<Transfer> downloads = const [],
      List<Transfer> uploads = const [],
      VoidCallback? onClose,
    }) => tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Align(
            alignment: Alignment.topRight,
            child: SizedBox(
              width: kPanelWidth,
              child: TransfersPanel(
                downloads: downloads,
                uploads: uploads,
                onClose: onClose ?? () {},
              ),
            ),
          ),
        ),
      ),
    );

    String textOf(WidgetTester tester, String key) =>
        tester.widget<Text>(keyed(key)).data!;

    testWidgets('sums up in its head, and lists downloads, then what is '
        'shared', (tester) async {
      var closed = 0;
      await pumpPanel(
        tester,
        downloads: [
          _transfer('down', done: 6),
          _transfer('old', running: false),
        ],
        uploads: [_transfer('up', done: 2)],
        onClose: () => closed++,
      );

      expect(textOf(tester, 'transfers-title'), '2 transfers');
      expect(tester.widget<Text>(keyed('transfers-title')).style, kTextName);
      expect(textOf(tester, 'transfers-percent'), '40%');
      final head = tester.getRect(keyed('transfers-head'));
      expect(
        tester.widget<Container>(keyed('transfers-head')).color,
        kColorHead,
      );
      final bar = find.descendant(
        of: keyed('transfers-head'),
        matching: find.byType(ProgressBar),
      );
      expect(tester.widget<ProgressBar>(bar).value, 0.4);
      expect(tester.getSize(bar).width, kSizeWidth);
      final close = tester.getRect(keyed('transfers-close'));
      expect(close.size, const Size.square(kControlHeight));
      expect(head.right - close.right, kSpaceRow);
      expect(close.top - head.top, kSpaceRow);

      final order = [
        tester.getTopLeft(find.text('downloading')).dy,
        tester.getTopLeft(keyed('row-down')).dy,
        tester.getTopLeft(keyed('row-old')).dy,
        tester.getTopLeft(find.text('sharing')).dy,
        tester.getTopLeft(keyed('row-up')).dy,
      ];
      expect(order, [...order]..sort());
      expect(tester.widget<Text>(find.text('sharing')).style, kTextCaption);
      // the rows keep the panel's inset.
      expect(tester.getRect(keyed('row-down')).left - head.left, kSpaceRow);
      expect(head.right - tester.getRect(keyed('row-down')).right, kSpaceRow);

      await tester.tap(keyed('transfers-close'));
      expect(closed, 1);
    });

    testWidgets('a section with nothing in it is left out', (tester) async {
      await pumpPanel(tester, uploads: [_transfer('up', running: false)]);

      expect(find.text('downloading'), findsNothing);
      expect(find.text('sharing'), findsOneWidget);
      // nothing is running: the head has no bar to show.
      expect(textOf(tester, 'transfers-title'), 'no transfers');
      expect(find.byType(ProgressBar), findsNothing);
      expect(keyed('transfers-percent'), findsNothing);
    });

    testWidgets('with nothing at all it says so', (tester) async {
      await pumpPanel(tester);

      expect(textOf(tester, 'transfers-empty'), 'no transfers yet');
      expect(find.text('downloading'), findsNothing);
      expect(find.text('sharing'), findsNothing);
    });

    testWidgets('a row is two lines, with its bar, what it comes to and its '
        'buttons at the end', (tester) async {
      await pumpPanel(
        tester,
        downloads: [
          (
            running: true,
            done: 5,
            total: 10,
            row: TransferRow(
              key: const Key('row'),
              icon: Icons.download,
              name: 'a-rather-long-name.iso',
              caption: '3.9 GiB, since 14:00',
              captionKey: const Key('caption'),
              progress: 0.5,
              end: '50%',
              actions: [
                AppButton(
                  key: const Key('cancel'),
                  icon: Icons.close,
                  label: 'cancel',
                  width: kPanelButtonWidth,
                  onPressed: () {},
                ),
              ],
            ),
          ),
        ],
      );

      final name = tester.getRect(find.text('a-rather-long-name.iso'));
      final caption = tester.getRect(keyed('caption'));
      expect(caption.top, greaterThanOrEqualTo(name.bottom));
      expect(caption.left, name.left);
      final bar = find.descendant(
        of: keyed('row'),
        matching: find.byType(ProgressBar),
      );
      expect(tester.getSize(bar).width, kSizeWidth);
      expect(tester.widget<Text>(keyed('transfer-end')).data, '50%');
      expect(tester.getSize(keyed('cancel')).width, kPanelButtonWidth);
      expect(kPanelButtonWidth, lessThan(kActionWidth));
    });
  });
}
