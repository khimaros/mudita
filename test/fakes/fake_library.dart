// a stand-in for the library server's side of the control protocol, held in
// memory: it answers the methods the library's window uses from state the
// test sets up, changes that state as the server would, and records what
// it was asked. `FakeServer` puts one behind a real socket; `FakeLibraryCore`
// hands one to the window directly.

import 'package:mudita/core/core.dart';

typedef Json = Map<String, dynamic>;

const String _kDownloadFolder = '/home/me/Downloads/mudita';
const String _kDownloadsName = 'downloads';
const int _kPageFiles = 2;
const String _kUnknownMethod = 'unknown method';

Json fileJson(String name, int size, [int seed = 0]) => {
  'name': name,
  'size': size,
  'root': seed.toRadixString(16).padLeft(2, '0') * 32,
};

/// one of the user's own files as `files` lists it, shared and with a
/// token made from its name.
Json sharedJson(String library, String path, int size) => {
  'path': path,
  'name': path.split('/').last,
  'library': library,
  'size': size,
  'state': 'shared',
  'progress': 1,
  'served': 0,
  'token': 'VLD0:${path.split('/').last}:key',
  'error': null,
};

class FakeLibrary {
  /// every request received, oldest first.
  final List<Json> requests = [];

  /// methods that answer with this error instead of a result.
  Map<String, String> failing = {};

  bool ready = true;
  String downloadFolder = _kDownloadFolder;

  /// the user's own libraries: each `name`, `folders`, `listed`, `token`.
  List<Json> libraries = [
    {
      'name': _kDownloadsName,
      'folders': [_kDownloadFolder],
      'listed': false,
      'token': 'VLD0:downloads:key',
    },
  ];

  /// the user's own files, as `files` lists them.
  List<Json> files = [];

  /// other people's libraries, as `discover` lists them.
  List<Json> discovered = [];
  List<Json> downloads = [];

  /// the catalog of each of other people's libraries, by id or by link,
  /// and what they are called.
  Map<String, List<Json>> catalogs = {};
  String catalogName = '';

  /// other people's downloads from this server, as `uploads` lists them;
  /// null plays a server from before it had that method.
  List<Json>? uploads;

  /// what each file link leads to, as `lookup` answers, by the link with
  /// nothing in it percent-encoded.
  Map<String, Json> links = {};

  /// what `search.poll` answers, one per poll; the last one repeats.
  List<Json> polls = [
    {'asked': 0, 'done': true, 'results': <Json>[]},
  ];

  int _nextDownload = 1;

  /// the requests for [method], oldest first, by their params.
  List<Json> paramsOf(String method) => [
    for (final request in requests)
      if (request['method'] == method) (request['params'] ?? Json()) as Json,
  ];

  /// a catalog page from this offset on is refused with [failing]'s word
  /// for `browse`, while the pages before it are answered.
  int? browseFailsFrom;

  /// why [request] is refused, or null when it is answered.
  String? refusalOf(Json request) {
    if (request['method'] == 'uploads' && uploads == null) {
      return _kUnknownMethod;
    }
    final params = (request['params'] ?? Json()) as Json;
    final from = browseFailsFrom;
    if (from != null && (params['offset'] ?? from) as int < from) return null;
    return failing[request['method']];
  }

  /// what the server answers; null stands for a refusal, said by
  /// [refusalOf].
  Json? ask(Json request) {
    requests.add(request);
    if (refusalOf(request) != null) return null;
    return _answer(
      request['method'] as String,
      (request['params'] ?? Json()) as Json,
    );
  }

