// the library's window, driven against a fake of the server held in
// memory: the window decides nothing about the network itself, so what is
// tested here is what it shows and what it asks the server and the desktop
// to do.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:logging/logging.dart';
import 'package:mudita/core/log_history.dart';
import 'package:mudita/core/share_link.dart';
import 'package:mudita/desktop/desktop.dart';
import 'package:mudita/desktop/library_model.dart';
import 'package:mudita/desktop/ui/home.dart';
import 'package:mudita/desktop/ui/pieces.dart';
import 'package:mudita/ui/app_shell.dart';
import 'package:mudita/ui/download_card.dart';
import 'package:mudita/ui/transfers_panel.dart';
import 'package:mudita/ui/log_card.dart';
import 'package:mudita/ui/parts.dart';

import '../fakes/fake_library.dart';

const String _kImages = '/home/me/images';
const String _kDemo = '$_kImages/demo/behead-demo.img.gz';
const String _kKodi = '$_kImages/kodi.img.gz';
const String _kDownloads = '/home/me/Downloads/mudita';
const String _kLibraryLink = 'library/VLD0:lib:key';
const String _kFileLink = 'VLD0%3Aabc%3Adef/notes.txt';
const List<String> _kSideOrder = ['search', 'discover', 'downloads', 'shared'];

// wide enough for the window's column, and tall enough that every card is
// laid out at once.
const Size _kSurface = Size(1000, 2400);

// owns the model for as long as the window shows, as the app's own
// entrypoint does, so that the model stops asking when the window goes.
class _Window extends StatefulWidget {
  const _Window({
    required this.core,
    required this.history,
    required this.desktop,
  });

  final FakeLibraryCore core;
  final LogHistory history;
  final Desktop desktop;

  @override
  State<_Window> createState() => _WindowState();
}

class _WindowState extends State<_Window> {
  late final LibraryModel _model = LibraryModel(widget.core)..start();

