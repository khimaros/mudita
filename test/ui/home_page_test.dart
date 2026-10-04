// bindu's one screen, driven against a fake core: the app decides nothing
// about the network itself, so what is tested here is what it shows and
// what it asks the core to do.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:logging/logging.dart';
import 'package:mudita/core/log_history.dart';
import 'package:mudita/core/share_link.dart';
import 'package:mudita/main.dart';
import 'package:mudita/ui/bindu_model.dart';
import 'package:mudita/ui/app_shell.dart';
import 'package:mudita/ui/download_card.dart';
import 'package:mudita/ui/home_page.dart';
import 'package:mudita/ui/library_browser.dart';
import 'package:mudita/ui/log_card.dart';
import 'package:mudita/ui/parts.dart';
import 'package:mudita/ui/share_card.dart';
import 'package:mudita/ui/transfers_panel.dart';

import '../fakes/fake_core.dart';

const String _kBase = 'https://example.org/app/';
const String _kName = 'notes.txt';
const String _kToken = 'VLD0:abc:def';
const String _kOtherName = 'b.bin';
const String _kOtherToken = 'VLD0:other:key';
const String _kLibrary = 'library/VLD0:lib:key';
const int _kSize = 65000;

// when two downloaders started, in local time as the screen shows it.
final DateTime _kStarted = DateTime(2026, 10, 5, 14, 2);
final DateTime _kLater = DateTime(2026, 10, 5, 14, 3);

// tall enough that every card is laid out at once.
const Size _kSurface = Size(800, 1600);

