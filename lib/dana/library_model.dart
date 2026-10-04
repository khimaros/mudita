// what the library's screens show, kept in step with the core by asking it
// again every little while: the user's libraries with their folders and
// files, other people's libraries, the downloads, and searches. everything
// here is a view of the core's state; nothing is decided on this side.
// every control method the window uses is named in this file.

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../core/core.dart';
import '../core/types.dart';
import '../ui/bindu_model.dart';

const Duration kRefreshEvery = Duration(seconds: 2);
const Duration kSearchPollEvery = Duration(milliseconds: 500);

// more shared files than this are counted but not listed.
const int _kFilesShown = 500;

const String _kDone = 'done';
const String _kFailed = 'error';
const String _kUploading = 'downloading';
const int _kMsPerSecond = 1000;

/// where a file of someone else's stands with the user: to be had, on its
/// way, or here.
enum FileStanding { wanted, downloading, downloaded }

/// one of the user's own libraries: a name over folders, with a link of
/// its own once its record is published. [listed] is whether searches find
/// it. [files] counts what is in its folders.
typedef OwnLibrary = ({
  String name,
  List<String> folders,
  bool listed,
  String? token,
  int files,
});

OwnLibrary _ownLibrary(Json json) => (
  name: json['name'] as String,
  folders: (json['folders'] as List).cast<String>(),
  listed: json['listed'] as bool,
  token: json['token'] as String?,
  files: json['files'] as int,
);

/// someone else's library. [id] stands for its route; [name] is what it
/// calls itself, unknown until it has answered once; [seen] is when the
/// server last had word that it was up, if it ever did. [files] and [link]
/// are shown when the server tells them.
typedef KnownLibrary = ({
  String id,
  String? name,
  DateTime? seen,
  int? files,
  String? link,
});

KnownLibrary _knownLibrary(Json json) {
  final seen = json['seen'] as int?;
  return (
    id: json['id'] as String,
    name: json['name'] as String?,
    seen: seen == null
        ? null
        : DateTime.fromMillisecondsSinceEpoch(seen * _kMsPerSecond),
    files: json['files'] as int?,
    link: json['link'] as String?,
  );
}

/// a search result: a file, how many libraries asked have it, every name
/// they have it under, the most common first, and what those [libraries]
/// are called, when the server says.
typedef SearchHit = ({
  RemoteFile file,
  int sources,
  List<String> names,
  List<String> libraries,
});

List<String> _strings(Object? list) =>
    ((list ?? const <dynamic>[]) as List).cast<String>();

SearchHit _searchHit(Json json) => (
  file: remoteFile(json),
  sources: json['sources'] as int,
  names: _strings(json['names']),
  libraries: _strings(json['libraries']),
);

/// one of the user's own files, which [library] it is in, and how far
/// along sharing it is. [state] is "hashing", "publishing", "shared" or
/// "error".
typedef SharedFile = ({
  String path,
  String name,
  String library,
  int size,
  String state,
  double progress,
  String? token,
  String? error,
});

SharedFile _sharedFile(Json json) => (
  path: json['path'] as String,
  name: json['name'] as String,
  library: (json['library'] ?? '') as String,
  size: json['size'] as int,
  state: json['state'] as String,
  progress: ((json['progress'] ?? 0) as num).toDouble(),
  token: json['token'] as String?,
  error: json['error'] as String?,
);

/// one search, filling in as libraries answer.
class SearchModel extends ChangeNotifier {
  SearchModel._(this._core, this.query);

  final Core _core;
  final String query;
  List<SearchHit> _hits = [];
  int _asked = 0;
  bool _done = false;
  bool _cancelled = false;
  String? _error;

  List<SearchHit> get hits => _hits;

  /// libraries that have answered or failed so far.
  int get asked => _asked;
  bool get done => _done;
  String? get error => _error;

  void cancel() => _cancelled = true;

