// what bindu's one screen shows, kept in step with the core by asking it
// again every second: the file being shared, and the files links have led
// to, each from being looked up to being downloaded. everything here is a
// view of the core's state, plus what the user has asked for.

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../core/core.dart';
import '../core/types.dart';

const Duration kBinduRefreshEvery = Duration(seconds: 1);

/// what a download is saved as when nobody has given the file a name.
const String kFallbackName = 'download';

const String _kDone = 'done';

enum IncomingState { looking, waiting, downloading, done, failed }

/// a file a link led to, from the link being looked up to the download of
/// it. it stays listed until the user dismisses it.
class Incoming {
  Incoming._(this.id, this.link);

  /// this screen's own number for it, which no other entry gets.
  final int id;

  /// the link or token, as the user gave it.
  final String link;

  /// what [link] leads to, once known.
  Found? found;

  /// the name the download is saved under: the link's, or the one the user
  /// chose among those the file is known by.
  String name = kFallbackName;

  /// the download in the core, once asked for.
  Download? download;

  /// whether the finished download has been handed to the user.
  bool saved = false;

  /// when the download was asked for, and when it was seen to be whole.
  DateTime? startedAt;
  DateTime? finishedAt;

  /// whether a download of it was asked for: until then it waits where
  /// its link was given, and from then on it is a transfer.
  bool get started => _downloadId != null || _starting;

  String? _error;
  int? _downloadId;
  bool _starting = false;

  /// why the link could not be looked up, or the download not finished.
  String? get failure => _error ?? download?.error;

  // a download counts as under way from the moment it is asked for, which
  // is before the core has answered.
  IncomingState get state {
    if (failure != null) return IncomingState.failed;
    if (_downloadId != null || _starting) {
      return download?.state == _kDone
          ? IncomingState.done
          : IncomingState.downloading;
    }
    return found == null ? IncomingState.looking : IncomingState.waiting;
  }
}

class BinduModel extends ChangeNotifier {
  BinduModel(this._core);

  final Core _core;
  Timer? _timer;
  bool _refreshing = false;
  int? _shareId;
  int _nextIncoming = 0;

  /// whether the core can reach the network yet.
  bool ready = false;

  // done once the core has reached the network. a link is often there
  // before the network is (the page was opened with it), and what it leads
  // to is asked for then, not refused now.
  final Completer<void> _reached = Completer();

  /// what went wrong the last time the core was asked, if anything.
  String? error;

  /// the file being shared from this screen, if any. one at a time.
  Share? share;

  /// other people's downloads from this core.
  List<Upload> uploads = const [];

  /// true from choosing a file until the core has it.
  bool picking = false;

  /// the files links have led to, newest first.
  final List<Incoming> incoming = [];