void main() {
  late FakeCore core;

  // what the system clipboard holds; null plays a browser that keeps the
  // clipboard from the page.
  String? clipboard;

  // the folders a desktop was asked to show.
  late List<String> opened;

  setUp(() {
    clipboard = '';
    opened = [];
    core = FakeCore()
      ..toPick = (name: _kName, size: _kSize)
      ..links[_kToken] = {
        'token': _kToken,
        'size': _kSize,
        'name': '',
        'names': <String>[],
      }
      ..links['$_kToken/$_kName'] = {
        'token': _kToken,
        'size': _kSize,
        'name': _kName,
        'names': [_kName],
      }
      ..links['$_kOtherToken/$_kOtherName'] = {
        'token': _kOtherToken,
        'size': 5,
        'name': _kOtherName,
        'names': [_kOtherName],
      };
  });

  // the model asks the core again every second; a tick lets it catch up.
  Future<void> tick(WidgetTester tester) async {
    await tester.pump(kBinduRefreshEvery);
    await tester.pump();
  }

  final panel = find.byType(TransfersPanel);
  bool panelOpen() => panel.evaluate().isNotEmpty;

  Future<void> openPanel(WidgetTester tester) async {
    if (panelOpen()) return;
    await tester.tap(find.byKey(const Key('transfers-button')));
    await tester.pump();
  }

  Future<void> closePanel(WidgetTester tester) async {
    if (!panelOpen()) return;
    await tester.tap(find.byKey(const Key('transfers-close')));
    await tester.pump();
  }

  void mockClipboard(WidgetTester tester) {
    final messenger = tester.binding.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') {
        clipboard = (call.arguments as Map)['text'] as String;
      }
      if (call.method != 'Clipboard.getData') return null;
      if (clipboard == null) throw PlatformException(code: 'paste_fail');
      return {'text': clipboard};
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(SystemChannels.platform, null),
    );
  }

  Future<void> pumpHome(
    WidgetTester tester, {
    String? initialLink,
    Stream<String> links = const Stream.empty(),
    bool desktop = false,
  }) async {
    tester.view.physicalSize = _kSurface;
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    mockClipboard(tester);
    final history = LogHistory();
    addTearDown(history.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: HomePage(
          core: core,
          history: history,
          linkBase: _kBase,
          initialLink: initialLink,
          links: links,
          // a desktop can show a folder; a browser cannot.
          openFolder: desktop ? (folder) async => opened.add(folder) : null,
        ),
      ),
    );
    await tester.pump();
  }

  // an open panel has a shade over the page: what is on the page is
  // pressed with the panel out of the way, as a user would have to.
  Future<void> press(WidgetTester tester, Finder button) async {
    final inPanel = find.descendant(of: panel, matching: button);
    if (inPanel.evaluate().isEmpty) await closePanel(tester);
    await tester.tap(button);
    await tick(tester);
  }

  Finder keyed(String key) => find.byKey(Key(key));

  // the widget with [key] inside the card with the key [card].
  Finder within(String card, String key) =>
      find.descendant(of: keyed(card), matching: keyed(key));

  Finder ofType(String card, Type type) =>
      find.descendant(of: keyed(card), matching: find.byType(type));

  String textIn(WidgetTester tester, Finder finder) {
    final widget = tester.widget(finder);
    if (widget is ActionText) return widget.text;
    return widget is SelectableText ? widget.data! : (widget as Text).data!;
  }

  String textOf(WidgetTester tester, String key) => textIn(tester, keyed(key));

  // what the head of the library's card says, part by part.
  List<String> path(WidgetTester tester) => [
    for (final part
        in find
            .descendant(of: keyed('path'), matching: find.byType(Text))
            .evaluate())
      (part.widget as Text).data!,
  ];

  Future<void> share(WidgetTester tester) => press(tester, keyed('pick-file'));

  // type a link into the receive side and press enter.
  Future<void> enter(WidgetTester tester, String input) async {
    await press(tester, keyed('switch-receive'));
    await tester.enterText(keyed('token-input'), input);
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tick(tester);
  }

  testWidgets('the app waits for the core, then lets go of it', (tester) async {
    final connecting = Completer<FakeCore>();
    await tester.pumpWidget(
      MuditaApp(connect: () => connecting.future, linkBase: _kBase),
    );
    expect(keyed('starting'), findsOneWidget);

    connecting.complete(core);
    await tester.pump();
    await tester.pump();
    expect(find.byType(HomePage), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    expect(core.closed, isTrue);
  });

  testWidgets('what is logged while the core starts is in the log', (
    tester,
  ) async {
    final connecting = Completer<FakeCore>();
    await tester.pumpWidget(
      MuditaApp(connect: () => connecting.future, linkBase: _kBase),
    );
    Logger('mudita.server').info('connecting to veilid');
    connecting.complete(core);
    await tester.pump();
    await tester.pump();
    expect(textOf(tester, 'log-latest'), endsWith('connecting to veilid'));
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('a core that cannot start says why, and can be tried again', (
    tester,
  ) async {
    var attempts = 0;
    await tester.pumpWidget(
      MuditaApp(
        connect: () async =>
            ++attempts == 1 ? throw Exception('no server binary') : core,
        linkBase: _kBase,
      ),
    );
    await tester.pump();
    expect(textOf(tester, 'core-error'), contains('no server binary'));

    await tester.tap(find.text('retry'));
    await tester.pump();
    await tester.pump();
    expect(find.byType(HomePage), findsOneWidget);
  });

  Finder inChip(String text) =>
      find.descendant(of: keyed('connection'), matching: find.text(text));

  testWidgets('the connection chip follows the core', (tester) async {
    core.ready = false;
    await pumpHome(tester);
    expect(inChip('connecting'), findsOneWidget);

    core.ready = true;
    await tick(tester);
    expect(inChip('connected'), findsOneWidget);

    core.refusing = 'the server went away';
    await tick(tester);
    expect(inChip('the server went away'), findsOneWidget);
  });

  group('the switch', () {
    testWidgets('starts on send and moves when pressed', (tester) async {
      await pumpHome(tester);
      expect(keyed('pick-file'), findsOneWidget);
      expect(keyed('token-input'), findsNothing);

      await press(tester, keyed('switch-receive'));
      expect(keyed('pick-file'), findsNothing);
      expect(keyed('token-input'), findsOneWidget);

      await press(tester, keyed('switch-send'));
      expect(keyed('pick-file'), findsOneWidget);
    });

    testWidgets('is on receive when the app was opened with a link', (
      tester,
    ) async {
      await pumpHome(tester, initialLink: linkFor(_kBase, _kToken, _kName));
      expect(keyed('token-input'), findsOneWidget);
      expect(keyed('pick-file'), findsNothing);
    });

    testWidgets('stays on send for a page address with no link in it', (
      tester,
    ) async {
      await pumpHome(tester, initialLink: _kBase);
      expect(keyed('pick-file'), findsOneWidget);
    });
  });

  group('sending', () {
    testWidgets('with nothing shared there is only the place to add a file', (
      tester,
    ) async {
      await pumpHome(tester);
      expect(find.byType(AddFileArea), findsOneWidget);
      expect(textOf(tester, 'add-file-hint'), 'drop a file here to send it');
      expect(find.byType(ShareCard), findsNothing);
      expect(find.byType(TransferRow), findsNothing);
    });

    testWidgets('a picked file takes the place of the area: hashing, then '
        'the link', (tester) async {
      await pumpHome(tester);
      await share(tester);
      expect(find.byType(AddFileArea), findsNothing);
      expect(keyed('pick-file'), findsNothing);
      expect(find.byType(ShareCard), findsOneWidget);
      expect(textOf(tester, 'share-progress'), 'hashing');
      expect(find.text('50%'), findsOneWidget);
      expect(textOf(tester, 'share-state'), contains('KiB'));
      expect(keyed('share-link'), findsNothing);

      core.finishSharing(_kToken);
      await tick(tester);
      // the link carries the name, so whoever opens it sees what it is.
      expect(textOf(tester, 'share-link'), linkFor(_kBase, _kToken, _kName));
      expect(textOf(tester, 'share-link'), endsWith('/$_kName'));
      expect(textOf(tester, 'share-state'), endsWith('ready to share'));
      expect(find.text(_kName), findsOneWidget);
      // the core said nothing of downloaders, so none is made up.
      expect(textOf(tester, 'transfers-label'), 'no transfers');
    });

    testWidgets('stopping brings the place to add a file back', (tester) async {
      await pumpHome(tester);
      await share(tester);
      core.finishSharing(_kToken);
      await tick(tester);

      await press(tester, keyed('stop-sharing'));
      expect(core.stopped, [1]);
      expect(find.byType(ShareCard), findsNothing);
      expect(find.byType(AddFileArea), findsOneWidget);

      // and the next file is shared in its turn.
      core.toPick = (name: _kOtherName, size: 5);
      await share(tester);
      expect(find.text(_kOtherName), findsOneWidget);
    });

    const first = 'upload-3f9a1c0e7b2d4a68';
    const second = 'upload-b71c4d0200000000';

    testWidgets('the button in the top bar counts the people downloading, '
        'and opens the panel that lists them', (tester) async {
      await pumpHome(tester);
      await share(tester);
      core.finishSharing(_kToken);
      await tick(tester);
      // nobody is downloading: the button stays, and says so.
      expect(textOf(tester, 'transfers-label'), 'no transfers');
      expect(tester.getRect(keyed('transfers-fill')).width, 0);
      // it stands directly left of the connection chip.
      expect(
        tester.getRect(keyed('connection')).left -
            tester.getRect(keyed('transfers-button')).right,
        kSpaceRow,
      );

      core.tellDownloaders([
        FakeCore.downloader('3f9a1c0e7b2d4a68', _kStarted, done: 4),
        FakeCore.downloader('b71c4d0200000000', _kLater, done: 1),
      ]);
      await tick(tester);
      expect(textOf(tester, 'transfers-label'), '25% of 2 transfers');
      expect(
        tester.getRect(keyed('transfers-fill')).width,
        moreOrLessEquals(kTransfersButtonWidth * 0.25, epsilon: 1.5),
      );
      // nothing of it is on the page any more.
      expect(panel, findsNothing);
      expect(find.byType(TransferRow), findsNothing);

      await openPanel(tester);
      expect(textOf(tester, 'transfers-title'), '2 transfers');
      expect(textOf(tester, 'transfers-percent'), '25%');
      expect(find.text('sharing'), findsOneWidget);
      expect(find.text('downloading'), findsNothing);
      expect(find.byType(TransferRow), findsNWidgets(2));
      expect(
        find.descendant(of: keyed(first), matching: find.text(_kName)),
        findsOneWidget,
      );
      // a file shared on its own is in no library.
      expect(
        textIn(tester, within(first, 'upload-caption')),
        'downloader 3f9a, since 14:02',
      );
      expect(textIn(tester, within(first, 'transfer-end')), '40%');
      expect(
        tester.widget<ProgressBar>(ofType(first, ProgressBar)).value,
        closeTo(0.4, 0.001),
      );
      expect(
        textIn(tester, within(second, 'upload-caption')),
        'downloader b71c, since 14:03',
      );
      expect(textIn(tester, within(second, 'transfer-end')), '10%');
      expect(
        tester.getTopLeft(keyed(second)).dy,
        greaterThan(tester.getBottomLeft(keyed(first)).dy),
      );
      // there is nothing to press on such a row.
      expect(ofType(first, AppButton), findsNothing);
    });

    testWidgets('a downloader that finishes is done, and one that leaves went '
        'away', (tester) async {
      await pumpHome(tester);
      await share(tester);
      core
        ..finishSharing(_kToken)
        ..tellDownloaders([
          FakeCore.downloader(
            '3f9a1c0e7b2d4a68',
            _kStarted,
            done: 10,
            state: 'finished',
            seen: _kLater,
          ),
          FakeCore.downloader(
            'b71c4d0200000000',
            _kStarted,
            done: 6,
            state: 'gone',
            seen: _kLater,
          ),
        ]);
      await tick(tester);
      await openPanel(tester);

      expect(textOf(tester, 'transfers-title'), 'no transfers');
      expect(find.byType(ProgressBar), findsNothing);
      expect(
        textIn(tester, within(first, 'upload-caption')),
        'downloader 3f9a, finished at 14:03',
      );
      expect(textIn(tester, within(first, 'transfer-end')), 'done');
      expect(tester.widget<Icon>(ofType(first, Icon)).color, kColorGreen);
      expect(
        textIn(tester, within(second, 'upload-caption')),
        'downloader b71c, went away at 14:03',
      );
      // how far it got before it left.
      expect(textIn(tester, within(second, 'transfer-end')), '60%');
      expect(tester.widget<TransferRow>(keyed(second)).dimmed, isTrue);
    });

    testWidgets('the core\'s own list of uploads is used when it has one', (
      tester,
    ) async {
      await pumpHome(tester);
      await share(tester);
      core
        ..finishSharing(_kToken)
        ..uploads = [
          {
            'id': '77d0000000000000',
            'name': 'renamed.txt',
            'library': null,
            'done': 5,
            'total': 10,
            'size': _kSize,
            'state': 'downloading',
            'started': _kStarted.millisecondsSinceEpoch ~/ 1000,
            'seen': _kStarted.millisecondsSinceEpoch ~/ 1000,
          },
        ];
      await tick(tester);
      await openPanel(tester);

      expect(
        find.descendant(
          of: keyed('upload-77d0000000000000'),
          matching: find.text('renamed.txt'),
        ),
        findsOneWidget,
      );
      expect(textOf(tester, 'transfers-percent'), '50%');
      // the core answering the method with nothing in it is no error.
      core.uploads = [];
      await tick(tester);
      expect(textOf(tester, 'transfers-empty'), 'no transfers yet');
      expect(find.text('connected'), findsOneWidget);
    });

    testWidgets('the panel is a drawer under the top bar, closed by its x '
        'or by the shade', (tester) async {
      await pumpHome(tester);
      expect(panel, findsNothing);

      await openPanel(tester);
      final drawer = tester.getRect(panel);
      expect(drawer.width, kPanelWidth);
      expect(drawer.top, kTopBarHeight);
      expect(drawer.right, _kSurface.width);
      expect(drawer.bottom, _kSurface.height);
      expect(textOf(tester, 'transfers-empty'), 'no transfers yet');
      // the shade is over the rest of the page, the log bar included.
      final shade = tester.getRect(keyed('transfers-shade'));
      expect(shade.left, 0);
      expect(shade.right, drawer.left);
      expect(shade.bottom, _kSurface.height);

      await tester.tap(keyed('transfers-close'));
      await tester.pump();
      expect(panel, findsNothing);

      await openPanel(tester);
      await tester.tap(keyed('transfers-shade'));
      await tester.pump();
      expect(panel, findsNothing);

      // the button opens and closes it too.
      await openPanel(tester);
      await tester.tap(keyed('transfers-button'));
      await tester.pump();
      expect(panel, findsNothing);
    });

    testWidgets('a share the core gave up on says why', (tester) async {
      await pumpHome(tester);
      await share(tester);
      core.shares.last.addAll({'state': 'error', 'error': 'file vanished'});
      await tick(tester);

      expect(textOf(tester, 'share-error'), endsWith('file vanished'));
      await press(tester, keyed('stop-sharing'));
      expect(find.byType(ShareCard), findsNothing);
    });

    testWidgets('backing out of the file dialog changes nothing', (
      tester,
    ) async {
      core.toPick = null;
      await pumpHome(tester);
      await share(tester);

      expect(keyed('pick-file'), findsOneWidget);
      expect(find.byType(ShareCard), findsNothing);
      expect(core.shares, isEmpty);
    });

    testWidgets('copy puts the link on the clipboard and says so', (
      tester,
    ) async {
      await pumpHome(tester);
      await share(tester);
      core.finishSharing(_kToken);
      await tick(tester);

      await tester.tap(keyed('copy-link'));
      await tester.pump();

      expect(clipboard, linkFor(_kBase, _kToken, _kName));
      expect(find.text('link copied'), findsOneWidget);
    });

    testWidgets('a file dropped on the page is shared, from either side', (
      tester,
    ) async {
      await pumpHome(tester, initialLink: linkFor(_kBase, _kToken, _kName));
      expect(find.text('drop a file here to send it'), findsNothing);

      core.drop(1);
      await tick(tester);
      expect(find.byType(ShareCard), findsOneWidget);
    });

    testWidgets('a file dropped while one is shared is refused', (
      tester,
    ) async {
      await pumpHome(tester);
      await share(tester);

      core
        ..toPick = (name: _kOtherName, size: 5)
        ..drop(1);
      await tick(tester);
      expect(find.text('stop sharing the current file first'), findsOneWidget);
      expect(core.shares, hasLength(1));
      expect(find.text(_kName), findsOneWidget);
      expect(find.text(_kOtherName), findsNothing);
    });

    testWidgets('several files dropped together are refused', (tester) async {
      await pumpHome(tester);

      core.drop(2);
      await tick(tester);
      expect(find.text('one file at a time'), findsOneWidget);
      expect(core.shares, isEmpty);
    });

    testWidgets('where a drop cannot be seen, none is invited', (tester) async {
      core.droppable = false;
      await pumpHome(tester);
      expect(textOf(tester, 'add-file-hint'), 'choose a file to send');
    });
  });

  // a file that was looked up and is waiting on the user: named, with its
  // size, and with nothing asked of the core beyond the look.
  void expectFoundNotDownloaded(WidgetTester tester) {
    expect(textOf(tester, 'download-name'), _kName);
    expect(textOf(tester, 'download-ready'), contains('KiB'));
    expect(keyed('download'), findsOneWidget);
    expect(core.started, isEmpty);
    expect(core.saved, isEmpty);
  }

  group('receiving', () {
    testWidgets('a pasted link waits on the page, and moves to the panel '
        'when its download is started', (tester) async {
      await pumpHome(tester);
      await press(tester, keyed('switch-receive'));
      expect(keyed('pending'), findsNothing);

      await enter(tester, '$_kToken/$_kName');
      // it waits on the page, under the link row; pasting opens nothing,
      // and a file that waits is no transfer.
      expect(panel, findsNothing);
      expect(within('pending', 'incoming-0'), findsOneWidget);
      expect(textOf(tester, 'transfers-label'), 'no transfers');
      await openPanel(tester);
      expect(textOf(tester, 'transfers-empty'), 'no transfers yet');
      expect(
        find.descendant(of: panel, matching: keyed('download')),
        findsNothing,
      );

      // started, it leaves the page, and the panel opens to show it.
      await press(tester, keyed('download'));
      expect(keyed('pending'), findsNothing);
      expect(panel, findsOneWidget);
      expect(
        find.descendant(of: panel, matching: keyed('incoming-0')),
        findsOneWidget,
      );
      expect(find.text('downloading'), findsOneWidget);
      expect(textOf(tester, 'transfers-title'), '1 transfer');
      expect(textOf(tester, 'transfers-label'), '33% of 1 transfer');
      expect(textOf(tester, 'transfers-percent'), '33%');
      // the panel never offers to start one.
      expect(keyed('download'), findsNothing);
    });

    testWidgets('the link row comes first, then what waits, then the '
        'library', (tester) async {
      core
        ..libraryName = 'the shelf'
        ..libraries[_kLibrary] = [
          {
            'name': 'a.bin',
            'size': 1,
            'root': 'aa' * 32,
            'folder': '',
            'token': 'VLD0:a:key',
          },
        ];
      await pumpHome(tester, initialLink: _kLibrary);
      await enter(tester, '$_kToken/$_kName');

      final order = [
        tester.getTopLeft(find.byType(AddLinkRow)).dy,
        tester.getTopLeft(keyed('pending')).dy,
        tester.getTopLeft(find.byType(LibraryBrowser)).dy,
      ];
      expect(order, [...order]..sort());
      expect(
        tester.getTopLeft(find.byType(AddLinkRow)).dy,
        greaterThan(tester.getBottomLeft(keyed('switch-receive')).dy),
      );
    });

    testWidgets('a link the app was opened with shows its file and waits', (
      tester,
    ) async {
      await pumpHome(tester, initialLink: linkFor(_kBase, _kToken, _kName));
      await tick(tester);
      expectFoundNotDownloaded(tester);
      expect(
        tester.widget<AppButton>(keyed('download')).kind,
        ButtonKind.primary,
      );

      await press(tester, keyed('download'));
      expect(core.started.single, (token: _kToken, name: _kName));
      expect(textIn(tester, within('incoming-0', 'transfer-end')), '33%');
      expect(
        tester.widget<ProgressBar>(ofType('incoming-0', ProgressBar)).value,
        closeTo(1 / 3, 0.001),
      );
      expect(tester.widget<AppButton>(keyed('cancel')).kind, ButtonKind.danger);
      expect(tester.getSize(keyed('cancel')).width, kPanelButtonWidth);
      // two lines: the name, then its size and since when.
      expect(
        textOf(tester, 'download-running'),
        matches(r'^87\.9 KiB, since \d\d:\d\d$'),
      );

      core.finishDownload();
      await tick(tester);
      expect(
        textOf(tester, 'download-done'),
        matches(r'^87\.9 KiB, saved at \d\d:\d\d$'),
      );
      expect(textIn(tester, within('incoming-0', 'transfer-end')), 'done');
      expect(
        tester.widget<Icon>(ofType('incoming-0', Icon).first).color,
        kColorGreen,
      );
      expect(find.widgetWithText(AppButton, 'dismiss'), findsOneWidget);
      // handed over once, however long it stays listed.
      await tick(tester);
      expect(core.saved, hasLength(1));
    });

    testWidgets('a finished download is shown in its folder on a desktop', (
      tester,
    ) async {
      core.savedIn = '/home/me/Downloads';
      await pumpHome(
        tester,
        initialLink: linkFor(_kBase, _kToken, _kName),
        desktop: true,
      );
      await tick(tester);
      await press(tester, keyed('download'));
      core.finishDownload();
      await tick(tester);

      // the file is where the user said: there is nothing to save again.
      expect(keyed('save-file'), findsNothing);
      await press(tester, keyed('show-in-folder'));
      expect(opened, ['/home/me/Downloads']);
    });

    testWidgets(
      'a link opened before the network is there is looked up once it is',
      (tester) async {
        core.ready = false;
        await pumpHome(tester, initialLink: linkFor(_kBase, _kToken, _kName));
        await tick(tester);
        expect(keyed('download-error'), findsNothing);
        expect(keyed('download-looking'), findsOneWidget);

        core.ready = true;
        await tick(tester);
        expectFoundNotDownloaded(tester);
      },
    );

    testWidgets('a link pasted into the open tab shows its file and waits', (
      tester,
    ) async {
      final links = StreamController<String>();
      addTearDown(links.close);
      await pumpHome(tester, links: links.stream);
      expect(keyed('token-input'), findsNothing);

      links.add(linkFor(_kBase, _kToken, _kName));
      await tick(tester);
      expectFoundNotDownloaded(tester);

      // the same link again is the same file, not a second one.
      links.add(linkFor(_kBase, _kToken, _kName));
      await tick(tester);
      expect(find.byType(TransferRow), findsOneWidget);
    });

    testWidgets('a link typed in is looked up, and the field made ready for '
        'the next', (tester) async {
      await pumpHome(tester);
      await enter(tester, '$_kToken/$_kName');

      expectFoundNotDownloaded(tester);
      expect(
        tester.widget<TextField>(keyed('token-input')).controller!.text,
        '',
      );
    });

    testWidgets('paste takes the link from the clipboard', (tester) async {
      await pumpHome(tester);
      clipboard = linkFor(_kBase, _kToken, _kName);
      await press(tester, keyed('switch-receive'));
      await press(tester, keyed('paste'));

      expectFoundNotDownloaded(tester);
    });

    for (final unreadable in [null, '  ']) {
      testWidgets('paste with ${unreadable == null ? 'no' : 'an empty'} '
          'clipboard points at the keyboard', (tester) async {
        await pumpHome(tester);
        clipboard = unreadable;
        await press(tester, keyed('switch-receive'));
        await press(tester, keyed('paste'));

        expect(find.text('press ctrl+v in the field'), findsOneWidget);
        expect(find.byType(TransferRow), findsNothing);
      });
    }

    testWidgets('a dismissed save dialog can be reopened', (tester) async {
      core.acceptSave = false;
      await pumpHome(tester);
      await enter(tester, '$_kToken/$_kName');
      await press(tester, keyed('download'));
      core.finishDownload();
      await tick(tester);
      expect(textOf(tester, 'download-done'), endsWith(', not saved yet'));
      // in a browser there is no folder to show.
      expect(keyed('show-in-folder'), findsNothing);
      expect(find.byTooltip('save'), findsOneWidget);

      core.acceptSave = true;
      await press(tester, keyed('save-file'));
      expect(textOf(tester, 'download-done'), contains(', saved at '));
      expect(find.byTooltip('save again'), findsOneWidget);

      await press(tester, keyed('save-file'));
      expect(core.saved, hasLength(2));
    });

    testWidgets('two files are listed at once, each with its own state', (
      tester,
    ) async {
      await pumpHome(tester, initialLink: linkFor(_kBase, _kToken, _kName));
      await enter(tester, '$_kOtherToken/$_kOtherName');
      expect(find.byType(TransferRow), findsNWidgets(2));
      expect(core.started, isEmpty);

      await press(tester, within('incoming-1', 'download'));
      expect(core.started.single, (token: _kOtherToken, name: _kOtherName));
      // one is in the panel now, and the other still waits on the page.
      expect(within('incoming-1', 'cancel'), findsOneWidget);
      expect(
        find.descendant(of: panel, matching: keyed('incoming-0')),
        findsNothing,
      );
      await closePanel(tester);
      expect(within('pending', 'incoming-0'), findsOneWidget);
      expect(within('pending', 'incoming-1'), findsNothing);

      await press(tester, within('incoming-0', 'download'));
      expect(textOf(tester, 'transfers-title'), '2 transfers');
      core.finishDownload();
      await tick(tester);
      expect(within('incoming-0', 'download-done'), findsOneWidget);
      expect(within('incoming-1', 'cancel'), findsOneWidget);
      expect(textOf(tester, 'transfers-title'), '1 transfer');
      // what is running comes before what has ended.
      expect(
        tester.getTopLeft(keyed('incoming-1')).dy,
        lessThan(tester.getTopLeft(keyed('incoming-0')).dy),
      );
      expect(core.discarded, isEmpty);
    });

    testWidgets('a waiting file is dismissed without the core hearing of it', (
      tester,
    ) async {
      await pumpHome(tester, initialLink: linkFor(_kBase, _kToken, _kName));
      await enter(tester, '$_kOtherToken/$_kOtherName');

      await press(tester, within('incoming-0', 'dismiss'));
      expect(keyed('incoming-0'), findsNothing);
      expect(keyed('incoming-1'), findsOneWidget);
      expect(core.discarded, isEmpty);
    });

    testWidgets('cancelling a download discards it in the core', (
      tester,
    ) async {
      await pumpHome(tester, initialLink: linkFor(_kBase, _kToken, _kName));
      await tick(tester);
      await press(tester, keyed('download'));

      await press(tester, keyed('cancel'));
      expect(find.byType(TransferRow), findsNothing);
      expect(core.discarded, hasLength(1));
      expect(core.downloads, isEmpty);
    });

    testWidgets('dismissing a finished download stops it being shared on', (
      tester,
    ) async {
      await pumpHome(tester, initialLink: linkFor(_kBase, _kToken, _kName));
      await tick(tester);
      await press(tester, keyed('download'));
      core.finishDownload();
      await tick(tester);

      await press(tester, keyed('dismiss'));
      expect(find.byType(TransferRow), findsNothing);
      expect(core.discarded, hasLength(1));
    });

    testWidgets('a download that fails says why, and can be dismissed', (
      tester,
    ) async {
      await pumpHome(tester, initialLink: linkFor(_kBase, _kToken, _kName));
      await tick(tester);
      await press(tester, keyed('download'));
      core.failDownload('no route to any peer');
      await tick(tester);
      final reason = tester.widget<Text>(keyed('download-error'));
      expect(reason.data, 'no route to any peer');
      expect(reason.style!.color, kColorRed);
      expect(find.widgetWithText(AppButton, 'dismiss'), findsOneWidget);

      await press(tester, keyed('dismiss'));
      expect(find.byType(TransferRow), findsNothing);
      expect(core.discarded, hasLength(1));
    });

    testWidgets('empty input asks for a link instead of looking one up', (
      tester,
    ) async {
      await pumpHome(tester);
      await enter(tester, '   ');

      expect(find.text('paste a share link or token'), findsOneWidget);
      expect(find.byType(TransferRow), findsNothing);
    });

    testWidgets('a token that leads nowhere reports the failure', (
      tester,
    ) async {
      await pumpHome(tester);
      await enter(tester, 'VLD0:nothing:here');

      // a lookup that fails says why on the page, where the link was given.
      expect(panel, findsNothing);
      final reason = tester.widget<Text>(within('pending', 'download-error'));
      expect(reason.data, contains('nothing is shared'));
      expect(reason.style!.color, kColorRed);
      expect(keyed('download'), findsNothing);
      expect(core.started, isEmpty);

      await press(tester, keyed('dismiss'));
      expect(find.byType(TransferRow), findsNothing);
    });
  });

  group('a bare token', () {
    setUp(() {
      core.links[_kToken] = {
        'token': _kToken,
        'size': _kSize,
        'name': 'notes.txt',
        'names': ['notes.txt', 'minutes.txt'],
      };
    });

    testWidgets('shows every name the file is known by', (tester) async {
      await pumpHome(tester, initialLink: _kToken);
      await tick(tester);

      expect(keyed('download-name-notes.txt'), findsOneWidget);
      expect(keyed('download-name-minutes.txt'), findsOneWidget);
      // the most common name is the one offered.
      expect(textOf(tester, 'download-name'), 'notes.txt');
    });

    testWidgets('is saved under the name the user picks', (tester) async {
      await pumpHome(tester, initialLink: _kToken);
      await tick(tester);

      await tester.tap(keyed('download-name-minutes.txt'));
      await tester.pump();
      expect(textOf(tester, 'download-name'), 'minutes.txt');
      await press(tester, keyed('download'));

      expect(core.started.single, (token: _kToken, name: 'minutes.txt'));
    });

    testWidgets('that nobody has named is saved as "download"', (tester) async {
      core.links[_kToken] = {
        'token': _kToken,
        'size': _kSize,
        'name': '',
        'names': <String>[],
      };
      await pumpHome(tester);
      await enter(tester, _kToken);
      await press(tester, keyed('download'));

      expect(core.started.single.name, kFallbackName);
    });
  });

  group('a link to a library', () {
    setUp(() {
      core
        ..libraryName = 'the shelf'
        ..libraries[_kLibrary] = [
          {
            'name': _kName,
            'size': _kSize,
            'root': 'aa' * 32,
            'folder': 'docs',
            'token': _kToken,
          },
          {
            'name': 'other.bin',
            'size': 300,
            'root': 'bb' * 32,
            'folder': 'docs/more',
            'token': 'VLD0:other:key',
          },
          {
            'name': 'third.bin',
            'size': 1,
            'root': 'cc' * 32,
            'folder': 'docs/more',
            'token': 'VLD0:third:key',
          },
        ];
    });

    testWidgets('lists the library, page after page, folder by folder', (
      tester,
    ) async {
      await pumpHome(
        tester,
        initialLink: libraryLinkFor(_kBase, 'VLD0:lib:key'),
      );
      await tick(tester);

      // a library is something received, so that side is showing, in the
      // browser the library's window has: at its top, with all pages read.
      expect(find.byType(LibraryBrowser), findsOneWidget);
      expect(core.browsed, [0, 2]);
      expect(keyed('library-status'), findsNothing);
      // the path starts at the library: there is no list of libraries here
      // to go back to, and so no arrow.
      expect(path(tester), ['the shelf']);
      expect(keyed('back'), findsNothing);
      expect(find.text('libraries'), findsNothing);
      final picture = find.descendant(
        of: keyed('card-head'),
        matching: find.byIcon(Icons.local_library_outlined),
      );
      expect(picture, findsOneWidget);
      // only the top level shows: here a folder, and none of the files.
      expect(keyed('browse-folder-docs'), findsOneWidget);
      expect(find.text('2 files'), findsNothing);
      expect(
        find.descendant(
          of: keyed('browse-folder-docs'),
          matching: find.text('3 files'),
        ),
        findsOneWidget,
      );
      expect(find.text(_kName), findsNothing);
      expect(find.text('third.bin'), findsNothing);
      // nothing is downloaded for having been listed.
      expect(core.started, isEmpty);
      expect(keyed('pending'), findsNothing);
      expect(textOf(tester, 'transfers-label'), 'no transfers');
      // the link it was opened by is there to hand on.
      await press(tester, keyed('copy-link'));
      expect(clipboard, libraryLinkFor(_kBase, 'VLD0:lib:key'));
    });

    testWidgets('is gone through by folder row and by path', (tester) async {
      await pumpHome(tester, initialLink: _kLibrary);
      await tick(tester);

      await press(tester, keyed('browse-folder-docs'));
      expect(path(tester), ['the shelf', '/', 'docs']);
      expect(find.text(_kName), findsOneWidget);
      expect(find.text('third.bin'), findsNothing);
      await press(tester, keyed('browse-folder-docs/more'));
      expect(path(tester), ['the shelf', '/', 'docs', '/', 'more']);
      expect(find.text('third.bin'), findsOneWidget);

      await press(tester, keyed('path-1'));
      expect(path(tester), ['the shelf', '/', 'docs']);
      await press(tester, keyed('path-0'));
      expect(path(tester), ['the shelf']);
      expect(keyed('browse-folder-docs'), findsOneWidget);
    });

    testWidgets('a file picked out of it is downloaded under its name, and '
        'shows among the transfers at once', (tester) async {
      await pumpHome(tester, initialLink: _kLibrary);
      await tick(tester);
      await press(tester, keyed('browse-folder-docs'));
      expect(find.byTooltip('download'), findsOneWidget);

      await tester.tap(keyed('catalog-download-docs/$_kName'));
      await tester.pump();
      // running from the press: it never waits on the page, and the panel
      // opens to show it.
      expect(keyed('pending'), findsNothing);
      expect(panel, findsOneWidget);
      expect(keyed('download'), findsNothing);
      await tick(tester);

      expect(core.started.single, (token: _kToken, name: _kName));
      expect(textOf(tester, 'download-name'), _kName);
      expect(keyed('cancel'), findsOneWidget);
      expect(keyed('download'), findsNothing);
      // while it runs it is not offered a second time.
      expect(keyed('catalog-download-docs/$_kName'), findsNothing);
      expect(
        tester
            .widget<AppButton>(keyed('catalog-downloading-docs/$_kName'))
            .onPressed,
        isNull,
      );

      // a second file from it is listed beside the first.
      await press(tester, keyed('browse-folder-docs/more'));
      await press(tester, keyed('catalog-download-docs/more/third.bin'));
      expect(find.byType(TransferRow), findsNWidgets(2));
    });

    testWidgets('typed into the link field, it opens the library', (
      tester,
    ) async {
      await pumpHome(tester);
      expect(find.byType(LibraryBrowser), findsNothing);

      await enter(tester, libraryLinkFor(_kBase, 'VLD0:lib:key'));

      expect(path(tester), ['the shelf']);
      expect(keyed('browse-folder-docs'), findsOneWidget);
      expect(core.started, isEmpty);
    });

    testWidgets('is closed by the x in its head, and replaced by another '
        'link', (tester) async {
      core.libraries['library/VLD0:other:lib'] = [
        {
          'name': 'elsewhere.bin',
          'size': 1,
          'root': 'ee' * 32,
          'folder': '',
          'token': 'VLD0:else:key',
        },
      ];
      await pumpHome(tester, initialLink: _kLibrary);
      await tick(tester);
      await press(tester, keyed('browse-folder-docs'));

      await enter(tester, 'library/VLD0:other:lib');
      expect(find.byType(LibraryBrowser), findsOneWidget);
      expect(find.text('elsewhere.bin'), findsOneWidget);
      expect(keyed('browse-folder-docs'), findsNothing);

      expect(find.byTooltip('close'), findsOneWidget);
      await press(tester, keyed('close-library'));
      expect(find.byType(LibraryBrowser), findsNothing);
      expect(keyed('token-input'), findsOneWidget);
    });

    testWidgets('opened before the network is there, it waits and then lists', (
      tester,
    ) async {
      core.ready = false;
      await pumpHome(tester, initialLink: _kLibrary);
      await tick(tester);
      expect(keyed('library-error'), findsNothing);
      expect(keyed('library-loading'), findsOneWidget);
      expect(textOf(tester, 'library-status'), 'looking up the library');
      // until it says what it is called, it is called a library.
      expect(path(tester), ['library']);

      core.ready = true;
      await tick(tester);
      expect(path(tester), ['the shelf']);
      expect(keyed('library-status'), findsNothing);
      expect(keyed('library-loading'), findsNothing);
    });

    testWidgets('a library that is gone is reported, and can be retried', (
      tester,
    ) async {
      final catalog = core.libraries.remove(_kLibrary)!;
      await pumpHome(tester, initialLink: _kLibrary);
      await tick(tester);
      expect(textOf(tester, 'library-error'), contains('offline'));

      core.libraries[_kLibrary] = catalog;
      await press(tester, find.text('refresh'));
      expect(keyed('library-error'), findsNothing);
      expect(keyed('browse-folder-docs'), findsOneWidget);
    });

    testWidgets('a page that fails keeps what was listed, and refresh '
        'carries on from there', (tester) async {
      core.browseFailsFrom = 2;
      await pumpHome(tester, initialLink: _kLibrary);
      await tick(tester);
      expect(textOf(tester, 'library-error'), contains('offline'));
      // the folder the first page named is there, with what came of it.
      Finder counted(String count) => find.descendant(
        of: keyed('browse-folder-docs'),
        matching: find.text(count),
      );
      expect(counted('2 files'), findsOneWidget);

      core.browseFailsFrom = null;
      await press(tester, keyed('library-refresh'));
      expect(keyed('library-error'), findsNothing);
      expect(counted('3 files'), findsOneWidget);
      // the first page was not asked for a second time.
      expect(core.browsed, [0, 2]);
    });

    testWidgets('folders come before the files beside them', (tester) async {
      core.libraries[_kLibrary]!.insert(0, {
        'name': 'top.bin',
        'size': 9,
        'root': 'dd' * 32,
        'folder': '',
        'token': 'VLD0:top:key',
      });
      await pumpHome(tester, initialLink: _kLibrary);
      await tick(tester);
      await tick(tester);

      expect(find.text('(top folder)'), findsNothing);
      expect(
        tester.getTopLeft(find.text('docs')).dy,
        lessThan(tester.getTopLeft(find.text('top.bin')).dy),
      );
    });
  });

  group('the log', () {
    testWidgets('is a bar along the bottom showing the latest line', (
      tester,
    ) async {
      await pumpHome(tester);
      expect(textOf(tester, 'log-latest'), 'nothing yet.');
      expect(keyed('activity-log'), findsNothing);
      expect(
        tester.getTopLeft(find.byType(LogBar)).dy,
        greaterThan(tester.getBottomLeft(keyed('pick-file')).dy),
      );
      expect(tester.getBottomLeft(find.byType(LogBar)).dy, _kSurface.height);

      // the core's lines arrive under its own logger names.
      Logger('mudita.peer').info('serving from slot 3');
      Logger('mudita.server').warning('route replaced');
      Logger('mudita.peer').fine('a detail for developers');
      Logger('elsewhere').severe('not ours');
      await tester.pump();
      expect(
        textOf(tester, 'log-latest'),
        matches(r'^\d\d:\d\d:\d\d WARNING route replaced$'),
      );
    });

    testWidgets('opens into every line, and its top strip closes it', (
      tester,
    ) async {
      await pumpHome(tester);
      Logger('mudita.peer').info('serving from slot 3');
      Logger('mudita.server').warning('route replaced');
      await press(tester, keyed('log-bar'));

      expect(keyed('log-latest'), findsNothing);
      final lines = textOf(tester, 'activity-log').split('\n');
      expect(lines.first, matches(r'^\d\d:\d\d:\d\d WARNING route replaced$'));
      expect(lines.last, matches(r'^\d\d:\d\d:\d\d INFO serving from slot 3$'));

      expect(keyed('log-copy'), findsNothing);
      // the whole strip closes it, not only the arrow at its end.
      final strip = tester.getRect(keyed('log-close'));
      expect(strip.width, tester.getSize(find.byType(LogBar)).width);
      await tester.tapAt(strip.center);
      await tester.pump();
      expect(keyed('activity-log'), findsNothing);
      expect(keyed('log-latest'), findsOneWidget);
    });
  });
}
