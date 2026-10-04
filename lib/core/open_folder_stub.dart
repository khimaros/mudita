// a page in a browser has no file manager to show a folder in.

/// how to show a folder to the user: nothing, here.
const Future<void> Function(String folder)? kOpenFolder = null;

/// the folder the file at [path] is in. what a browser calls where it kept
/// a file is no path, and has no folder.
String folderOf(String path) => path;