  Json _answer(String method, Json params) => switch (method) {
    'status' => _status(),
    'libraries' => {'libraries': _views()},
    'files' => {'total': files.length, 'files': files},
    'discover' => {'libraries': discovered},
    'downloads' => {'downloads': downloads},
    'uploads' => {'uploads': uploads},
    'shares' => {'shares': <Json>[]},
    'lookup' => links[_decoded(params['link'])]!,
    'library.add' => _change(
      () => libraries.add({
        'name': params['name'],
        'folders': <String>[],
        'listed': params['listed'] ?? true,
        'token': 'VLD0:${params['name']}:key',
      }),
    ),
    'library.remove' => _change(
      () => libraries.removeWhere((held) => held['name'] == params['name']),
    ),
    'library.rename' => _change(() => _rename(params['name'], params['to'])),
    'library.set' => _change(
      () => _named(params['name'])['listed'] = params['listed'],
    ),
    'folders.add' => _change(() => _addFolder(params)),
    'folders.remove' => _change(
      () => _folders(params['library']).remove(params['path']),
    ),
    'browse' => _page(
      catalogs[params['library'] ?? _decoded(params['link'])]!,
      params['offset'] as int,
    ),
    'search.start' => {'search': 7},
    'search.poll' => polls.length > 1 ? polls.removeAt(0) : polls.first,
    'download' => _startDownload(params),
    'download.cancel' => _change(
      () => downloads.removeWhere((held) => held['id'] == params['id']),
    ),
    _ => {},
  };

  // a link arrives as it was pasted, percent-encoded or not; the server
  // reads both, so this does too.
  String _decoded(Object? link) => Uri.decodeComponent(link! as String);

  Json _change(void Function() change) {
    change();
    return {};
  }

  Json _named(Object? name) =>
      libraries.firstWhere((held) => held['name'] == name);

  List<String> _folders(Object? name) =>
      (_named(name)['folders'] as List).cast<String>();

  void _rename(Object? name, Object? to) {
    _named(name)['name'] = to;
    for (final file in files.where((file) => file['library'] == name)) {
      file['library'] = to;
    }
  }

  // with no library named, a folder becomes a library called what it is.
  void _addFolder(Json params) {
    final path = params['path'] as String;
    final library = params['library'];
    if (library != null) return _folders(library).add(path);
    libraries.add({
      'name': path.split('/').last,
      'folders': [path],
      'listed': true,
      'token': 'VLD0:${path.split('/').last}:key',
    });
  }

  List<Json> _views() => [
    for (final held in libraries)
      {
        ...held,
        'files': files.where((file) => file['library'] == held['name']).length,
      },
  ];

  Json _status() => {
    'ready': ready,
    'downloads': downloadFolder,
    'files': files.length,
    'shared': files.where((file) => file['state'] == 'shared').length,
    'discovered': discovered.length,
    'libraries': _views(),
  };

  // two files to a page, so that a test can ask for the second.
  Json _page(List<Json> catalog, int offset) => {
    'total': catalog.length,
    'library': catalogName,
    'files': catalog.skip(offset).take(_kPageFiles).toList(),
  };

  // a file asked for by what it is starts with no pieces of three; one
  // asked for by a link is still being looked up.
  Json _startDownload(Json params) {
    final file = params['file'] as Json?;
    final id = _nextDownload++;
    downloads = [
      {
        'id': id,
        'name': file?['name'],
        'size': file?['size'],
        'state': file == null ? 'resolving' : 'downloading',
        'done': 0,
        'total': file == null ? null : 3,
        'error': null,
        'saved': null,
      },
      ...downloads,
    ];
    return {'download': id};
  }

  /// the download [id] finishes, saved under its name in the download
  /// folder.
  void finishDownload(int id) {
    final download = downloads.firstWhere((held) => held['id'] == id);
    download.addAll({
      'state': 'done',
      'done': download['total'],
      'saved': '$downloadFolder/${download['name']}',
    });
  }

  /// the download [id] fails for [reason].
  void failDownload(int id, String reason) => downloads
      .firstWhere((held) => held['id'] == id)
      .addAll({'state': 'error', 'error': reason});
}

/// the library server as the window reaches it, without a socket.
class FakeLibraryCore implements Core {
  final FakeLibrary state = FakeLibrary();
  bool closed = false;

  @override
  Future<Json> call(String method, [Json? params]) async {
    final request = {'method': method, 'params': ?params};
    final answer = state.ask(request);
    if (answer == null) throw CoreException(state.refusalOf(request)!);
    return answer;
  }

  @override
  Stream<Drop>? get drops => null;

  @override
  Future<int?> pickAndShare() => throw UnimplementedError();

  @override
  Future<int?> download(String token, String name) =>
      throw UnimplementedError();

  @override
  Future<bool> save(int id) => throw UnimplementedError();

  @override
  Future<void> discard(int id) => throw UnimplementedError();

  @override
  Future<void> close() async => closed = true;
}
