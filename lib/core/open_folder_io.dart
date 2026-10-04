// on a desktop a folder is shown by the desktop's own opener.

import 'dart:io';

const String _kOpener = 'xdg-open';

/// how to show a folder to the user: there is a way here.
const Future<void> Function(String folder) kOpenFolder = openFolder;

/// show [folder] in the desktop's file manager.
Future<void> openFolder(String folder) => Process.run(_kOpener, [folder]);

/// the folder the file at [path] is in.
String folderOf(String path) => File(path).parent.path;