  /// begin following the core's state.
  void start() {
    _timer = Timer.periodic(kBinduRefreshEvery, (_) => unawaited(refresh()));
    unawaited(refresh());
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  /// read the core's state again. a slow answer is not asked for twice.
  Future<void> refresh() async {
    if (_refreshing) return;
    _refreshing = true;
    try {
      ready = (await _core.call('status'))['ready'] as bool;
      if (ready && !_reached.isCompleted) _reached.complete();
      share = jsonList(
        (await _core.call('shares'))['shares'],
      ).where((entry) => entry['id'] == _shareId).map(shareOf).firstOrNull;
      final downloads = {
        for (final entry in jsonList(
          (await _core.call('downloads'))['downloads'],
        ))
          entry['id'] as int: downloadOf(entry),
      };
      for (final item in incoming) {
        _follow(item, downloads[item._downloadId]);
      }
      uploads = await _readUploads();
      error = null;
    } on CoreException catch (e) {
      error = e.message;
    } finally {
      _refreshing = false;
    }
    notifyListeners();
  }

  // a download is handed over once, the moment it is whole.
  void _follow(Incoming item, Download? download) {
    item.download = download;
    if (item.state != IncomingState.done || item.finishedAt != null) return;
    item.finishedAt = DateTime.now();
    unawaited(save(item));
  }

  // other people's downloads from this core, by its own account when it
  // gives one. an older core has no such method, and still tells who is
  // downloading the one file shared.
  Future<List<Upload>> _readUploads() async {
    try {
      final listed = uploadsOf(await _core.call('uploads'));
      if (listed != null) return listed;
    } on CoreException {
      // answered below, by what the share itself says.
    }
    final shared = share;
    return shared == null ? const [] : uploadsOfShare(shared);
  }

  /// whether a file is shared, or about to be: there is room for no other.
  bool get sharing => picking || share != null;

  /// let the user choose a file, and share it.
  Future<void> pickFile() => shareFrom(_core.pickAndShare);

  /// share what [start] gives the core, which answers with the share's id,
  /// or null when the user backed out.
  Future<void> shareFrom(Future<int?> Function() start) async {
    picking = true;
    notifyListeners();
    try {
      _shareId = await start() ?? _shareId;
    } on CoreException catch (e) {
      error = e.message;
    }
    picking = false;
    await refresh();
  }

  /// stop sharing the file.
  Future<void> stopSharing() async {
    final id = _shareId;
    _shareId = null;
    share = null;
    if (id == null) return;
    try {
      await _core.call('share.stop', {'id': id});
    } on CoreException catch (e) {
      error = e.message;
    }
    await refresh();
  }

  // a new entry for [link] at the top of the list. null when the link has
  // one already; one that failed is tried afresh.
  Future<Incoming?> _entry(String link) async {
    for (final old in incoming.where((item) => item.link == link).toList()) {
      if (old.state != IncomingState.failed) return null;
      await dismiss(old);
    }
    final item = Incoming._(_nextIncoming++, link);
    incoming.insert(0, item);
    return item;
  }

  /// find out what [link], a share link or token, leads to. nothing is
  /// downloaded: that waits for [startDownload].
  Future<void> open(String link) async {
    final item = await _entry(link);
    if (item == null) return;
    notifyListeners();
    try {
      await _reached.future;
      final found = foundOf(await _core.call('lookup', {'link': link}));
      item
        ..found = found
        ..name = found.name.isEmpty ? kFallbackName : found.name;
    } on CoreException catch (e) {
      item._error = e.message;
    }
    notifyListeners();
  }

  /// save [item] under [chosen], one of the names the file is known by.
  void chooseName(Incoming item, String chosen) {
    item.name = chosen;
    notifyListeners();
  }

  /// download what [item]'s link led to. the user asking for this is the
  /// only thing that starts a download.
  Future<void> startDownload(Incoming item) async {
    final found = item.found;
    if (found == null || item._downloadId != null || item._starting) return;
    item
      .._starting = true
      ..startedAt = DateTime.now();
    notifyListeners();
    try {
      item._downloadId = await _core.download(found.token, item.name);
    } on CoreException catch (e) {
      item._error = e.message;
    }
    item._starting = false;
    // dismissed while the user was saying where to save it.
    if (!incoming.contains(item)) return dismiss(item);
    await refresh();
  }

  /// hand [item]'s finished download to the user, again if they ask.
  Future<void> save(Incoming item) async {
    final id = item._downloadId;
    if (id == null) return;
    try {
      item.saved = await _core.save(id) || item.saved;
    } on CoreException catch (e) {
      error = e.message;
    }
    notifyListeners();
  }

  /// take [item] off the list. a download it had is let go of in the core,
  /// which stops it, or stops it being shared on.
  Future<void> dismiss(Incoming item) async {
    incoming.remove(item);
    notifyListeners();
    final id = item._downloadId;
    if (id == null) return;
    try {
      await _core.discard(id);
    } on CoreException catch (e) {
      error = e.message;
    }
  }

  /// a page of the library behind [link], from [offset] on, and the
  /// library's name.
  Future<CatalogPage> browse(String link, int offset) async {
    await _reached.future;
    final page = await _core.call('browse', {'link': link, 'offset': offset});
    return (
      total: page['total'] as int,
      library: (page['library'] ?? '') as String,
      files: jsonList(page['files']).map(remoteFile).toList(),
    );
  }

  /// download [file], picked out of a library, under the name it has there.
  Future<void> downloadFrom(RemoteFile file) async {
    final token = file.token;
    if (token == null) return;
    final item = await _entry(token);
    if (item == null) return;
    item
      ..found = (
        token: token,
        size: file.size,
        name: file.name,
        names: const [],
      )
      ..name = file.name.isEmpty ? kFallbackName : file.name;
    await startDownload(item);
    // one picked out of a library is downloading or it is nothing: backed
    // out of, it does not stay to be asked for again.
    if (item.state == IncomingState.waiting) await dismiss(item);
  }

  /// whether [file] is listed here already, on its way or arrived: it is
  /// not to be had a second time.
  bool has(RemoteFile file) => incoming.any(
    (item) => item.link == file.token && item.state != IncomingState.failed,
  );
}