  @override
  void dispose() {
    _model.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => LibraryHome(
    model: _model,
    history: widget.history,
    desktop: widget.desktop,
  );
}

void main() {
  late FakeLibraryCore core;
  late FakeLibrary state;

  // what the system clipboard holds.
  late String clipboard;

  // the desktop: the folder the user chooses next (null plays them backing
  // out), and the folders shown to them.
  String? toPick;
  late List<String> opened;

  setUp(() {
    clipboard = '';
    toPick = null;
    opened = [];
    core = FakeLibraryCore();
    state = core.state
      ..libraries.add({
        'name': 'linux images',
        'folders': [_kImages],
        'listed': true,
        'token': 'VLD0:images:key',
      })
      ..files = [
        sharedJson('linux images', _kDemo, 900),
        sharedJson('linux images', _kKodi, 1400),
      ];
  });

  // the model asks the server again every two seconds; a tick lets it
  // catch up.
  Future<void> tick(WidgetTester tester) async {
    await tester.pump(kRefreshEvery);
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

  Future<void> pumpWindow(WidgetTester tester) async {
    tester.view.physicalSize = _kSurface;
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final messenger = tester.binding.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') {
        clipboard = (call.arguments as Map)['text'] as String;
      }
      return call.method == 'Clipboard.getData' ? {'text': clipboard} : null;
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(SystemChannels.platform, null),
    );
    final history = LogHistory();
    addTearDown(history.dispose);
    final Desktop desktop = (
      pickFolder: () async => toPick,
      openFolder: (folder) async => opened.add(folder),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: _Window(core: core, history: history, desktop: desktop),
      ),
    );
    await tester.pump();
  }

  Finder keyed(String key) => find.byKey(Key(key));

  // the widget with [key] inside the card with the key [card].
  Finder within(String card, String key) =>
      find.descendant(of: keyed(card), matching: keyed(key));

  String textOf(WidgetTester tester, Finder finder) =>
      tester.widget<Text>(finder).data!;

  // an open panel has a shade over the page: what is on the page is
  // pressed with the panel out of the way, as a user would have to.
  Future<void> press(WidgetTester tester, Finder button) async {
    final inPanel = find.descendant(of: panel, matching: button);
    if (inPanel.evaluate().isEmpty) await closePanel(tester);
    await tester.tap(button);
    await tick(tester);
  }

  Future<void> show(WidgetTester tester, String side) =>
      press(tester, keyed('side-$side'));

  Future<void> enter(WidgetTester tester, String field, String text) async {
    await tester.enterText(keyed(field), text);
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tick(tester);
  }

  Json lastAsked(String method) => state.paramsOf(method).last;

  testWidgets('the switch moves between the four sides, with the log bar '
      'under each', (tester) async {
    await pumpWindow(tester);
    final places = [
      for (final side in _kSideOrder) tester.getCenter(keyed('side-$side')).dx,
    ];
    expect(places, [...places]..sort());
    expect(keyed('search-input'), findsOneWidget);

    const marks = {
      'search': 'search-input',
      'discover': 'library-link-input',
      'downloads': 'token-input',
      'shared': 'new-library-name',
    };
    for (final side in _kSideOrder) {
      await show(tester, side);
      for (final MapEntry(key: other, value: mark) in marks.entries) {
        expect(keyed(mark), other == side ? findsOneWidget : findsNothing);
      }
      expect(find.byType(LogBar), findsOneWidget);
      expect(tester.getBottomLeft(find.byType(LogBar)).dy, _kSurface.height);
    }
  });

  testWidgets('the connection chip and the log follow the server', (
    tester,
  ) async {
    state.ready = false;
    await pumpWindow(tester);
    expect(find.text('connecting'), findsOneWidget);

    state.ready = true;
    Logger('mudita.server').info('serving 2 files');
    await tick(tester);
    expect(find.text('connected'), findsOneWidget);
    expect(textOf(tester, keyed('log-latest')), endsWith('serving 2 files'));

    state.failing = {'status': 'the server went away'};
    await tick(tester);
    expect(find.text('the server went away'), findsOneWidget);
  });

  group('search', () {
    final hit = {
      ...fileJson('behead-kodi.img.gz', 1400, 1),
      'token': 'VLD0:kodi:key',
      'sources': 2,
      'names': ['behead-kodi.img.gz', 'kodi-rpi4.img.gz'],
    };
    final card = 'hit-${hit['root']}';

    Future<void> search(WidgetTester tester) async {
      await tester.enterText(keyed('search-input'), 'kodi');
      await press(tester, keyed('search'));
      await tester.pump(kSearchPollEvery);
    }

    testWidgets('fills in as libraries answer, a card for each file', (
      tester,
    ) async {
      state.polls = [
        {'asked': 1, 'done': false, 'results': <Json>[]},
        {
          'asked': 2,
          'done': true,
          'results': [
            hit,
            {
              ...fileJson('kodi-skin.zip', 12, 2),
              'token': 'VLD0:s:k',
              'sources': 1,
            },
          ],
        },
      ];
      await pumpWindow(tester);
      await search(tester);

      expect(lastAsked('search.start'), {'query': 'kodi'});
      expect(state.paramsOf('search.poll'), hasLength(2));
      expect(find.text('behead-kodi.img.gz'), findsOneWidget);
      expect(find.text('kodi-skin.zip'), findsOneWidget);
      expect(
        textOf(tester, within(card, 'hit-caption')),
        '1.4 KiB, in 2 libraries, also known as kodi-rpi4.img.gz',
      );
    });

    testWidgets('a file found is downloaded, and then shown in its folder', (
      tester,
    ) async {
      state.polls = [
        {
          'asked': 1,
          'done': true,
          'results': [hit],
        },
      ];
      await pumpWindow(tester);
      await search(tester);
      expect(within(card, 'show-in-folder'), findsNothing);

      await press(tester, within(card, 'download'));
      expect(lastAsked('download'), {
        'file': fileJson('behead-kodi.img.gz', 1400, 1),
        'name': 'behead-kodi.img.gz',
      });
      // the panel opens to show it under way.
      expect(
        find.descendant(of: panel, matching: find.text('behead-kodi.img.gz')),
        findsOneWidget,
      );
      expect(textOf(tester, keyed('transfers-label')), '0% of 1 transfer');

      state.finishDownload(1);
      await tick(tester);
      expect(
        textOf(tester, within(card, 'hit-caption')),
        endsWith(', downloaded'),
      );
      expect(within(card, 'download'), findsNothing);
      await press(tester, within(card, 'show-in-folder'));
      expect(opened, [_kDownloads]);
    });

    testWidgets('download pressed twice starts one download, and is not '
        'offered while it runs', (tester) async {
      state.polls = [
        {
          'asked': 1,
          'done': true,
          'results': [hit],
        },
      ];
      await pumpWindow(tester);
      await search(tester);

      // the second press comes before the server has answered the first,
      // and before the screen has been drawn again.
      await tester.tap(within(card, 'download'));
      await tester.tap(within(card, 'download'));
      await tester.pump();
      expect(within(card, 'download'), findsNothing);
      expect(within(card, 'downloading'), findsOneWidget);

      await tick(tester);
      expect(state.paramsOf('download'), hasLength(1));
      expect(within(card, 'download'), findsNothing);
      final running = tester.widget<AppButton>(within(card, 'downloading'));
      expect(running.onPressed, isNull);

      state.finishDownload(1);
      await tick(tester);
      expect(within(card, 'downloading'), findsNothing);
      expect(within(card, 'show-in-folder'), findsOneWidget);
    });

    testWidgets('a download that failed, or was taken off the list, can be '
        'started again', (tester) async {
      state.polls = [
        {
          'asked': 1,
          'done': true,
          'results': [hit],
        },
      ];
      await pumpWindow(tester);
      await search(tester);
      await press(tester, within(card, 'download'));
      state.failDownload(1, 'no route to any peer');
      await tick(tester);

      await press(tester, within(card, 'download'));
      expect(state.paramsOf('download'), hasLength(2));
      expect(within(card, 'downloading'), findsOneWidget);

      // cancelled in the panel, it is offered here once more.
      await openPanel(tester);
      await press(tester, within('download-2', 'cancel'));
      await press(tester, within(card, 'download'));
      expect(state.paramsOf('download'), hasLength(3));
    });

    testWidgets('a file being downloaded is the same file in a library and '
        'behind a pasted link', (tester) async {
      state
        ..polls = [
          {
            'asked': 1,
            'done': true,
            'results': [hit],
          },
        ]
        ..discovered = [
          {'id': 'aa', 'name': 'the shelf', 'seen': null},
        ]
        ..catalogs = {
          'aa': [
            // the library has it under a name of its own.
            {
              ...fileJson('kodi-rpi4.img.gz', 1400, 1),
              'token': 'VLD0:kodi:key',
            },
          ],
        }
        ..links['VLD0:kodi:key'] = {
          'token': 'VLD0:kodi:key',
          'size': 1400,
          'name': 'kodi',
          'names': ['kodi'],
        };
      await pumpWindow(tester);
      await search(tester);
      await press(tester, within(card, 'download'));

      await show(tester, 'discover');
      await press(tester, keyed('library-aa'));
      const row = 'catalog-download-/kodi-rpi4.img.gz';
      expect(keyed(row), findsNothing);
      expect(keyed('catalog-downloading-/kodi-rpi4.img.gz'), findsOneWidget);

      await show(tester, 'downloads');
      await enter(tester, 'token-input', 'VLD0:kodi:key');
      await press(tester, within('incoming-0', 'download'));
      expect(state.paramsOf('download'), hasLength(1));
      expect(keyed('download-1'), findsOneWidget);
    });

    testWidgets('a file found has a link that can be copied', (tester) async {
      state.polls = [
        {
          'asked': 1,
          'done': true,
          'results': [hit],
        },
      ];
      await pumpWindow(tester);
      await search(tester);

      await press(tester, within(card, 'copy-link'));
      expect(
        clipboard,
        linkFor(kHostedBase, 'VLD0:kodi:key', 'behead-kodi.img.gz'),
      );
      expect(find.text('link copied'), findsOneWidget);
    });

    testWidgets('finding nothing, or failing, is said', (tester) async {
      state.polls = [
        {'asked': 3, 'done': true, 'results': <Json>[]},
      ];
      await pumpWindow(tester);
      await search(tester);
      expect(
        textOf(tester, keyed('search-status')),
        'nothing found in 3 libraries',
      );

      state.failing = {'search.start': 'not ready yet'};
      await search(tester);
      expect(textOf(tester, keyed('search-status')), 'not ready yet');
    });
  });

  group('discover', () {
    const shelf = 'library-0011223344556677';
    const unnamed = 'library-8899aabbccddeeff';
    // sorted by folder, as a library lists itself, and two files to a page:
    // the top of the library is the first page and its folders the second.
    final catalog = [
      {...fileJson('notes.txt', 1, 1), 'token': 'VLD0:1:k'},
      {...fileJson('report.pdf', 2, 2), 'token': 'VLD0:2:k'},
      {...fileJson('photo.jpg', 3, 3), 'folder': 'trips', 'token': 'VLD0:3:k'},
      {
        ...fileJson('video.mp4', 4, 4),
        'folder': 'trips/2024',
        'token': 'VLD0:4:k',
      },
    ];
    const video = 'trips/2024/video.mp4';

    Future<void> open(WidgetTester tester) async {
      await pumpWindow(tester);
      await show(tester, 'discover');
      await press(tester, keyed(shelf));
    }

    // the card of the browser, as against the row above it.
    final browserCard = find.ancestor(
      of: keyed('card-head'),
      matching: find.byType(AppCard),
    );

    // what the path row says, part by part, the last being where one is.
    List<String> path(WidgetTester tester) => [
      for (final part
          in find
              .descendant(of: keyed('path'), matching: find.byType(Text))
              .evaluate())
        (part.widget as Text).data!,
    ];

    setUp(() {
      final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
      state
        ..catalogName = 'the shelf'
        ..catalogs = {'0011223344556677': catalog, _kLibraryLink: catalog}
        ..discovered = [
          {'id': '0011223344556677', 'name': 'the shelf', 'seen': now - 120},
          {'id': '8899aabbccddeeff', 'name': null, 'seen': now - 86400},
        ];
    });

    // how many of [icon] there are in the row with the key [row].
    Finder iconsIn(String row, IconData icon) =>
        find.descendant(of: keyed(row), matching: find.byIcon(icon));

    testWidgets('the top level is one card, the libraries its rows, in '
        'three columns', (tester) async {
      state.discovered.first.addAll({'files': 38, 'link': 'https://x/#l'});
      await pumpWindow(tester);
      await show(tester, 'discover');

      expect(find.byType(AppCard), findsNWidgets(2));
      expect(path(tester), ['libraries']);
      expect(keyed('back'), findsNothing);
      for (final heading in ['name', 'last seen', 'files']) {
        expect(find.text(heading), findsOneWidget, reason: heading);
      }
      expect(
        find.descendant(of: keyed(shelf), matching: find.text('the shelf')),
        findsOneWidget,
      );
      expect(
        find.descendant(of: keyed(unnamed), matching: find.text('(no name)')),
        findsOneWidget,
      );
      final recent = tester.widget<Text>(within(shelf, 'library-seen'));
      expect(recent.data, '2 minutes ago');
      expect(recent.style!.color, kColorDim);
      final old = tester.widget<Text>(within(unnamed, 'library-seen'));
      expect(old.data, startsWith('20'));
      expect(old.style!.color, kColorAmber);
      // how many files, when the server says; nothing when it does not.
      String filesOf(String row) =>
          tester.widget<SizeText>(within(row, 'library-files')).text;
      expect(filesOf(shelf), '38');
      expect(filesOf(unnamed), '');
      // a row has no buttons: it is pressed to open the library, and says
      // so with an arrow after the name.
      for (final row in [shelf, unnamed]) {
        expect(
          find.descendant(of: keyed(row), matching: find.byType(AppButton)),
          findsNothing,
        );
        expect(iconsIn(row, Icons.chevron_right), findsOneWidget);
        expect(
          tester.widget<InkWell>(keyed(row)).mouseCursor,
          SystemMouseCursors.click,
        );
      }
      final name = tester.getRect(
        find.descendant(of: keyed(shelf), matching: find.text('the shelf')),
      );
      final arrow = tester.getRect(iconsIn(shelf, Icons.chevron_right));
      expect(arrow.left - name.right, kSpaceIn);
      expect(keyed('copy-link'), findsNothing);
    });

    testWidgets('a row opens the library, whose path the head becomes, and '
        'whose link is there to copy', (tester) async {
      state.discovered.first['link'] = 'https://x/#l';
      await pumpWindow(tester);
      await show(tester, 'discover');
      await press(tester, keyed(shelf));

      expect(path(tester), ['libraries', '/', 'the shelf']);
      expect(keyed(unnamed), findsNothing);
      expect(find.text('last seen'), findsNothing);
      await press(tester, keyed('copy-link'));
      expect(clipboard, 'https://x/#l');
    });

    testWidgets('"refresh" in the head looks for libraries again', (
      tester,
    ) async {
      await pumpWindow(tester);
      await show(tester, 'discover');

      final refresh = find.descendant(
        of: keyed('card-head'),
        matching: find.widgetWithText(AppButton, 'refresh'),
      );
      expect(tester.getSize(refresh), const Size(kActionWidth, kControlHeight));
      // the row above the card only takes a link now.
      expect(
        find.descendant(
          of: find.byType(StartRow),
          matching: find.byType(AppButton),
        ),
        findsOneWidget,
      );
      await press(tester, refresh);
      expect(state.paramsOf('discover.refresh'), hasLength(1));
    });

    testWidgets('with no libraries known, the card says so, once it has '
        'looked', (tester) async {
      state
        ..discovered = []
        ..failing = {'discover': 'not ready yet'};
      await pumpWindow(tester);
      await show(tester, 'discover');
      // the first look has not come back: that is not "none".
      expect(keyed('libraries-loading'), findsOneWidget);
      expect(keyed('libraries-empty'), findsNothing);

      state.failing = {};
      await tick(tester);
      expect(keyed('libraries-loading'), findsNothing);
      expect(
        textOf(tester, keyed('libraries-empty')),
        'no libraries found yet',
      );
      expect(path(tester), ['libraries']);
    });

    testWidgets('rows that open something say so; a file\'s row does not', (
      tester,
    ) async {
      await open(tester);
      expect(
        iconsIn('browse-folder-trips', Icons.chevron_right),
        findsOneWidget,
      );
      expect(
        iconsIn('browse-file-/notes.txt', Icons.chevron_right),
        findsNothing,
      );
      final name = tester.getRect(find.text('trips'));
      final arrow = tester.getRect(
        iconsIn('browse-folder-trips', Icons.chevron_right),
      );
      expect(arrow.left - name.right, kSpaceIn);
      expect(
        tester
            .widget<Icon>(iconsIn('browse-folder-trips', Icons.chevron_right))
            .color,
        kColorDim,
      );
    });

    testWidgets('no part of the path is in the accent colour, at any level', (
      tester,
    ) async {
      await open(tester);
      await press(tester, keyed('browse-folder-trips'));
      await press(tester, keyed('browse-folder-trips/2024'));

      final parts = find.descendant(
        of: keyed('path'),
        matching: find.byType(Text),
      );
      for (final part in tester.widgetList<Text>(parts)) {
        final separator = part.data == '/';
        expect(
          part.style!.copyWith(decoration: TextDecoration.none),
          kTextName.copyWith(
            color: separator ? kColorDim : kColorText,
            decoration: TextDecoration.none,
          ),
          reason: part.data,
        );
      }
      // the parts before the last still lead to their levels.
      for (final link in ['path-libraries', 'path-0', 'path-1']) {
        expect(
          tester.widget<InkWell>(keyed(link)).mouseCursor,
          SystemMouseCursors.click,
        );
      }
      await press(tester, keyed('path-0'));
      expect(path(tester), ['libraries', '/', 'the shelf']);
    });

    testWidgets('the row that takes a link is there at every level, and a '
        'link pasted inside a library opens the pasted one', (tester) async {
      state.catalogs[_kLibraryLink] = [
        {...fileJson('linked.bin', 7, 7), 'token': 'VLD0:7:k'},
      ];
      await pumpWindow(tester);
      await show(tester, 'discover');
      expect(keyed('library-link-input'), findsOneWidget);
      await press(tester, keyed(shelf));
      expect(keyed('library-link-input'), findsOneWidget);
      await press(tester, keyed('browse-folder-trips'));
      expect(keyed('library-link-input'), findsOneWidget);
      // the row is above the card.
      expect(
        tester.getRect(find.byType(StartRow)).bottom,
        lessThan(tester.getRect(keyed('card-head')).top),
      );

      clipboard = libraryLinkFor(kHostedBase, 'VLD0:lib:key');
      await press(tester, keyed('paste'));
      expect(path(tester), hasLength(3));
      expect(find.text('linked.bin'), findsOneWidget);
      expect(find.text('photo.jpg'), findsNothing);
    });

    testWidgets('an opened library shows its top: folders, then files, and '
        'only that level', (tester) async {
      await open(tester);

      // every page was read without a press.
      // once listed, the head has nothing to say of the listing.
      expect(keyed('library-status'), findsNothing);
      expect(keyed('library-loading'), findsNothing);
      expect(find.textContaining('4 files'), findsNothing);
      expect(state.paramsOf('browse'), [
        {'library': '0011223344556677', 'offset': 0},
        {'library': '0011223344556677', 'offset': 2},
      ]);

      // the head of the card is the path, and there is no other way back.
      expect(path(tester), ['libraries', '/', 'the shelf']);
      expect(find.widgetWithText(AppButton, 'back'), findsNothing);
      // the arrow back stands in the column of the pictures below it, so
      // the path starts where the names do.
      expect(
        tester
            .widget<Icon>(
              find.descendant(of: keyed('back'), matching: find.byType(Icon)),
            )
            .icon,
        Icons.chevron_left,
      );
      expect(
        tester.getCenter(keyed('back')).dx,
        tester.getCenter(find.byIcon(Icons.folder_outlined)).dx,
      );
      expect(
        tester.getTopLeft(find.text('libraries')).dx,
        tester.getTopLeft(find.text('notes.txt')).dx,
      );
      // a folder has no button: the row is the way in, and shows a hand
      // over it.
      expect(
        find.descendant(
          of: keyed('browse-folder-trips'),
          matching: find.byType(AppButton),
        ),
        findsNothing,
      );
      expect(
        tester.widget<InkWell>(keyed('browse-folder-trips')).mouseCursor,
        SystemMouseCursors.click,
      );
      expect(find.text('name'), findsOneWidget);
      expect(find.text('size'), findsOneWidget);
      // what is in the folders is not shown beside what is at the top.
      expect(find.text('photo.jpg'), findsNothing);
      expect(find.text('video.mp4'), findsNothing);
      expect(find.text('trips/2024'), findsNothing);
      final rows = [
        for (final name in ['trips', 'notes.txt', 'report.pdf'])
          tester.getTopLeft(find.text(name)).dy,
      ];
      expect(rows, [...rows]..sort());
      // a folder counts the files in the folders beneath it too.
      expect(
        find.descendant(
          of: keyed('browse-folder-trips'),
          matching: find.text('2 files'),
        ),
        findsOneWidget,
      );
      // the server gave no link for this library, so none is offered.
      expect(keyed('copy-link'), findsNothing);
    });

    testWidgets('opening a folder shows what is in it and extends the path', (
      tester,
    ) async {
      await open(tester);
      await press(tester, keyed('browse-folder-trips'));

      expect(path(tester), ['libraries', '/', 'the shelf', '/', 'trips']);
      expect(find.text('notes.txt'), findsNothing);
      expect(find.text('photo.jpg'), findsOneWidget);
      expect(find.text('video.mp4'), findsNothing);
      // a folder is called what it is, not the whole way to it.
      expect(find.text('2024'), findsOneWidget);
      expect(
        find.descendant(
          of: keyed('browse-folder-trips/2024'),
          matching: find.text('1 file'),
        ),
        findsOneWidget,
      );
      expect(
        tester.getTopLeft(find.text('2024')).dy,
        lessThan(tester.getTopLeft(find.text('photo.jpg')).dy),
      );

      await press(tester, keyed('browse-folder-trips/2024'));
      expect(path(tester).sublist(3), ['/', 'trips', '/', '2024']);
      expect(find.text('video.mp4'), findsOneWidget);
      expect(find.text('photo.jpg'), findsNothing);
      expect(keyed('library-status'), findsNothing);
    });

    testWidgets('the head of the card is a bar of its own, as tall as the '
        'bar along the top, over rows a little taller than elsewhere', (
      tester,
    ) async {
      state.discovered.first['link'] = 'https://x/#l';
      await open(tester);
      final card = tester.getRect(browserCard);
      final head = tester.getRect(keyed('card-head'));
      final line = tester.getRect(keyed('card-divider'));

      // the bar and the line under it reach the card's edges.
      expect(head.topLeft, card.topLeft);
      expect(head.width, card.width);
      expect(head.height, kTopBarHeight);
      expect((line.left, line.right), (card.left, card.right));
      expect(line.top, head.bottom);
      expect(tester.widget<Container>(keyed('card-head')).color, kColorHead);
      // what is in the bar has a row's gap above and below, and the card's
      // inset at its sides.
      final copy = tester.getRect(keyed('copy-link'));
      expect(copy.top - head.top, kSpaceRow);
      expect(head.bottom - copy.bottom, kSpaceRow);
      expect(card.right - copy.right, kSpaceRow);
      // "copy link" says so in words, like the buttons of other cards.
      expect(copy.size, const Size(kActionWidth, kControlHeight));
      expect(find.text('copy link'), findsOneWidget);

      // the first column is a button wide in every row: the arrow back,
      // a folder's picture, and a file's button in place of its picture,
      // each in the middle of it.
      final button = tester.getRect(keyed('catalog-download-/notes.txt'));
      final column = card.left + kSpaceRow + kControlHeight / 2;
      expect(button.size, const Size.square(kControlHeight));
      expect(button.left - card.left, kSpaceRow);
      expect(tester.getCenter(keyed('back')).dx, column);
      expect(tester.getCenter(find.byIcon(Icons.folder_outlined)).dx, column);
      expect(find.byIcon(Icons.insert_drive_file_outlined), findsNothing);
      // a file's button is its picture alone, and says what it does when
      // pointed at; a folder's picture is the grey of everything dim.
      expect(
        find.descendant(
          of: browserCard,
          matching: find.textContaining('download'),
        ),
        findsNothing,
      );
      expect(find.byTooltip('download'), findsNWidgets(2));
      expect(
        tester.widget<Icon>(find.byIcon(Icons.folder_outlined)).color,
        kColorDim,
      );
      // sizes, a folder's count among them, end at the card's inset, under
      // the end of "copy link": nothing stands to their right.
      for (final size in ['size', '2 files', '1 B', '2 B']) {
        expect(tester.getRect(find.text(size)).right, copy.right, reason: size);
      }
      // the headings come a row's gap under the line.
      expect(tester.getRect(find.text('name')).top - line.bottom, kSpaceRow);
      expect(
        tester.getRect(find.text('name')).left,
        tester.getRect(find.text('notes.txt')).left,
      );

      // rows of the listing are as tall as the switch, a row's gap apart,
      // with their buttons the usual height in the middle.
      final folder = tester.getRect(keyed('browse-folder-trips'));
      final file = tester.getRect(keyed('browse-file-/notes.txt'));
      expect(folder.height, kNavHeight);
      expect(file.height, kNavHeight);
      expect(file.top - folder.bottom, kSpaceRow);
      expect(button.center.dy, file.center.dy);
      // the last row is as far from the card's foot as from its sides.
      final last = tester.getRect(keyed('browse-file-/report.pdf'));
      expect(card.bottom - last.bottom, kSpaceRow);
      expect(card.right - last.right, kSpaceRow);
    });

    testWidgets('the arrow goes one level back, and from the top to the '
        'list', (tester) async {
      await open(tester);
      await press(tester, keyed('browse-folder-trips'));
      await press(tester, keyed('browse-folder-trips/2024'));
      expect(find.byTooltip('back'), findsOneWidget);

      await press(tester, keyed('back'));
      expect(path(tester).last, 'trips');
      expect(find.text('photo.jpg'), findsOneWidget);

      await press(tester, keyed('back'));
      expect(path(tester).last, 'the shelf');
      expect(find.text('notes.txt'), findsOneWidget);

      await press(tester, keyed('back'));
      expect(path(tester), ['libraries']);
      expect(keyed(shelf), findsOneWidget);
      // there is nowhere further back.
      expect(keyed('back'), findsNothing);
    });

    testWidgets('the top level has the same bar, insets and columns as a '
        'library', (tester) async {
      state.discovered.first['files'] = 38;
      await pumpWindow(tester);
      await show(tester, 'discover');
      final card = tester.getRect(browserCard);
      final head = tester.getRect(keyed('card-head'));
      final line = tester.getRect(keyed('card-divider'));
      final refresh = tester.getRect(keyed('discover-refresh'));

      expect(head.topLeft, card.topLeft);
      expect(head.width, card.width);
      expect(head.height, kTopBarHeight);
      expect((line.left, line.right), (card.left, card.right));
      expect(tester.widget<Container>(keyed('card-head')).color, kColorHead);
      expect(refresh.top - head.top, kSpaceRow);
      expect(head.bottom - refresh.bottom, kSpaceRow);
      expect(card.right - refresh.right, kSpaceRow);

      // the first column is where it is inside a library: a button wide,
      // with the pictures in the middle of it.
      final column = card.left + kSpaceRow + kControlHeight / 2;
      final pictures = find.descendant(
        of: browserCard,
        matching: find.byIcon(Icons.local_library_outlined),
      );
      expect(pictures, findsNWidgets(3));
      for (final picture in pictures.evaluate()) {
        final box = picture.renderObject! as RenderBox;
        expect(box.localToGlobal(box.size.center(Offset.zero)).dx, column);
      }
      // the columns: names under "name" and under the path, when seen
      // under "last seen", counts under "files", ending at the inset.
      final name = find.descendant(
        of: keyed(shelf),
        matching: find.text('the shelf'),
      );
      expect(tester.getRect(name).left, tester.getRect(find.text('name')).left);
      expect(
        tester.getRect(name).left,
        tester.getRect(find.text('libraries')).left,
      );
      final seen = tester.getRect(find.text('last seen'));
      expect(tester.getRect(within(shelf, 'library-seen')).left, seen.left);
      final files = tester.getRect(find.text('files'));
      expect(files.right, refresh.right);
      expect(tester.getRect(find.text('38')).right, refresh.right);
      expect(files.right - kSizeWidth - kSpaceRow - seen.left, 210);
      expect(tester.getRect(find.text('name')).top - line.bottom, kSpaceRow);

      // rows are as tall and as far apart as inside a library.
      final first = tester.getRect(keyed(shelf));
      final last = tester.getRect(keyed(unnamed));
      expect(first.height, kNavHeight);
      expect(last.top - first.bottom, kSpaceRow);
      expect(card.bottom - last.bottom, kSpaceRow);
    });

    testWidgets('a part of the path goes back to that level, and '
        '"libraries" to the list', (tester) async {
      await open(tester);
      await press(tester, keyed('browse-folder-trips'));
      await press(tester, keyed('browse-folder-trips/2024'));
      // where one is, is no link.
      expect(keyed('path-2'), findsNothing);

      await press(tester, keyed('path-1'));
      expect(path(tester).last, 'trips');
      expect(find.text('photo.jpg'), findsOneWidget);

      await press(tester, keyed('browse-folder-trips/2024'));
      await press(tester, keyed('path-0'));
      expect(path(tester).last, 'the shelf');
      expect(find.text('notes.txt'), findsOneWidget);

      await press(tester, keyed('path-libraries'));
      expect(path(tester), ['libraries']);
      expect(keyed(shelf), findsOneWidget);
    });

    testWidgets('a library opened again starts at its top', (tester) async {
      await open(tester);
      await press(tester, keyed('browse-folder-trips'));
      await press(tester, keyed('path-libraries'));
      await press(tester, keyed(shelf));

      expect(path(tester).last, 'the shelf');
      expect(find.text('notes.txt'), findsOneWidget);
    });

    testWidgets('the place in a library is kept on coming back to the side', (
      tester,
    ) async {
      await open(tester);
      await press(tester, keyed('browse-folder-trips'));
      await show(tester, 'search');
      await show(tester, 'discover');

      expect(path(tester).last, 'trips');
      expect(find.text('photo.jpg'), findsOneWidget);
      expect(state.paramsOf('browse'), hasLength(2));
    });

    testWidgets('a folder shows when the page that names it arrives, and a '
        'page that fails is carried on from', (tester) async {
      state
        ..failing = {'browse': 'that library is offline'}
        ..browseFailsFrom = 2;
      await open(tester);

      final failed = tester.widget<Text>(keyed('library-error'));
      expect(failed.data, contains('that library is offline'));
      expect(failed.style!.color, kColorRed);
      expect(find.text('report.pdf'), findsOneWidget);
      expect(keyed('browse-folder-trips'), findsNothing);

      state.failing = {};
      await press(tester, keyed('library-refresh'));
      expect(keyed('library-status'), findsNothing);
      expect(keyed('library-refresh'), findsNothing);
      expect(find.text('report.pdf'), findsOneWidget);
      expect(keyed('browse-folder-trips'), findsOneWidget);
      expect(
        [for (final asked in state.paramsOf('browse')) asked['offset']],
        [0, 2, 2],
      );
    });

    testWidgets('opened before the network is there, it waits and then '
        'lists', (tester) async {
      state.ready = false;
      await open(tester);
      expect(keyed('library-error'), findsNothing);
      expect(keyed('library-loading'), findsOneWidget);
      expect(textOf(tester, keyed('library-status')), 'looking up the library');
      expect(path(tester), ['libraries', '/', 'the shelf']);

      state.ready = true;
      await tick(tester);
      expect(keyed('library-status'), findsNothing);
      expect(keyed('library-loading'), findsNothing);
    });

    testWidgets('a library the server gives a link for has it to copy', (
      tester,
    ) async {
      state.discovered.first['link'] = 'https://x/#l';
      await open(tester);

      await press(tester, keyed('copy-link'));
      expect(clipboard, 'https://x/#l');
    });

    testWidgets('download pressed twice in a folder starts one download, '
        'and a finished one is shown in its folder', (tester) async {
      await open(tester);
      await press(tester, keyed('browse-folder-trips'));
      await press(tester, keyed('browse-folder-trips/2024'));
      const row = 'catalog-download-$video';
      const running = 'catalog-downloading-$video';

      await tester.tap(keyed(row));
      await tester.tap(keyed(row));
      await tester.pump();
      expect(keyed(row), findsNothing);
      expect(tester.widget<AppButton>(keyed(running)).onPressed, isNull);

      await tick(tester);
      expect(state.paramsOf('download'), [
        {'file': fileJson('video.mp4', 4, 4), 'name': 'video.mp4'},
      ]);
      // the files of the other levels are still to be had.
      await press(tester, keyed('path-0'));
      expect(keyed('catalog-download-/notes.txt'), findsOneWidget);
      await press(tester, keyed('browse-folder-trips'));
      await press(tester, keyed('browse-folder-trips/2024'));

      state.finishDownload(1);
      await tick(tester);
      expect(keyed(running), findsNothing);
      await press(tester, keyed('catalog-show-in-folder-$video'));
      expect(opened, [_kDownloads]);
    });

    testWidgets('opens a library by a pasted link', (tester) async {
      await pumpWindow(tester);
      await show(tester, 'discover');
      clipboard = libraryLinkFor(kHostedBase, 'VLD0:lib:key');
      await press(tester, keyed('paste'));

      // the link goes to the server as it came, which reads it either way.
      expect(lastAsked('browse'), {
        'link': libraryLinkFromInput(clipboard),
        'offset': 2,
      });
      expect(keyed('library-status'), findsNothing);
      expect(path(tester), ['libraries', '/', 'the shelf']);
    });

    testWidgets('a library that cannot be read says why, and can be tried '
        'again', (tester) async {
      state.failing = {'browse': 'that library is offline'};
      await pumpWindow(tester);
      await show(tester, 'discover');
      await enter(tester, 'library-link-input', _kLibraryLink);
      expect(textOf(tester, keyed('library-error')), contains('offline'));
      // until it says what it is called, it is called a library.
      expect(path(tester).last, 'library');

      state.failing = {};
      await press(tester, keyed('library-refresh'));
      expect(keyed('library-status'), findsNothing);
      expect(path(tester).last, 'the shelf');
    });

    testWidgets('what is no library link is not opened', (tester) async {
      await pumpWindow(tester);
      await show(tester, 'discover');
      await enter(tester, 'library-link-input', 'VLD0:abc:def/notes.txt');

      expect(find.text('paste a library link'), findsOneWidget);
      expect(state.paramsOf('browse'), isEmpty);
      expect(state.paramsOf('download'), isEmpty);
    });
  });

  group('shared', () {
    const images = 'own-linux images';
    const downloads = 'own-downloads';

    Future<void> pumpShared(WidgetTester tester) async {
      await pumpWindow(tester);
      await show(tester, 'shared');
    }

    Json upload(
      String id,
      String name, {
      String? library = 'linux images',
      int done = 4,
      String state = 'downloading',
    }) {
      final at = DateTime(2026, 10, 5, 14, 2).millisecondsSinceEpoch ~/ 1000;
      return {
        'id': id,
        'name': name,
        'library': library,
        'done': done,
        'total': 10,
        'size': 1400,
        'state': state,
        'started': at,
        'seen': at + 60,
      };
    }

    testWidgets('a server that knows nothing of uploads shows none, and no '
        'trouble', (tester) async {
      expect(state.uploads, isNull);
      await pumpShared(tester);

      expect(state.paramsOf('uploads'), isNotEmpty);
      expect(textOf(tester, keyed('transfers-label')), 'no transfers');
      expect(find.text('connected'), findsOneWidget);
      await openPanel(tester);
      expect(textOf(tester, keyed('transfers-empty')), 'no transfers yet');
      expect(
        textOf(tester, within(images, 'own-caption')),
        '2 files, 2.2 KiB, discoverable',
      );
      expect(keyed('own-downloading'), findsNothing);
    });

    testWidgets('who is downloading is in the panel, and counted on the '
        'library they download from', (tester) async {
      state.uploads = [
        upload('3f9a1c0e7b2d4a68', 'kodi.img.gz'),
        upload('b71c4d0200000000', 'kodi.img.gz', done: 1),
        upload(
          '08e2000000000000',
          'beach.jpg',
          library: null,
          state: 'finished',
        ),
        upload('77d0000000000000', 'phosh.img.gz', done: 6, state: 'gone'),
      ];
      await pumpShared(tester);

      // nothing of it is on the page: the libraries follow the row that
      // starts one.
      expect(find.byType(TransferRow), findsNothing);
      expect(textOf(tester, keyed('transfers-label')), '25% of 2 transfers');
      expect(
        tester.getRect(keyed('transfers-fill')).width,
        moreOrLessEquals(kTransfersButtonWidth * 0.25, epsilon: 1.5),
      );

      // a library says how many transfers it has, in the accent.
      final note = tester.widget<Text>(within(images, 'own-downloading'));
      expect(note.data, '2 transfers');
      expect(note.style!.color, kColorAccent);
      expect(
        textOf(tester, within(images, 'own-caption')),
        '2 files, 2.2 KiB, discoverable',
      );
      expect(within(downloads, 'own-downloading'), findsNothing);

      await openPanel(tester);
      expect(textOf(tester, keyed('transfers-title')), '2 transfers');
      expect(textOf(tester, keyed('transfers-percent')), '25%');
      expect(find.text('sharing'), findsOneWidget);
      expect(find.text('downloading'), findsNothing);
      const running = 'upload-3f9a1c0e7b2d4a68';
      const finished = 'upload-08e2000000000000';
      const gone = 'upload-77d0000000000000';
      expect(
        textOf(tester, within(running, 'upload-caption')),
        'linux images, downloader 3f9a, since 14:02',
      );
      expect(textOf(tester, within(running, 'transfer-end')), '40%');
      expect(
        find.descendant(of: keyed(running), matching: find.byType(AppButton)),
        findsNothing,
      );
      // one in no library says so by leaving the library out.
      expect(
        textOf(tester, within(finished, 'upload-caption')),
        'downloader 08e2, finished at 14:03',
      );
      expect(textOf(tester, within(finished, 'transfer-end')), 'done');
      expect(
        textOf(tester, within(gone, 'upload-caption')),
        'linux images, downloader 77d0, went away at 14:03',
      );
      expect(textOf(tester, within(gone, 'transfer-end')), '60%');
      expect(tester.widget<TransferRow>(keyed(gone)).dimmed, isTrue);
    });

    testWidgets('one transfer from a library is "1 transfer"', (tester) async {
      state.uploads = [upload('3f9a1c0e7b2d4a68', 'kodi.img.gz')];
      await pumpShared(tester);

      expect(textOf(tester, within(images, 'own-downloading')), '1 transfer');
    });

    testWidgets('the pencil stands directly left of what follows it, with '
        'no gap where "stop sharing" is not offered', (tester) async {
      await pumpShared(tester);

      Rect at(String card, String key) => tester.getRect(within(card, key));
      expect(
        at(images, 'stop-sharing').left - at(images, 'rename').right,
        kSpaceRow,
      );
      // the library downloads go to cannot be stopped; its pencil moves up
      // to "copy link".
      expect(within(downloads, 'stop-sharing'), findsNothing);
      expect(
        at(downloads, 'copy-library-link').left - at(downloads, 'rename').right,
        kSpaceRow,
      );
    });

    testWidgets('lists the libraries, and never offers to stop sharing the '
        'one downloads go to', (tester) async {
      await pumpShared(tester);

      expect(
        textOf(tester, within(images, 'own-caption')),
        '2 files, 2.2 KiB, discoverable',
      );
      expect(
        textOf(tester, within(downloads, 'own-caption')),
        '0 files, by link only',
      );
      expect(within(images, 'stop-sharing'), findsOneWidget);
      expect(within(downloads, 'stop-sharing'), findsNothing);
      expect(within(downloads, 'copy-library-link'), findsOneWidget);
      // closed, a card shows neither its folders nor its files.
      expect(find.text(_kImages), findsNothing);
    });

    testWidgets('opens one library at a time', (tester) async {
      await pumpShared(tester);
      await press(tester, keyed('toggle-linux images'));
      expect(within(images, 'discoverable'), findsOneWidget);
      expect(find.text(_kImages), findsOneWidget);
      expect(find.text('kodi.img.gz'), findsOneWidget);

      await press(tester, keyed('toggle-downloads'));
      expect(within(images, 'discoverable'), findsNothing);
      expect(within(downloads, 'discoverable'), findsOneWidget);
      // the folder downloads go to cannot be removed.
      expect(keyed('remove-folder-$_kDownloads'), findsNothing);

      await press(tester, keyed('toggle-downloads'));
      expect(keyed('discoverable'), findsNothing);
    });

    testWidgets('toggles whether a library is discoverable', (tester) async {
      await pumpShared(tester);
      await press(tester, keyed('toggle-linux images'));
      await press(tester, within(images, 'discoverable'));

      expect(lastAsked('library.set'), {
        'name': 'linux images',
        'listed': false,
      });
      expect(
        textOf(tester, within(images, 'own-caption')),
        endsWith('by link only'),
      );
    });

    testWidgets('renames a library where its name stands, and keeps it open', (
      tester,
    ) async {
      await pumpShared(tester);
      await press(tester, keyed('toggle-linux images'));
      await press(tester, keyed('folder-$_kImages/demo'));
      await press(tester, within(images, 'rename'));
      await enter(tester, 'rename-input', 'images');

      expect(lastAsked('library.rename'), {
        'name': 'linux images',
        'to': 'images',
      });
      expect(keyed('rename-input'), findsNothing);
      expect(within('own-images', 'discoverable'), findsOneWidget);
      expect(keyed('file-$_kDemo'), findsOneWidget);
    });

    testWidgets('a rename left alone, or to the same name, asks nothing', (
      tester,
    ) async {
      await pumpShared(tester);
      await press(tester, within(images, 'rename'));
      await enter(tester, 'rename-input', 'linux images');
      expect(keyed('rename-input'), findsNothing);

      await press(tester, within(images, 'rename'));
      await press(tester, keyed('side-shared'));
      expect(keyed('rename-input'), findsNothing);
      expect(state.paramsOf('library.rename'), isEmpty);
    });

    testWidgets('adds and removes a folder of a library', (tester) async {
      await pumpShared(tester);
      await press(tester, keyed('toggle-linux images'));

      toPick = '/home/me/more';
      await press(tester, within(images, 'add-folder'));
      expect(lastAsked('folders.add'), {
        'path': '/home/me/more',
        'library': 'linux images',
      });
      expect(find.text('/home/me/more'), findsOneWidget);
      // one button adds a folder, on the first folder's row.
      expect(within(images, 'add-folder'), findsOneWidget);

      await press(tester, keyed('remove-folder-/home/me/more'));
      expect(lastAsked('folders.remove'), {
        'path': '/home/me/more',
        'library': 'linux images',
      });
      expect(find.text('/home/me/more'), findsNothing);
    });

    testWidgets('asks before a library stops being shared', (tester) async {
      await pumpShared(tester);
      await press(tester, within(images, 'stop-sharing'));
      expect(textOf(tester, keyed('confirm-text')), contains('"linux images"'));

      await press(tester, keyed('confirm-cancel'));
      expect(state.paramsOf('library.remove'), isEmpty);
      expect(keyed(images), findsOneWidget);

      await press(tester, within(images, 'stop-sharing'));
      await press(tester, keyed('confirm-stop'));
      expect(lastAsked('library.remove'), {'name': 'linux images'});
      expect(keyed(images), findsNothing);
    });

    testWidgets('copies a library link and a file link', (tester) async {
      await pumpShared(tester);
      await press(tester, within(images, 'copy-library-link'));
      expect(clipboard, libraryLinkFor(kHostedBase, 'VLD0:images:key'));
      expect(find.text('library link copied'), findsOneWidget);

      await press(tester, keyed('toggle-linux images'));
      await press(tester, keyed('copy-link-$_kKodi'));
      // a file's link has its name in it.
      expect(
        clipboard,
        linkFor(kHostedBase, 'VLD0:kodi.img.gz:key', 'kodi.img.gz'),
      );
    });

    testWidgets('a library not published yet has no link to copy', (
      tester,
    ) async {
      state.libraries.last['token'] = null;
      await pumpShared(tester);

      final button = tester.widget<AppButton>(
        within(images, 'copy-library-link'),
      );
      expect(button.onPressed, isNull);
    });

    testWidgets('opens and closes a folder in the tree', (tester) async {
      await pumpShared(tester);
      await press(tester, keyed('toggle-linux images'));
      const folder = 'folder-$_kImages/demo';
      expect(
        find.descendant(of: keyed(folder), matching: find.text('1 file')),
        findsOneWidget,
      );
      expect(keyed('file-$_kDemo'), findsNothing);
      // a folder comes before the files beside it.
      expect(
        tester.getTopLeft(keyed(folder)).dy,
        lessThan(tester.getTopLeft(keyed('file-$_kKodi')).dy),
      );

      await press(tester, keyed(folder));
      expect(keyed('file-$_kDemo'), findsOneWidget);
      // what is in a folder is set in from it.
      expect(
        tester.getTopLeft(find.text('behead-demo.img.gz')).dx,
        greaterThan(tester.getTopLeft(find.text('demo')).dx),
      );

      await press(tester, keyed(folder));
      expect(keyed('file-$_kDemo'), findsNothing);
    });

    testWidgets('a file still being hashed says so in place of its link', (
      tester,
    ) async {
      state.files.last.addAll({
        'state': 'hashing',
        'progress': 0.4,
        'token': null,
      });
      await pumpShared(tester);
      await press(tester, keyed('toggle-linux images'));

      expect(keyed('copy-link-$_kKodi'), findsNothing);
      expect(find.text('hashing 40%'), findsOneWidget);
    });

    testWidgets('a new library is named and given its first folder', (
      tester,
    ) async {
      await pumpShared(tester);
      toPick = '/home/me/music';
      await tester.enterText(keyed('new-library-name'), 'tunes');
      await press(tester, keyed('choose-folder'));

      expect(lastAsked('library.add'), {'name': 'tunes'});
      expect(lastAsked('folders.add'), {
        'path': '/home/me/music',
        'library': 'tunes',
      });
      expect(keyed('own-tunes'), findsOneWidget);
      final field = tester.widget<TextField>(keyed('new-library-name'));
      expect(field.controller!.text, '');
    });

    testWidgets('a new library with no name is called what its folder is', (
      tester,
    ) async {
      await pumpShared(tester);
      toPick = '/home/me/music';
      await press(tester, keyed('choose-folder'));

      expect(state.paramsOf('library.add'), isEmpty);
      expect(lastAsked('folders.add'), {'path': '/home/me/music'});
      expect(keyed('own-music'), findsOneWidget);
    });

    testWidgets('backing out of choosing a folder changes nothing', (
      tester,
    ) async {
      await pumpShared(tester);
      await tester.enterText(keyed('new-library-name'), 'tunes');
      await press(tester, keyed('choose-folder'));

      expect(state.paramsOf('library.add'), isEmpty);
      expect(state.paramsOf('folders.add'), isEmpty);
    });

    testWidgets('the folders are looked over again when asked', (tester) async {
      await pumpShared(tester);
      await press(tester, keyed('rescan'));
      expect(state.paramsOf('rescan'), hasLength(1));
    });
  });

  group('downloads', () {
    // newest first, as the server lists them: one being looked up, one
    // running, one saved, and one that failed, none of them started here.
    setUp(() {
      state.links[Uri.decodeComponent(_kFileLink)] = {
        'token': 'VLD0:abc:def',
        'size': 65000,
        'name': 'notes.txt',
        'names': ['notes.txt'],
      };
      state.downloads = [
        {'id': 14, 'name': null, 'size': null, 'state': 'resolving', 'done': 0},
        {
          'id': 13,
          'name': 'ubuntu.iso',
          'size': 1400,
          'state': 'downloading',
          'done': 1,
          'total': 3,
        },
        {
          'id': 12,
          'name': 'kodi-skin.zip',
          'size': 12,
          'state': 'done',
          'done': 3,
          'total': 3,
          'saved': '$_kDownloads/kodi-skin.zip',
        },
        {
          'id': 11,
          'name': 'old-backup.tar',
          'size': 5,
          'state': 'error',
          'error': 'nobody is sharing this file right now',
        },
      ];
    });

    Future<void> pumpDownloads(WidgetTester tester) async {
      await pumpWindow(tester);
      await show(tester, 'downloads');
    }

    testWidgets('a pasted file link is looked up and waits, and is '
        'downloaded on request', (tester) async {
      state.downloads = [];
      await pumpDownloads(tester);
      clipboard = linkFor(kHostedBase, 'VLD0:abc:def', 'notes.txt');
      await press(tester, keyed('paste'));

      expect(lastAsked('lookup'), {'link': _kFileLink});
      expect(textOf(tester, keyed('download-name')), 'notes.txt');
      expect(textOf(tester, keyed('download-ready')), contains('KiB'));
      // it waits on the page, in a card under the link row. nothing is
      // downloaded that the user has not asked for, and pasting opens
      // nothing.
      expect(within('pending', 'incoming-0'), findsOneWidget);
      expect(
        tester.getTopLeft(keyed('pending')).dy,
        greaterThan(tester.getBottomLeft(find.byType(AddLinkRow)).dy),
      );
      expect(state.paramsOf('download'), isEmpty);
      expect(panel, findsNothing);
      expect(textOf(tester, keyed('transfers-label')), 'no transfers');

      await press(tester, within('incoming-0', 'download'));
      expect(lastAsked('download'), {
        'token': 'VLD0:abc:def',
        'name': 'notes.txt',
      });
      // the file that waited is now one of the library's downloads: it
      // leaves the page, and the panel opens to show it under way.
      expect(keyed('pending'), findsNothing);
      expect(keyed('incoming-0'), findsNothing);
      expect(panel, findsOneWidget);
      expect(find.text('downloading'), findsOneWidget);
      expect(within('download-1', 'cancel'), findsOneWidget);
      expect(textOf(tester, keyed('transfers-title')), '1 transfer');
      // the server has not found the file yet, so there is no telling how
      // far along it is.
      expect(keyed('transfers-percent'), findsNothing);
      // the panel never offers to start one.
      expect(keyed('download'), findsNothing);
    });

    testWidgets('a link that leads nowhere says why on the page', (
      tester,
    ) async {
      state
        ..downloads = []
        ..failing = {'lookup': 'nothing is shared at that link'};
      await pumpDownloads(tester);
      await enter(tester, 'token-input', 'VLD0:nothing:here');

      final reason = tester.widget<Text>(within('pending', 'download-error'));
      expect(reason.data, 'nothing is shared at that link');
      expect(reason.style!.color, kColorRed);
      expect(panel, findsNothing);
      await press(tester, within('pending', 'dismiss'));
      expect(keyed('pending'), findsNothing);
    });

    testWidgets('a bare token offers the names the file is known by', (
      tester,
    ) async {
      state
        ..downloads = []
        ..links['VLD0:abc:def'] = {
          'token': 'VLD0:abc:def',
          'size': 65000,
          'name': 'notes.txt',
          'names': ['notes.txt', 'minutes.txt'],
        };
      await pumpDownloads(tester);
      await enter(tester, 'token-input', 'VLD0:abc:def');

      await press(tester, keyed('download-name-minutes.txt'));
      await press(tester, within('incoming-0', 'download'));
      expect(lastAsked('download'), {
        'token': 'VLD0:abc:def',
        'name': 'minutes.txt',
      });
    });

    testWidgets('a waiting file is dismissed without the server hearing of '
        'it', (tester) async {
      state.downloads = [];
      await pumpDownloads(tester);
      await enter(tester, 'token-input', _kFileLink);
      await press(tester, within('incoming-0', 'dismiss'));

      expect(keyed('incoming-0'), findsNothing);
      expect(state.paramsOf('download'), isEmpty);
      expect(state.paramsOf('download.cancel'), isEmpty);
    });

    testWidgets('a link pasted twice and asked for twice is one download', (
      tester,
    ) async {
      state.downloads = [];
      await pumpDownloads(tester);
      await enter(tester, 'token-input', _kFileLink);
      await press(tester, within('incoming-0', 'download'));
      // the same file again, by its bare token this time.
      await enter(tester, 'token-input', 'VLD0:abc:def/notes.txt');
      await press(tester, keyed('download'));

      expect(state.paramsOf('download'), hasLength(1));
      expect(find.byType(TransferRow), findsOneWidget);
      expect(keyed('download-1'), findsOneWidget);
    });

    testWidgets('the panel is a drawer under the top bar, over every side, '
        'closed by its x or by the shade', (tester) async {
      await pumpDownloads(tester);
      // the downloads are not on the page.
      expect(find.byType(TransferRow), findsNothing);
      expect(find.byType(AppCard), findsOneWidget);
      expect(textOf(tester, keyed('transfers-label')), '33% of 2 transfers');
      expect(
        tester.getRect(keyed('connection')).left -
            tester.getRect(keyed('transfers-button')).right,
        kSpaceRow,
      );

      await openPanel(tester);
      final drawer = tester.getRect(panel);
      expect(drawer.width, kPanelWidth);
      expect(drawer.top, kTopBarHeight);
      expect(drawer.right, _kSurface.width);
      expect(drawer.bottom, _kSurface.height);
      expect(tester.getRect(keyed('transfers-shade')).right, drawer.left);

      await tester.tap(keyed('transfers-shade'));
      await tester.pump();
      expect(panel, findsNothing);
      for (final side in _kSideOrder) {
        await show(tester, side);
        expect(keyed('transfers-button'), findsOneWidget);
      }
    });

    testWidgets('a download seen to start and to finish says when', (
      tester,
    ) async {
      state.downloads = [];
      await pumpDownloads(tester);
      await enter(tester, 'token-input', _kFileLink);
      await press(tester, within('incoming-0', 'download'));
      expect(
        textOf(tester, within('download-1', 'download-running')),
        matches(r'^since \d\d:\d\d$'),
      );
      state.finishDownload(1);
      await tick(tester);

      expect(
        textOf(tester, within('download-1', 'download-done')),
        matches(r'^saved at \d\d:\d\d$'),
      );
      expect(textOf(tester, within('download-1', 'transfer-end')), 'done');
    });

    testWidgets('a library link entered here opens on the discover side', (
      tester,
    ) async {
      state.catalogs = {
        _kLibraryLink: [
          {...fileJson('one.bin', 1, 1), 'folder': 'docs'},
        ],
      };
      await pumpDownloads(tester);
      await enter(tester, 'token-input', _kLibraryLink);

      expect(keyed('token-input'), findsNothing);
      expect(lastAsked('browse'), {'link': _kLibraryLink, 'offset': 0});
      expect(keyed('browse-folder-docs'), findsOneWidget);
      expect(state.paramsOf('lookup'), isEmpty);

      // the same link given again, from inside a folder, is at the top again.
      await press(tester, keyed('browse-folder-docs'));
      expect(find.text('one.bin'), findsOneWidget);
      await show(tester, 'downloads');
      await enter(tester, 'token-input', _kLibraryLink);
      expect(find.text('one.bin'), findsNothing);
      expect(keyed('browse-folder-docs'), findsOneWidget);

      // "libraries" shows the discover side's own list.
      await press(tester, keyed('path-libraries'));
      expect(keyed('discover-refresh'), findsOneWidget);
    });

    testWidgets('each state has its own words and buttons', (tester) async {
      await pumpDownloads(tester);
      await openPanel(tester);

      // two are running: the one being looked up, and the one arriving. the
      // whole is over what is known of them.
      expect(textOf(tester, keyed('transfers-title')), '2 transfers');
      expect(textOf(tester, keyed('transfers-percent')), '33%');
      expect(find.text('downloading'), findsOneWidget);
      expect(find.text('sharing'), findsNothing);

      // one the server is still looking up has no name or size yet.
      expect(
        find.descendant(
          of: keyed('download-14'),
          matching: find.text('download'),
        ),
        findsOneWidget,
      );
      expect(
        tester
            .widget<ProgressBar>(
              find.descendant(
                of: keyed('download-14'),
                matching: find.byType(ProgressBar),
              ),
            )
            .value,
        isNull,
      );
      expect(within('download-14', 'cancel'), findsOneWidget);

      expect(textOf(tester, within('download-13', 'transfer-end')), '33%');
      // one started elsewhere cannot say since when.
      expect(
        textOf(tester, within('download-13', 'download-running')),
        '1.4 KiB',
      );
      expect(
        tester.widget<AppButton>(within('download-13', 'cancel')).kind,
        ButtonKind.danger,
      );
      expect(within('download-13', 'dismiss'), findsNothing);

      // one that had finished before this window looked cannot say when.
      expect(
        textOf(tester, within('download-12', 'download-done')),
        '12 B, saved',
      );
      expect(textOf(tester, within('download-12', 'transfer-end')), 'done');
      expect(
        tester.getSize(within('download-12', 'show-in-folder')),
        const Size.square(kControlHeight),
      );
      expect(find.byTooltip('show in folder'), findsOneWidget);
      expect(
        tester.getSize(within('download-12', 'dismiss')),
        const Size(kPanelButtonWidth, kControlHeight),
      );
      expect(within('download-12', 'cancel'), findsNothing);

      final reason = tester.widget<Text>(
        within('download-11', 'download-error'),
      );
      expect(reason.data, 'nobody is sharing this file right now');
      expect(reason.style!.color, kColorRed);
      expect(within('download-11', 'dismiss'), findsOneWidget);
      // the server does not say what it was asked with, so one started
      // elsewhere cannot be asked for again.
      expect(within('download-11', 'try-again'), findsNothing);

      // what is running comes before what has ended.
      final order = [
        for (final id in [14, 13, 12, 11])
          tester.getTopLeft(keyed('download-$id')).dy,
      ];
      expect(order, [...order]..sort());
    });

    testWidgets('cancel and dismiss take a download off the server', (
      tester,
    ) async {
      await pumpDownloads(tester);
      await openPanel(tester);
      await press(tester, within('download-13', 'cancel'));
      expect(lastAsked('download.cancel'), {'id': 13});
      expect(keyed('download-13'), findsNothing);

      await press(tester, within('download-12', 'dismiss'));
      expect(lastAsked('download.cancel'), {'id': 12});
      expect(keyed('download-12'), findsNothing);
      expect(keyed('download-11'), findsOneWidget);
    });

    testWidgets('a saved download is shown in its folder', (tester) async {
      await pumpDownloads(tester);
      await openPanel(tester);
      await press(tester, within('download-12', 'show-in-folder'));
      expect(opened, [_kDownloads]);
    });

    testWidgets('a download started here that failed is tried again', (
      tester,
    ) async {
      state.downloads = [];
      await pumpDownloads(tester);
      await enter(tester, 'token-input', _kFileLink);
      await press(tester, keyed('download'));
      state.failDownload(1, 'no route to any peer');
      await tick(tester);
      expect(
        textOf(tester, within('download-1', 'download-error')),
        'no route to any peer',
      );
      expect(
        tester.getSize(within('download-1', 'try-again')),
        const Size.square(kControlHeight),
      );
      expect(find.byTooltip('try again'), findsOneWidget);

      await press(tester, within('download-1', 'try-again'));
      expect(lastAsked('download.cancel'), {'id': 1});
      expect(state.paramsOf('download'), hasLength(2));
      expect(state.paramsOf('download').first, state.paramsOf('download').last);
      expect(keyed('download-1'), findsNothing);
      expect(within('download-2', 'cancel'), findsOneWidget);
    });
  });
}