  Future<void> _run() async {
    try {
      final started = await _core.call('search.start', {'query': query});
      while (!_done && !_cancelled) {
        final poll = await _core.call('search.poll', {
          'search': started['search'],
        });
        _hits = jsonList(poll['results']).map(_searchHit).toList();
        _asked = poll['asked'] as int;
        _done = poll['done'] as bool;
        notifyListeners();
        if (!_done) await Future<void>.delayed(kSearchPollEvery);
      }
    } on CoreException catch (e) {
      _error = e.message;
      _done = true;
      notifyListeners();
    }
  }
}

/// the file links the user has given and not yet asked to download: each
/// is looked up and waits, as on the one-file page, whose model this is,
/// so that the two cannot come to differ. what is different here is where
/// a download goes: to the library, which keeps the list of them.
class LinkLookups extends BinduModel {
  LinkLookups(super.core, this._library) {
    _library.addListener(_follow);
    _follow();
  }

  final LibraryModel _library;

  // a link is looked up once the server can reach the network, which this
  // learns by reading the server's state: once is enough.
  void _follow() {
    if (_library.ready && !ready) unawaited(refresh());
  }

  bool _gone = false;

  @override
  void dispose() {
    _gone = true;
    _library.removeListener(_follow);
    super.dispose();
  }

  // an answer from the server may come after the window has gone.
  @override
  void notifyListeners() {
    if (!_gone) super.notifyListeners();
  }

  @override
  Future<void> startDownload(Incoming item) async {
    final found = item.found;
    if (found == null) return;
    await dismiss(item);
    await _library.downloadLink(found.token, name: item.name);
  }
}

class LibraryModel extends ChangeNotifier {
  LibraryModel(this._core) {
    lookups = LinkLookups(_core, this);
  }

  final Core _core;
  Timer? _timer;
  bool _refreshing = false;

  /// the file links given on the downloads side, looked up and waiting.
  late final LinkLookups lookups;

  // what each download started from this window was asked for, and for
  // which file, by its id, so that one that failed can be asked for again.
  final Map<int, ({String key, Json asked})> _asked = {};

  // the files asked for from this window, each with the id of the download
  // the server made for it: null from the press until the server answers.
  final Map<String, int?> _wanted = {};

  // the downloads the server listed the last time it was read.
  Set<int> _listed = {};

  bool ready = false;

  // done once the server has reached the network. a library is often
  // opened before then, and its catalog is asked for then, not refused now.
  final Completer<void> _reached = Completer();

  /// where downloads go. the library holding it cannot be taken away.
  String downloadFolder = '';

  /// the user's own libraries, and the files in them.
  List<OwnLibrary> libraries = [];
  List<SharedFile> files = [];

  /// other people's libraries, as far as the server has discovered them.
  List<KnownLibrary> discovered = [];

  // whether the server's state has been read once, and whether it has been
  // asked to look for libraries again and not answered yet.
  bool _read = false;
  bool _lookingAgain = false;

  /// whether [discovered] is still to come: an empty list is not yet "none".
  bool get looking => !_read || _lookingAgain;
  List<Download> downloads = [];

  /// other people's downloads from this server, those under way first.
  List<Upload> uploads = const [];

  // the downloads that were under way at the last reading, and when each
  // one seen to finish since did.
  Set<int> _underWay = {};
  final Map<int, DateTime> _finishedAt = {};

  // when each download started from this window was asked for.
  final Map<int, DateTime> _startedAt = {};

  /// what went wrong the last time the server was asked, if anything.
  String? error;

  /// begin following the server's state.
  void start() {
    _timer = Timer.periodic(kRefreshEvery, (_) => unawaited(refresh()));
    unawaited(refresh());
  }

  @override
  void dispose() {
    _timer?.cancel();
    lookups.dispose();
    super.dispose();
  }

