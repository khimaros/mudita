// the pictures of the library's window in README.md: the window itself,
// drawn over the fake of the server with a made-up library in it, so that
// they can be made again whenever the window changes. this is no test, and
// `make test` passes it by: `make screenshots` runs it.

import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:logging/logging.dart';
import 'package:mudita/core/log_history.dart';
import 'package:mudita/dana/library_model.dart';
import 'package:mudita/dana/ui/home.dart';
import 'package:mudita/ui/app_shell.dart';

import '../fakes/fake_library.dart';

// where the pictures go, from the root of the repository, which is where
// flutter runs this from.
const String _kOut = 'docs/screenshots';

// the window as it is on a desktop, drawn at twice its size so that the
// pictures stay sharp on a dense screen.
const Size _kWindow = Size(1280, 800);
const double _kPixelRatio = 2;

// flutter's tests draw every letter as a box unless given fonts: the ones
// flutter itself ships, and the desktop's own monospace, as the app has.
const String _kFlutterFonts = 'bin/cache/artifacts/material_fonts';
const Map<String, List<String>> _kFonts = {
  'Roboto': ['Roboto-Regular.ttf', 'Roboto-Medium.ttf', 'Roboto-Bold.ttf'],
  'MaterialIcons': ['MaterialIcons-Regular.otf'],
};
const String _kMonoFamily = 'monospace';

const String _kHome = '/home/you';
const String _kImages = '$_kHome/images';
const String _kTalks = '$_kHome/talks';
const String _kDownloads = '$_kHome/Downloads/mudita';
const String _kOwn = 'linux images';
const String _kQuery = 'debian';
const String _kPasted = 'VLD0:k3vW9sQmT1xYh7RzB2nLc8dPaF6uJ0eG:M4oNqXr5tVbZ';

const int _kMiB = 1024 * 1024;
const int _kNetinst = 754 * _kMiB;
const int _kHandbook = 31 * _kMiB;

// how long the window is given to hear from the server and draw again.
const int _kSettles = 6;

const Key _kShot = Key('shot');

Future<void> _loadFonts() async {
  final flutter = Platform.environment['FLUTTER_ROOT']!;
  // asked without flutter's environment, which points fontconfig at
  // flutter's own fonts and away from the desktop's.
  final mono = Process.runSync(
    'fc-match',
    ['--format=%{file}', _kMonoFamily],
    environment: {'HOME': Platform.environment['HOME']!},
    includeParentEnvironment: false,
  ).stdout;
  final families = {
    _kMonoFamily: ['$mono'],
    for (final MapEntry(key: family, value: files) in _kFonts.entries)
      family: [for (final file in files) '$flutter/$_kFlutterFonts/$file'],
  };
  for (final MapEntry(key: family, value: paths) in families.entries) {
    final loader = FontLoader(family);
    for (final path in paths) {
      loader.addFont(
        File(path).readAsBytes().then((bytes) => ByteData.sublistView(bytes)),
      );
    }
    await loader.load();
  }
}

int _secondsAgo(Duration ago) =>
    DateTime.now().subtract(ago).millisecondsSinceEpoch ~/ 1000;

Json _hit(
  String name,
  int size,
  int seed,
  List<String> libraries, [
  List<String>? names,
]) => {
  ...fileJson(name, size, seed),
  'token': 'VLD0:$name:key',
  'sources': libraries.length,
  'libraries': libraries,
  'names': names ?? [name],
};

Json _upload(String id, String name, int done, {String state = 'downloading'}) {
  return {
    'id': id,
    'name': name,
    'library': _kOwn,
    'done': done,
    'total': 100,
    'state': state,
    'started': _secondsAgo(const Duration(minutes: 9)),
    'seen': _secondsAgo(const Duration(minutes: 2)),
  };
}

Json _download(int id, String name, int size, int done, {String? saved}) => {
  'id': id,
  'name': name,
  'size': size,
  'state': saved == null ? 'downloading' : 'done',
  'done': done,
  'total': 100,
  'error': null,
  'saved': saved,
};

