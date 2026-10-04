// what the core's control methods hand back, as the app's screens use it
// (DESIGN.md "the server and its remote control"). nothing here decides
// anything: these are views of the core's state.

// the core tells time in seconds.
const int _kMsPerSecond = 1000;

/// a file as a library lists it: what it calls the file, where it keeps it
/// (for showing only), and the share [token] to download it by.
typedef RemoteFile = ({
  String name,
  int size,
  String root,
  String folder,
  String? token,
});

/// a page of another library's catalog: how many files the library has in
/// all, what it is called (empty for no name), and the files of the page.
typedef CatalogPage = ({int total, String library, List<RemoteFile> files});

/// someone downloading a shared file, as the sharer sees them: how many
/// pieces of how many they have, when they [started] and when they were
/// last [seen]. [state] is "downloading", "finished" or "gone".
typedef Downloader = ({
  String id,
  int done,
  int total,
  String state,
  DateTime started,
  DateTime seen,
});

/// someone's download of one of this node's files, as the node sees it:
/// which [file], from which [library] of the node's if from one, and how
/// the [downloader] is getting on.
typedef Upload = ({String file, String? library, Downloader downloader});

/// a file shared one at a time, how far along sharing it is, and who is
/// downloading it, oldest first. [state] is "hashing", "publishing",
/// "shared" or "error".
typedef Share = ({
  int id,
  String name,
  int size,
  String state,
  double progress,
  String? token,
  String? error,
  List<Downloader> downloaders,
});

/// what a link leads to, found without downloading anything. [name] is the
/// one to save under by default, possibly empty; [names] is every name the
/// peers sharing the file give it, the most common first.
typedef Found = ({String token, int size, String name, List<String> names});

/// a download in the core. [state] is "resolving", "downloading", "done"
/// or "error"; the name and sizes are unknown until its manifest is read.
/// [saved] is where a finished one is, as the core names places.
typedef Download = ({
  int id,
  String? name,
  int? size,
  String state,
  int done,
  int? total,
  String? error,
  String? saved,
});

typedef Json = Map<String, dynamic>;

List<Json> jsonList(Object? list) => (list! as List).cast<Json>();

RemoteFile remoteFile(Json json) => (
  name: json['name'] as String,
  size: json['size'] as int,
  root: json['root'] as String,
  folder: (json['folder'] ?? '') as String,
  token: json['token'] as String?,
);

Json remoteFileJson(RemoteFile file) => {
  'name': file.name,
  'size': file.size,
  'root': file.root,
};

Share shareOf(Json json) => (
  id: json['id'] as int,
  name: json['name'] as String,
  size: json['size'] as int,
  state: json['state'] as String,
  progress: ((json['progress'] ?? 0) as num).toDouble(),
  token: json['token'] as String?,
  error: json['error'] as String?,
  // a core from before downloaders were told of says nothing of them.
  downloaders: jsonList(
    json['downloads'] ?? const <dynamic>[],
  ).map(downloaderOf).toList(),
);

DateTime _moment(Object? seconds) =>
    DateTime.fromMillisecondsSinceEpoch((seconds! as int) * _kMsPerSecond);

Downloader downloaderOf(Json json) => (
  id: json['id'] as String,
  done: json['done'] as int,
  total: json['total'] as int,
  state: json['state'] as String,
  started: _moment(json['started']),
  seen: _moment(json['seen']),
);

/// other people's downloads from this node, as `uploads` answers. null
/// when the reply has no such list, as from a core without the method.
List<Upload>? uploadsOf(Json json) => switch (json['uploads']) {
  final List<dynamic> list => list.cast<Json>().map(_uploadOf).toList(),
  _ => null,
};

Upload _uploadOf(Json json) => (
  file: json['name'] as String,
  library: json['library'] as String?,
  downloader: downloaderOf(json),
);

/// the downloaders of the one file [share] is, as uploads: what a core
/// without `uploads` still tells of them.
List<Upload> uploadsOfShare(Share share) => [
  for (final downloader in share.downloaders)
    (file: share.name, library: null, downloader: downloader),
];

Found foundOf(Json json) => (
  token: json['token'] as String,
  size: json['size'] as int,
  name: (json['name'] ?? '') as String,
  names: ((json['names'] ?? const <dynamic>[]) as List).cast<String>(),
);

Download downloadOf(Json json) => (
  id: json['id'] as int,
  name: json['name'] as String?,
  size: json['size'] as int?,
  state: json['state'] as String,
  done: (json['done'] ?? 0) as int,
  total: json['total'] as int?,
  error: json['error'] as String?,
  saved: json['saved'] as String?,
);