  /// read the server's state again. a slow answer is not asked for twice.
  Future<void> refresh() async {
    if (_refreshing) return;
    _refreshing = true;
    try {
      final status = await _core.call('status');
      ready = status['ready'] as bool;
      if (ready && !_reached.isCompleted) _reached.complete();
      downloadFolder = status['downloads'] as String;
      final own = await _core.call('libraries');
      libraries = jsonList(own['libraries']).map(_ownLibrary).toList();
      final listed = await _core.call('files', {
        'offset': 0,
        'limit': _kFilesShown,
      });
      files = jsonList(listed['files']).map(_sharedFile).toList();
      final others = await _core.call('discover');
      discovered = jsonList(others['libraries']).map(_knownLibrary).toList();
      final running = await _core.call('downloads');
      downloads = jsonList(running['downloads']).map(downloadOf).toList();
      _noteFinished();
      _forgetRemoved();
      uploads = await _readUploads();
      _read = true;
      error = null;
    } on CoreException catch (e) {
      error = e.message;
    } finally {
      _refreshing = false;
    }
    notifyListeners();
  }

  // other people's downloads from this server. one from before it had the
  // method refuses it, which is no trouble: there is then nothing to list.
  Future<List<Upload>> _readUploads() async {
    try {
      return uploadsOf(await _core.call('uploads')) ?? const [];
    } on CoreException {
      return const [];
    }
  }

  // a download seen under way and then seen whole finished between the
  // two readings: that is as near as this window knows when.
  void _noteFinished() {
    for (final download in downloads) {
      if (download.state == _kDone && _underWay.contains(download.id)) {
        _finishedAt[download.id] = DateTime.now();
      }
    }
    _underWay = {
      for (final download in downloads)
        if (download.state != _kDone && download.state != _kFailed) download.id,
    };
    _finishedAt.removeWhere((id, _) => !downloads.any((d) => d.id == id));
  }

  /// when the download [id] was seen to finish; null for one that had
  /// finished before this window looked.
  DateTime? finishedAt(int id) => _finishedAt[id];

  /// when the download [id] was asked for; null for one this window did
  /// not start.
  DateTime? startedAt(int id) => _startedAt[id];

  /// how many people are downloading from the library called [name].
  int downloadingFrom(String name) => uploads
      .where(
        (upload) =>
            upload.library == name && upload.downloader.state == _kUploading,
      )
      .length;

  // a change is shown as soon as the server has made it. a refusal is kept
  // in [error] for the screen to show, like a failed refresh, and answered
  // with null.
  Future<Json?> _do(String method, [Json? params]) async {
    final Json answer;
    try {
      answer = await _core.call(method, params);
    } on CoreException catch (e) {
      error = e.message;
      notifyListeners();
      return null;
    }
    await refresh();
    return answer;
  }

  /// share [folder] as a new library called [name]. with no name, the
  /// server calls it what the folder is called.
  Future<void> addLibrary(String name, String folder) async {
    if (name.isEmpty) {
      await _do('folders.add', {'path': folder});
    } else if (await _do('library.add', {'name': name}) != null) {
      await addFolder(name, folder);
    }
  }

  Future<void> removeLibrary(String name) =>
      _do('library.remove', {'name': name});
  Future<void> renameLibrary(String name, String to) =>
      _do('library.rename', {'name': name, 'to': to});
  Future<void> setListed(String name, {required bool listed}) =>
      _do('library.set', {'name': name, 'listed': listed});
  Future<void> addFolder(String library, String path) =>
      _do('folders.add', {'path': path, 'library': library});
  Future<void> removeFolder(String library, String path) =>
      _do('folders.remove', {'path': path, 'library': library});
  Future<void> rescan() => _do('rescan');

  /// have the server look for libraries that have joined since it last did.
  Future<void> lookForLibraries() async {
    _lookingAgain = true;
    notifyListeners();
    await _do('discover.refresh');
    _lookingAgain = false;
    notifyListeners();
  }

  /// download a file found in another library, into the download folder,
  /// under the name that library gives it.
  Future<void> downloadFile(RemoteFile file) => _download(_keyOf(file), {
    'file': remoteFileJson(file),
    'name': file.name,
  });

  /// download what the share token [token] leads to, into the download
  /// folder, under [name] when one was settled on.
  Future<void> downloadLink(String token, {String? name}) =>
      _download(token, {'token': token, 'name': ?name});

