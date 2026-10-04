// the app's side of the control protocol, against a fake server on a real
// unix socket: connecting, the model that follows the server's state, and
// what it asks the server to do. the words the window makes of that state
// are tested here too, being pure.

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:mudita/core/core.dart';
import 'package:mudita/core/core_io.dart';
import 'package:mudita/core/server.dart';
import 'package:mudita/core/types.dart' show RemoteFile;
import 'package:mudita/core/file_tree.dart';
import 'package:mudita/dana/labels.dart';
import 'package:mudita/dana/library_model.dart';

import '../fakes/fake_server.dart';

const String _kFolder = '/home/me/Downloads/mudita';

void main() {
  late FakeServer fake;
  late Server server;
  late LibraryModel model;

  setUp(() async {
    fake = await FakeServer.start();
    server = await Server.start(socketPath: fake.path, binary: '/no/such');
    model = LibraryModel(SocketCore(server));
    addTearDown(() async {
      model.dispose();
      await server.close();
      await fake.stop();
    });
  });

  List<String> asked() => [
    for (final request in fake.state.requests) request['method'] as String,
  ];
  Json paramsOf(String method) => fake.state.paramsOf(method).last;

  test('a server already running is used, not started again', () {
    // the binary given does not exist, so connecting proves none was run.
    expect(server, isNotNull);
  });

  test('with no server and no binary to start, connecting fails', () async {
    final nowhere = '${Directory.systemTemp.path}/mudita-none.sock';
    await expectLater(
      Server.start(socketPath: nowhere, binary: '/no/such'),
      throwsA(isA<ProcessException>()),
    );
  });

  test('the model shows what the server reports', () async {
    fake.state
      ..files = [
        sharedJson('downloads', '$_kFolder/a.bin', 10),
        {
          ...sharedJson('downloads', '$_kFolder/b.bin', 20),
          'state': 'hashing',
          'progress': 0.5,
          'token': null,
        },
      ]
      ..discovered = [
        {'id': '0011223344556677', 'name': 'the shelf', 'seen': 1791072000},
        {'id': '8899aabbccddeeff', 'name': null, 'seen': null},
      ];

    await model.refresh();

    expect(model.ready, isTrue);
    expect(model.downloadFolder, _kFolder);
    final own = model.libraries.single;
    expect(own.name, 'downloads');
    expect(own.folders, [_kFolder]);
    expect(own.listed, isFalse);
    expect(own.token, 'VLD0:downloads:key');
    expect(own.files, 2);
    expect(model.files.first.token, 'VLD0:a.bin:key');
    expect(model.files.first.library, 'downloads');
    expect(model.files.last.state, 'hashing');
    expect(model.files.last.progress, 0.5);
    final [named, unknown] = model.discovered;
    expect(named.id, '0011223344556677');
    expect(named.name, 'the shelf');
    expect(named.seen!.toUtc(), DateTime.utc(2026, 10, 4, 0));
    expect(libraryLabel(named), 'the shelf');
    expect(libraryLabel(unknown), '(no name)');
    expect(unknown.seen, isNull);
    expect(model.error, isNull);
  });

  test('a server that is not ready yet is shown as such', () async {
    fake.state.ready = false;
    await model.refresh();

    expect(model.ready, isFalse);
  });

  test('a library has no token until its record is published', () async {
    fake.state.libraries.single['token'] = null;
    await model.refresh();

    expect(model.libraries.single.token, isNull);
  });

  test('a named library is made, then given its first folder', () async {
    await model.addLibrary('tunes', '/music');

    expect(paramsOf('library.add'), {'name': 'tunes'});
    expect(paramsOf('folders.add'), {'path': '/music', 'library': 'tunes'});
    expect(model.libraries.last.name, 'tunes');
    expect(model.libraries.last.folders, ['/music']);
  });

  test('a library with no name given is left for the server to name', () async {
    await model.addLibrary('', '/home/me/music');

    expect(asked(), isNot(contains('library.add')));
    expect(paramsOf('folders.add'), {'path': '/home/me/music'});
    expect(model.libraries.last.name, 'music');
  });

  test('a library that is refused gets no folder', () async {
    fake.state.failing = {'library.add': 'there is a library called tunes'};
    await model.addLibrary('tunes', '/music');

    expect(model.error, 'there is a library called tunes');
    expect(asked(), isNot(contains('folders.add')));
  });

  test(
    'renaming, listing and removing a library go through the server',
    () async {
      await model.addLibrary('tunes', '/music');

      await model.setListed('tunes', listed: false);
      expect(paramsOf('library.set'), {'name': 'tunes', 'listed': false});
      expect(model.libraries.last.listed, isFalse);

      await model.renameLibrary('tunes', 'songs');
      expect(paramsOf('library.rename'), {'name': 'tunes', 'to': 'songs'});
      expect(model.libraries.last.name, 'songs');

      await model.removeLibrary('songs');
      expect(paramsOf('library.remove'), {'name': 'songs'});
      expect(model.libraries.map((library) => library.name), ['downloads']);
    },
  );

  test('adding and removing a folder name the library it is in', () async {
    await model.addFolder('downloads', '/music');
    expect(paramsOf('folders.add'), {'path': '/music', 'library': 'downloads'});
    expect(model.libraries.single.folders, contains('/music'));

    await model.removeFolder('downloads', '/music');
    expect(paramsOf('folders.remove'), {
      'path': '/music',
      'library': 'downloads',
    });
    expect(model.libraries.single.folders, isNot(contains('/music')));
  });

  test('a rescan and a look for libraries are asked of the server', () async {
    await model.rescan();
    await model.lookForLibraries();

    expect(asked(), containsAll(['rescan', 'discover.refresh']));
  });

  test('a refusal is kept for the screen to show', () async {
    fake.state.failing = {'folders.add': 'not a folder: /nope'};
    await model.addFolder('downloads', '/nope');

    expect(model.error, 'not a folder: /nope');
  });

  test('a file found elsewhere says which folder it is kept in', () async {
    fake.state.catalogs = {
      'aa': [
        {...fileJson('one', 1), 'folder': 'music/live'},
        fileJson('two', 2),
      ],
    };
    final page = await model.browse(id: 'aa', offset: 0);

    expect(page.files.map((file) => file.folder), ['music/live', '']);
  });

  test('a library is browsed a page at a time, by id or by link', () async {
    final catalog = [fileJson('one', 1), fileJson('two', 2), fileJson('3', 3)];
    fake.state
      ..catalogName = 'the shelf'
      ..catalogs = {'aa': catalog, 'library/VLD0:lib:key': catalog};

    final first = await model.browse(id: 'aa', offset: 0);
    final rest = await model.browse(id: 'aa', offset: first.files.length);

    expect(first.total, 3);
    expect(first.library, 'the shelf');
    expect(first.files.map((file) => file.name), ['one', 'two']);
    expect(rest.files.single.name, '3');
    expect(paramsOf('browse'), {'library': 'aa', 'offset': 2});

    final linked = await model.browse(link: 'library/VLD0:lib:key', offset: 0);
    expect(linked.total, 3);
    expect(paramsOf('browse'), {'link': 'library/VLD0:lib:key', 'offset': 0});
  });

  test('a search fills in as the server hears from libraries', () async {
    fake.state.polls = [
      {'asked': 1, 'done': false, 'results': <Json>[]},
      {
        'asked': 3,
        'done': true,
        'results': [
          {
            ...fileJson('notes.txt', 9),
            'sources': 2,
            'names': ['notes.txt', 'n.txt'],
          },
        ],
      },
    ];
    final search = model.search('notes');
    final seen = <int>[];
    search.addListener(() => seen.add(search.asked));
    while (!search.done) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }

    expect(paramsOf('search.start'), {'query': 'notes'});
    expect(paramsOf('search.poll'), {'search': 7});
    expect(seen, [1, 3]);
    expect(search.hits.single.file.name, 'notes.txt');
    expect(search.hits.single.sources, 2);
    expect(search.hits.single.names, ['notes.txt', 'n.txt']);
  });

  RemoteFile found() => (
    name: 'notes.txt',
    size: 9,
    root: fileJson('notes.txt', 9, 5)['root'] as String,
    folder: 'docs',
    token: 'VLD0:a:b',
  );

  test('a found file is downloaded by what it is', () async {
    await model.downloadFile(found());

    expect(paramsOf('download'), {
      'file': fileJson('notes.txt', 9, 5),
      'name': 'notes.txt',
    });
    expect(model.downloads.single.name, 'notes.txt');
    expect(model.downloads.single.state, 'downloading');
    expect(model.downloads.single.total, 3);
    expect(model.downloaded(found()), isNull);

    fake.state.finishDownload(1);
    await model.refresh();
    expect(model.downloaded(found())!.saved, '$_kFolder/notes.txt');
  });

  test('a download that failed is asked for again as it was', () async {
    await model.downloadFile(found());
    final asking = paramsOf('download');
    fake.state.failDownload(1, 'nobody is sharing this file right now');
    await model.refresh();
    expect(model.canRetry(1), isTrue);

    await model.retryDownload(1);

    expect(fake.state.paramsOf('download'), [asking, asking]);
    expect(paramsOf('download.cancel'), {'id': 1});
    expect(model.downloads.single.id, 2);
    expect(model.downloads.single.state, 'downloading');
    expect(model.canRetry(1), isFalse);
  });

  test('a download this window did not start cannot be retried', () async {
    fake.state.downloads = [
      {'id': 9, 'name': 'x', 'size': 1, 'state': 'error', 'error': 'gone'},
    ];
    await model.refresh();

    expect(model.canRetry(9), isFalse);
    await model.retryDownload(9);
    expect(asked(), isNot(contains('download')));
  });

  test('a file asked for twice before the server answers is one download, '
      'whatever it is called', () async {
    final first = model.downloadFile(found());
    expect(model.standing(found()), FileStanding.downloading);
    final renamed = (
      name: 'other-name.txt',
      size: 9,
      root: found().root,
      folder: '',
      token: found().token,
    );
    await Future.wait([first, model.downloadFile(renamed)]);

    expect(fake.state.paramsOf('download'), hasLength(1));
    expect(model.standing(renamed), FileStanding.downloading);

    fake.state.finishDownload(1);
    await model.refresh();
    expect(model.standing(renamed), FileStanding.downloaded);
    await model.downloadFile(found());
    expect(fake.state.paramsOf('download'), hasLength(1));
  });

  test('a file whose download failed or was removed can be asked for '
      'again', () async {
    await model.downloadFile(found());
    fake.state.failDownload(1, 'no route');
    await model.refresh();
    expect(model.standing(found()), FileStanding.wanted);

    await model.downloadFile(found());
    expect(fake.state.paramsOf('download'), hasLength(2));
    expect(model.standing(found()), FileStanding.downloading);

    await model.cancelDownload(2);
    expect(model.standing(found()), FileStanding.wanted);

    // one removed by another client is noticed at the next reading.
    await model.downloadFile(found());
    fake.state.downloads.removeWhere((held) => held['id'] == 3);
    await model.refresh();
    expect(model.standing(found()), FileStanding.wanted);
  });

  test(
    'a download the server refuses leaves the file to be asked for',
    () async {
      fake.state.failing = {'download': 'not ready yet'};
      await model.downloadFile(found());

      expect(model.error, 'not ready yet');
      expect(model.standing(found()), FileStanding.wanted);
    },
  );

  test('a link is downloaded before its name is known', () async {
    await model.downloadLink('https://example.org/#VLD0:abc:def');

    expect(paramsOf('download'), {
      'token': 'https://example.org/#VLD0:abc:def',
    });
    expect(model.downloads.single.name, isNull);
    expect(model.downloads.single.state, 'resolving');

    await model.cancelDownload(model.downloads.single.id);
    expect(paramsOf('download.cancel'), {'id': 1});
    expect(model.downloads, isEmpty);
  });

  test('requests answered out of order still reach their askers', () async {
    final answers = await Future.wait([
      server.call('status'),
      server.call('discover'),
      server.call('downloads'),
    ]);

    expect(answers[0], contains('ready'));
    expect(answers[1], contains('libraries'));
    expect(answers[2], contains('downloads'));
    expect(asked(), ['status', 'discover', 'downloads']);
  });

  test('a server that goes away fails what was waiting on it', () async {
    await fake.stop();
    fake = await FakeServer.start();

    await expectLater(server.call('status'), throwsA(isA<CoreException>()));
  });

  group('the words on screen', () {
    final now = DateTime(2026, 10, 5, 14);

    test('a library seen lately says how long ago', () {
      expect(lastSeen(now, now), (text: 'just now', recent: true));
      expect(lastSeen(now.subtract(const Duration(minutes: 1)), now), (
        text: '1 minute ago',
        recent: true,
      ));
      expect(lastSeen(now.subtract(const Duration(minutes: 4)), now), (
        text: '4 minutes ago',
        recent: true,
      ));
    });

    test('a library not seen lately, or ever, is marked so', () {
      final old = lastSeen(now.subtract(kSeenRecently), now);
      expect(old.recent, isFalse);
      expect(old.text, '2026-10-05 13:54');
      expect(lastSeen(null, now), (text: 'not seen yet', recent: false));
    });

    test('the model says when it is looking for libraries', () async {
      expect(model.looking, isTrue);
      await model.refresh();
      expect(model.looking, isFalse);

      final again = model.lookForLibraries();
      expect(model.looking, isTrue);
      await again;
      expect(model.looking, isFalse);
    });

    SearchHit hit({
      int sources = 1,
      List<String> names = const [],
      List<String> libraries = const [],
    }) => (file: found(), sources: sources, names: names, libraries: libraries);

    test('a found file says where it is and what else it is called', () {
      expect(hitCaption(hit(), downloaded: false), '9 B, in 1 library');
      expect(
        hitCaption(
          hit(sources: 2, names: ['notes.txt', 'n.txt', 'm.txt']),
          downloaded: true,
        ),
        '9 B, in 2 libraries, also known as n.txt and m.txt, downloaded',
      );
      expect(
        hitCaption(
          hit(sources: 3, libraries: ['a', 'b', 'c']),
          downloaded: false,
        ),
        '9 B, in a, b and c',
      );
    });

    SharedFile shared(String path, {String state = 'shared'}) => (
      path: path,
      name: path.split('/').last,
      library: 'tunes',
      size: 1024,
      state: state,
      progress: 0.4,
      token: null,
      error: 'unreadable',
    );
    OwnLibrary own({required int files, bool listed = true}) => (
      name: 'tunes',
      folders: ['/m'],
      listed: listed,
      token: null,
      files: files,
    );

    test('an own library says its size only when every file is listed', () {
      final files = [shared('/m/a'), shared('/m/b')];
      expect(
        ownCaption(own(files: 2), files),
        '2 files, 2.0 KiB, discoverable',
      );
      expect(
        ownCaption(own(files: 900, listed: false), files),
        '900 files, by link only',
      );
      expect(ownCaption(own(files: 0), const []), '0 files, discoverable');
    });

    test('a file not shared yet says how far along it is', () {
      expect(sharingLabel(shared('/m/a', state: 'hashing')), 'hashing 40%');
      expect(sharingLabel(shared('/m/a', state: 'publishing')), 'publishing');
      expect(
        sharingLabel(shared('/m/a', state: 'error')),
        'could not share: unreadable',
      );
    });

    test('files are set out as the tree they are on disk', () {
      final tree = fileTreeOf([
        shared('/m/top.mp3'),
        shared('/m/live/one.mp3'),
        shared('/m/live/1999/two.mp3'),
        shared('/music-else/not-here.mp3'),
      ], (file) => foldersUnder('/m', file.path));

      expect(tree.count, 3);
      expect(tree.files.single.name, 'top.mp3');
      expect(tree.folders.keys, ['live']);
      final live = tree.folders['live']!;
      expect(live.count, 2);
      expect(live.files.single.name, 'one.mp3');
      expect(live.folders['1999']!.files.single.name, 'two.mp3');
      expect(tree.at(['live', '1999'])!.count, 1);
      expect(tree.at([]), same(tree));
      expect(tree.at(['nowhere']), isNull);
    });

    test('the files of a library are set out by the folder each names', () {
      final tree = fileTreeOf(
        ['top', 'trips/photo', 'trips/2024/video'],
        (String path) =>
            foldersIn(path.substring(0, path.lastIndexOf('/') + 1)),
      );

      expect(foldersIn(''), isEmpty);
      expect(foldersIn('trips/2024'), ['trips', '2024']);
      expect(tree.files, ['top']);
      expect(tree.at(['trips'])!.count, 2);
      expect(tree.at(['trips', '2024'])!.files, ['trips/2024/video']);
    });
  });
}
