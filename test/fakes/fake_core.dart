// a stand-in for the mudita core, held in memory: it answers the control
// methods bindu uses from state the test sets up, and records what it was
// asked to do.

import 'dart:async';

import 'package:mudita/core/core.dart';
import 'package:mudita/core/types.dart';

const String _kNothingShared = 'nothing is shared at that link';
const String _kOffline = 'that library is offline';
const String _kNotReady = 'not ready yet';
const int _kPageFiles = 2;

class FakeCore implements Core {
  bool ready = true;

  /// what the next [pickAndShare] picks; null plays the user backing out.
  ({String name, int size})? toPick;

  /// false plays the user dismissing the save dialog.
  bool acceptSave = true;

  /// what each link leads to, as `lookup` answers; a link not here leads
  /// nowhere.
  final Map<String, Json> links = {};

  /// the catalog of each library, by library link, and what it is called.
  final Map<String, List<Json>> libraries = {};
  String libraryName = '';

  /// a library goes offline for pages from this offset on.
  int? browseFailsFrom;

  /// the offsets of the catalog pages that were answered, oldest first.
  final List<int> browsed = [];

  final List<Json> shares = [];
  final List<Json> downloads = [];

  /// other people's downloads from this core, as `uploads` lists them;
  /// null plays a core from before it had that method.
  List<Json>? uploads;

  /// the folder downloads end up in, on a core that has paths.
  String? savedIn;

  /// what was asked for, oldest first.
  final List<({String token, String name})> started = [];
  final List<int> saved = [];
  final List<int> stopped = [];
  final List<int> discarded = [];
  bool closed = false;
  int _nextId = 1;

  /// false plays a platform that cannot see a file dropped on the app.
  bool droppable = true;
  final StreamController<Drop> _drops = StreamController.broadcast();

  /// what the core refuses every request with, when it is in trouble.
  String? refusing;

  @override
  Stream<Drop>? get drops => droppable ? _drops.stream : null;

  /// the user drops [count] files on the app at once; the first of them is
  /// what [toPick] says.
  void drop(int count) =>
      _drops.add((count: count, share: () async => (await pickAndShare())!));

  @override
  Future<Json> call(String method, [Json? params]) async => switch (method) {
    _ when refusing != null => throw CoreException(refusing!),
    'status' => {'ready': ready},
    // newest first, as the core lists them.
    'shares' => {'shares': shares.reversed.toList()},
    'downloads' => {'downloads': downloads},
    'uploads' when uploads != null => {'uploads': uploads},
    'share.stop' => _stop(params!['id'] as int),
    // as the core answers anything that needs the network before it is up.
    'lookup' || 'browse' when !ready => throw const CoreException(_kNotReady),
    'lookup' =>
      links[_decoded(params!['link'])] ??
          (throw const CoreException(_kNothingShared)),
    'browse' => _page(_decoded(params!['link']), params['offset'] as int),
    _ => throw CoreException('unknown method: $method'),
  };

  // a link arrives as it was pasted, percent-encoded or not; the core reads
  // both, so this does too.
  String _decoded(Object? link) => Uri.decodeComponent(link! as String);

  Json _stop(int id) {
    stopped.add(id);
    shares.removeWhere((share) => share['id'] == id);
    return {};
  }

  Json _page(String link, int offset) {
    final catalog = libraries[link];
    final gone = browseFailsFrom != null && offset >= browseFailsFrom!;
    if (catalog == null || gone) throw const CoreException(_kOffline);
    browsed.add(offset);
    return {
      'total': catalog.length,
      'library': libraryName,
      'files': catalog.skip(offset).take(_kPageFiles).toList(),
    };
  }

  // a picked file starts out halfway through hashing.
  @override
  Future<int?> pickAndShare() async {
    final file = toPick;
    if (file == null) return null;
    final id = _nextId++;
    shares.add({
      'id': id,
      'name': file.name,
      'size': file.size,
      'state': 'hashing',
      'progress': 0.5,
      'served': 0,
      'token': null,
      'error': null,
    });
    return id;
  }

  /// the core finishes hashing and publishing the share picked last.
  void finishSharing(String token, {int served = 0}) => shares.last.addAll({
    'state': 'shared',
    'progress': 1,
    'token': token,
    'served': served,
  });

  /// tell who is downloading the share picked last, oldest first. a share
  /// nobody was told about has no such list at all, as from an older core.
  void tellDownloaders(List<Json> entries) =>
      shares.last['downloads'] = entries;

  /// one downloader as the core lists it, [done] pieces into ten.
  static Json downloader(
    String id,
    DateTime started, {
    int done = 0,
    String state = 'downloading',
    DateTime? seen,
  }) => {
    'id': id,
    'done': done,
    'total': 10,
    'state': state,
    'started': started.millisecondsSinceEpoch ~/ 1000,
    'seen': (seen ?? started).millisecondsSinceEpoch ~/ 1000,
  };

  // a download starts out with one piece of three.
  @override
  Future<int?> download(String token, String name) async {
    started.add((token: token, name: name));
    final id = _nextId++;
    downloads.add({
      'id': id,
      'name': name,
      'size': 90000,
      'state': 'downloading',
      'done': 1,
      'total': 3,
      'error': null,
    });
    return id;
  }

  /// the core finishes the download started last. where the core has
  /// paths, it says where the file is.
  void finishDownload() => downloads.last.addAll({
    'state': 'done',
    'done': 3,
    if (savedIn != null) 'saved': '$savedIn/${downloads.last['name']}',
  });

  /// the download started last fails for [reason].
  void failDownload(String reason) =>
      downloads.last.addAll({'state': 'error', 'error': reason});

  @override
  Future<bool> save(int id) async {
    if (!acceptSave) return false;
    saved.add(id);
    return true;
  }

  @override
  Future<void> discard(int id) async {
    discarded.add(id);
    downloads.removeWhere((download) => download['id'] == id);
  }

  @override
  Future<void> close() async => closed = true;
}