  // what a file is known by here: its token, which the same bytes have
  // whatever they are called. a file listed without one has its root.
  String _keyOf(RemoteFile file) => file.token ?? file.root;

  // the download of the file [key] started from this window, as the server
  // last listed it.
  Download? _started(String key) {
    final id = _wanted[key];
    return downloads.where((download) => download.id == id).firstOrNull;
  }

  // a file asked for stands as downloading from the press on, whatever the
  // server has said so far. one whose download failed is to be had again.
  FileStanding _standing(String key) {
    if (!_wanted.containsKey(key)) return FileStanding.wanted;
    return switch (_started(key)?.state) {
      _kFailed => FileStanding.wanted,
      _kDone => FileStanding.downloaded,
      _ => FileStanding.downloading,
    };
  }

  // a file is downloaded once at a time, however often and from wherever
  // it is asked for. the asking is noted before the server hears of it.
  Future<void> _download(String key, Json asked) async {
    if (_standing(key) != FileStanding.wanted) return;
    _wanted[key] = null;
    notifyListeners();
    final at = DateTime.now();
    final started = await _do('download', asked);
    if (started == null) {
      _wanted.remove(key);
      return notifyListeners();
    }
    final id = started['download'] as int;
    _wanted[key] = id;
    _asked[id] = (key: key, asked: asked);
    _startedAt[id] = at;
    notifyListeners();
  }

  // a download asked for here that was listed and no longer is was taken
  // off the list, here or by another client: its file can be had again.
  // one not listed yet is kept, since a reading may have been under way
  // when it was started.
  void _forgetRemoved() {
    final now = {for (final download in downloads) download.id};
    _wanted.removeWhere(
      (_, id) => id != null && _listed.contains(id) && !now.contains(id),
    );
    _listed = now;
  }

  /// stop a running download, or take a finished or failed one off the
  /// list.
  Future<void> cancelDownload(int id) async {
    _asked.remove(id);
    _startedAt.remove(id);
    _wanted.removeWhere((_, held) => held == id);
    await _do('download.cancel', {'id': id});
  }

  /// whether [retryDownload] can ask for the download [id] again: only one
  /// this window started is known well enough.
  bool canRetry(int id) => _asked.containsKey(id);

  /// ask again for a download that failed, in place of the failed one.
  Future<void> retryDownload(int id) async {
    final again = _asked[id];
    if (again == null) return;
    await cancelDownload(id);
    await _download(again.key, again.asked);
  }

  /// where [file] stands: asked for here, by its token, or else finished
  /// by anyone under its name and size.
  FileStanding standing(RemoteFile file) {
    final asked = _standing(_keyOf(file));
    if (asked != FileStanding.wanted) return asked;
    return downloaded(file) == null
        ? FileStanding.wanted
        : FileStanding.downloaded;
  }

  /// the finished download of [file], if there is one: the one asked for
  /// here, or else one with its name and size, which is all a download
  /// says of what it holds.
  Download? downloaded(RemoteFile file) {
    final mine = _started(_keyOf(file));
    if (mine?.state == _kDone) return mine;
    return downloads
        .where(
          (download) =>
              download.state == _kDone &&
              download.name == file.name &&
              download.size == file.size,
        )
        .firstOrNull;
  }

  /// a page of the catalog of another library, from [offset] on: one the
  /// server knows by its [id], or else the one a library [link] leads to.
  Future<CatalogPage> browse({
    String? id,
    String? link,
    required int offset,
  }) async {
    // whether the network is there is learnt by reading the server's state.
    if (!_reached.isCompleted) await refresh();
    await _reached.future;
    final page = await _core.call('browse', {
      'library': ?id,
      if (id == null) 'link': link,
      'offset': offset,
    });
    return (
      total: page['total'] as int,
      library: (page['library'] ?? '') as String,
      files: jsonList(page['files']).map(remoteFile).toList(),
    );
  }

  /// search the libraries the server knows for files matching [query].
  SearchModel search(String query) {
    final search = SearchModel._(_core, query);
    unawaited(search._run());
    return search;
  }
}
