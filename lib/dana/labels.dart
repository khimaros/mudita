// the words the library's window puts under names: pure, so that each can
// be tested without a screen.

import '../ui/format.dart';
import 'library_model.dart';

const String _kNoName = '(no name)';

// a library writes its slot every two minutes and one unwritten for six is
// stale (DESIGN.md), so one not heard of for that long may well be gone.
const Duration kSeenRecently = Duration(minutes: 6);

/// a list in words, e.g. `a`, `a and b`, `a, b and c`.
String inWords(List<String> items) => items.length < 2
    ? items.join()
    : '${items.sublist(0, items.length - 1).join(', ')} and ${items.last}';

/// what someone else's library is called on screen.
String libraryLabel(KnownLibrary library) {
  final name = library.name ?? '';
  return name.isEmpty ? _kNoName : name;
}

/// when a library was last known to be up, in words, and whether that was
/// recently enough to expect it to answer.
({String text, bool recent}) lastSeen(DateTime? seen, DateTime now) {
  if (seen == null) return (text: 'not seen yet', recent: false);
  final ago = now.difference(seen);
  if (ago >= kSeenRecently) {
    return (text: formatDateTime(seen), recent: false);
  }
  final minutes = ago.inMinutes;
  final text = switch (minutes) {
    < 1 => 'just now',
    1 => '1 minute ago',
    _ => '$minutes minutes ago',
  };
  return (text: text, recent: true);
}

/// the line under a file a search found: its size, which libraries have it
/// (by name when the server says, else how many), what else it is called,
/// and whether it is here already.
String hitCaption(SearchHit hit, {required bool downloaded}) {
  final others = hit.names.where((name) => name != hit.file.name).toList();
  return [
    formatBytes(hit.file.size),
    if (hit.libraries.isNotEmpty)
      'in ${inWords(hit.libraries)}'
    else
      'in ${hit.sources == 1 ? '1 library' : '${hit.sources} libraries'}',
    if (others.isNotEmpty) 'also known as ${inWords(others)}',
    if (downloaded) 'downloaded',
  ].join(', ');
}

/// the line under one of the user's own libraries: how many files, their
/// size when every one of them is listed in [files], and how it is reached.
String ownCaption(OwnLibrary library, List<SharedFile> files) {
  final mine = files.where((file) => file.library == library.name).toList();
  final size = mine.fold(0, (sum, file) => sum + file.size);
  return [
    countFiles(library.files),
    if (mine.isNotEmpty && mine.length == library.files) formatBytes(size),
    library.listed ? 'discoverable' : 'by link only',
  ].join(', ');
}

/// how far along sharing one of the user's files is, for one with no link
/// to copy yet.
String sharingLabel(SharedFile file) => switch (file.state) {
  'hashing' => 'hashing ${formatPercent(file.progress)}',
  'error' => 'could not share: ${file.error}',
  _ => 'publishing',
};