/// a server with a few libraries of its own, some of other people's, and
/// transfers both ways.
FakeLibraryCore _staged() {
  final core = FakeLibraryCore();
  core.state
    ..downloadFolder = _kDownloads
    ..libraries = [
      {
        'name': _kOwn,
        'folders': [_kImages],
        'listed': true,
        'token': 'VLD0:images:key',
      },
      {
        'name': 'conference talks',
        'folders': [_kTalks],
        'listed': false,
        'token': 'VLD0:talks:key',
      },
      {
        'name': 'downloads',
        'folders': [_kDownloads],
        'listed': false,
        'token': 'VLD0:downloads:key',
      },
    ]
    ..files = [
      sharedJson(
        _kOwn,
        '$_kImages/debian/debian-13.1.0-amd64-netinst.iso',
        _kNetinst,
      ),
      sharedJson(
        _kOwn,
        '$_kImages/debian/debian-13.1.0-arm64-netinst.iso',
        698 * _kMiB,
      ),
      sharedJson(_kOwn, '$_kImages/debian/SHA256SUMS', 412),
      sharedJson(
        _kOwn,
        '$_kImages/postmarketos/pmos-phosh-pinephone.img.xz',
        1187 * _kMiB,
      ),
      sharedJson(_kOwn, '$_kImages/kodi-rpi4.img.gz', 286 * _kMiB),
      sharedJson(_kOwn, '$_kImages/README.txt', 1904),
      sharedJson(
        'conference talks',
        '$_kTalks/veilid-defcon.webm',
        402 * _kMiB,
      ),
      sharedJson('conference talks', '$_kTalks/slides.pdf', 6 * _kMiB),
      sharedJson('downloads', '$_kDownloads/debian-handbook.pdf', _kHandbook),
    ]
    ..discovered = [
      {
        'id': '0011223344556677',
        'name': 'the shelf',
        'seen': _secondsAgo(const Duration(seconds: 20)),
        'files': 214,
      },
      {
        'id': '2233445566778899',
        'name': 'public domain films',
        'seen': _secondsAgo(const Duration(minutes: 2)),
        'files': 37,
      },
      {
        'id': '445566778899aabb',
        'name': 'open maps',
        'seen': _secondsAgo(const Duration(minutes: 4)),
        'files': 12,
      },
      {
        'id': '66778899aabbccdd',
        'name': 'field recordings',
        'seen': _secondsAgo(const Duration(days: 1)),
      },
      {'id': '8899aabbccddeeff', 'name': null, 'seen': null},
    ]
    ..polls = [
      {
        'asked': 4,
        'done': true,
        'results': [
          _hit(
            'debian-13.1.0-amd64-netinst.iso',
            _kNetinst,
            1,
            ['the shelf', 'open maps'],
            ['debian-13.1.0-amd64-netinst.iso', 'debian-13-netinst.iso'],
          ),
          _hit('debian-13.1.0-arm64-netinst.iso', 698 * _kMiB, 2, [
            'the shelf',
          ]),
          _hit('debian-handbook.pdf', _kHandbook, 3, [
            'the shelf',
            'public domain films',
          ]),
          _hit('debian-live-13.1.0-amd64-gnome.iso', 3482 * _kMiB, 4, [
            'the shelf',
          ]),
        ],
      },
    ]
    ..links[_kPasted] = {
      'token': _kPasted,
      'size': 1264 * _kMiB,
      'name': 'night-of-the-living-dead.mkv',
      'names': ['night-of-the-living-dead.mkv', 'notld-1968.mkv'],
    }
    ..downloads = [
      _download(3, 'debian-live-13.1.0-amd64-gnome.iso', 3482 * _kMiB, 37),
      _download(2, 'osm-europe-extract.pmtiles', 912 * _kMiB, 81),
      _download(
        1,
        'debian-handbook.pdf',
        _kHandbook,
        100,
        saved: '$_kDownloads/debian-handbook.pdf',
      ),
    ]
    ..uploads = [
      _upload('3f9a1c0e7b2d4a68', 'kodi-rpi4.img.gz', 64),
      _upload('b71c4d02e5a9f013', 'debian-13.1.0-amd64-netinst.iso', 12),
      _upload('08e2a7c41d3b9f56', 'README.txt', 100, state: 'finished'),
    ];
  return core;
}

void main() {
  setUpAll(() async {
    await _loadFonts();
    Directory(_kOut).createSync(recursive: true);
  });

  late LibraryModel model;

  Finder keyed(String key) => find.byKey(Key(key));

  Future<void> settle(WidgetTester tester) async {
    for (var i = 0; i < _kSettles; i++) {
      await tester.pump();
    }
  }

  Future<void> press(WidgetTester tester, String key) async {
    await tester.tap(keyed(key));
    await settle(tester);
  }

  // the window as lib/dana/main.dart sets it up, over the staged server,
  // with the log saying what such a server would have.
  Future<void> open(WidgetTester tester) async {
    tester.view.physicalSize = _kWindow;
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final history = LogHistory();
    addTearDown(history.dispose);
    model = LibraryModel(_staged())..start();
    await tester.pumpWidget(
      RepaintBoundary(
        key: _kShot,
        child: MaterialApp(
          debugShowCheckedModeBanner: false,
          themeMode: ThemeMode.dark,
          theme: appTheme(Brightness.light),
          darkTheme: appTheme(Brightness.dark),
          home: LibraryHome(
            model: model,
            history: history,
            desktop: (pickFolder: () async => null, openFolder: (_) async {}),
          ),
        ),
      ),
    );
    Logger('$kLogName.peer')
      ..info('serving 9 files in 3 libraries')
      ..info('download 3f9a1c0e7b2d4a68 started (2 active)');
    await settle(tester);
  }

  // write what the window shows to `dana-<name>.png`, then take the window
  // down, so that the model stops asking before the test ends.
  Future<void> shoot(WidgetTester tester, String name) async {
    final shot = tester.renderObject<RenderRepaintBoundary>(find.byKey(_kShot));
    await tester.runAsync(() async {
      final image = await shot.toImage(pixelRatio: _kPixelRatio);
      final png = await image.toByteData(format: ui.ImageByteFormat.png);
      image.dispose();
      File('$_kOut/dana-$name.png').writeAsBytesSync(png!.buffer.asUint8List());
    });
    await tester.pumpWidget(const SizedBox.shrink());
    model.dispose();
  }

  testWidgets('search', (tester) async {
    await open(tester);
    await tester.enterText(keyed('search-input'), _kQuery);
    await press(tester, 'search');
    await shoot(tester, 'search');
  });

  testWidgets('browse', (tester) async {
    await open(tester);
    await press(tester, 'side-discover');
    await shoot(tester, 'browse');
  });

  testWidgets('download', (tester) async {
    await open(tester);
    await press(tester, 'side-downloads');
    await tester.enterText(keyed('token-input'), _kPasted);
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await settle(tester);
    await shoot(tester, 'download');
  });

  testWidgets('share', (tester) async {
    await open(tester);
    await press(tester, 'side-shared');
    await press(tester, 'toggle-$_kOwn');
    await press(tester, 'folder-$_kImages/debian');
    await shoot(tester, 'share');
  });

  testWidgets('transfers', (tester) async {
    await open(tester);
    await press(tester, 'side-shared');
    await press(tester, 'transfers-button');
    await shoot(tester, 'transfers');
  });
}
