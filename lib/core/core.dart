// the app's one way to the network: the mudita core, which is the rust
// implementation every client shares (DESIGN.md "the server and its remote
// control"). the app holds no protocol code of its own. it asks the core
// for everything through the control methods, and the core does the
// sharing, the downloading and the logging.
//
// where the core runs depends on the platform: in the browser it is a wasm
// module in the page; on a desktop it is a `dana serve` process reached
// over its socket. both are driven the same way.

import 'core_io.dart' if (dart.library.js_interop) 'core_web.dart';

/// the core refused a request, or could not be reached; with a message fit
/// to show the user.
class CoreException implements Exception {
  const CoreException(this.message);
  final String message;

  @override
  String toString() => message;
}

/// files the user dropped on the app in one go: how many there were, and a
/// way to [share] the first, which gives the share's id.
typedef Drop = ({int count, Future<int> Function() share});

abstract class Core {
  /// ask the core to do [method], and return its result. throws
  /// [CoreException] when it answers with an error or has gone away.
  Future<Map<String, dynamic>> call(
    String method, [
    Map<String, dynamic>? params,
  ]);

  /// let the user choose a file, and share it. the share's id, or null if
  /// they backed out.
  Future<int?> pickAndShare();

  /// what the user drops on the app, where the platform says: null where a
  /// drop cannot be seen, so that the screen does not invite one.
  Stream<Drop>? get drops;

  /// download the file behind the share token [token], to be saved as
  /// [name]. the download's id, or null if the user backed out of saying
  /// where it should go.
  Future<int?> download(String token, String name);

  /// hand a finished download to the user. false if they backed out.
  Future<bool> save(int id);

  /// abandon a download, or stop serving a finished one, and free what it
  /// stored.
  Future<void> discard(int id);

  /// let go of the core, stopping it if this app started it.
  Future<void> close();
}

/// the core for this platform. with [library] it is the user's library
/// server, started if none is running and left running if one was; without,
/// a core of this app's own that shares single files and goes when the app
/// does. a browser has only the second kind.
Future<Core> openCore({required bool library}) => startCore(library: library);
