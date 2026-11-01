// what the library's window asks of the desktop it runs on, apart from the
// core: a folder chosen by the user, and a folder shown to them. handed to
// the window as a value, so that a test can stand in for the desktop.

import 'package:file_selector/file_selector.dart';

import '../core/open_folder_io.dart';

export '../core/open_folder_io.dart' show folderOf;

typedef Desktop = ({
  /// let the user choose a folder; null if they backed out.
  Future<String?> Function() pickFolder,

  /// show [folder] in the desktop's file manager.
  Future<void> Function(String folder) openFolder,
});

const Desktop kDesktop = (pickFolder: _pickFolder, openFolder: openFolder);

Future<String?> _pickFolder() => getDirectoryPath();
