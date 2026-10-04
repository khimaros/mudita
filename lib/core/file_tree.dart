// files set out as the tree of folders they are in, from a flat list: the
// user's own files, which the core lists by their path on disk, and the
// files of someone else's library, which name the folder they are kept in.
// pure.

const String _kSeparator = '/';

/// a folder in the tree: the folders in it by name, and its own files.
class FileTree<T> {
  final Map<String, FileTree<T>> folders = {};
  final List<T> files = [];

  /// every file at or below this folder.
  int get count =>
      files.length +
      folders.values.fold(0, (sum, folder) => sum + folder.count);

  /// the folder reached by going into [path], a name at a time; this one
  /// for an empty path, and null when there is no such folder.
  FileTree<T>? at(Iterable<String> path) {
    FileTree<T>? folder = this;
    for (final name in path) {
      folder = folder?.folders[name];
    }
    return folder;
  }
}

/// the tree of [files], each put in the folder [foldersOf] says: the names
/// of the folders on the way to it, none for the top. a file it answers
/// null for is left out.
FileTree<T> fileTreeOf<T>(
  Iterable<T> files,
  List<String>? Function(T file) foldersOf,
) {
  final tree = FileTree<T>();
  for (final file in files) {
    final path = foldersOf(file);
    if (path == null) continue;
    var folder = tree;
    for (final name in path) {
      folder = folder.folders.putIfAbsent(name, FileTree<T>.new);
    }
    folder.files.add(file);
  }
  return tree;
}

/// the folders named by [folder], as a library gives the place of a file:
/// `trips/2024`, or nothing for the top.
List<String> foldersIn(String folder) =>
    folder.split(_kSeparator).where((name) => name.isNotEmpty).toList();

/// the folders on the way from the folder [root] down to the file at
/// [path], or null when the file is not under [root].
List<String>? foldersUnder(String root, String path) {
  final prefix = root.endsWith(_kSeparator) ? root : '$root$_kSeparator';
  if (!path.startsWith(prefix)) return null;
  final parts = path.substring(prefix.length).split(_kSeparator);
  return parts.sublist(0, parts.length - 1);
}
