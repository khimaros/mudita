// the core on a desktop: a `dana serve` process, reached over its control
// socket. files are named by path here, so sharing one is telling the
// server where it is, and a download is saved straight to where the user
// says.

import 'dart:io';

import 'package:file_selector/file_selector.dart';

import 'core.dart';
import 'server.dart';

const String _kPrivateSocket = 'mudita-app';

Future<Core> startCore({required bool library}) async => SocketCore(
  library
      ? await Server.start()
      // a core of this app's own: no folders, and a socket no other app or
      // server will be on, so it is always started here and stopped here.
      : await Server.start(
          socketPath: _privateSocketPath(),
          flags: const ['--no-library'],
        ),
);

String _privateSocketPath() {
  final runtime =
      Platform.environment['XDG_RUNTIME_DIR'] ?? Directory.systemTemp.path;
  return '$runtime/$_kPrivateSocket-$pid.sock';
}

class SocketCore implements Core {
  SocketCore(this._server);

  final Server _server;

  @override
  Future<Map<String, dynamic>> call(
    String method, [
    Map<String, dynamic>? params,
  ]) => _server.call(method, params);

  @override
  Future<int?> pickAndShare() async {
    final file = await openFile();
    if (file == null) return null;
    final added = await call('share.add', {'path': file.path});
    return added['share'] as int;
  }

  // the desktop window is not told of drops without a plugin for it.
  @override
  Stream<Drop>? get drops => null;

  // the place is asked for first: the server writes the file there as it
  // arrives, so that a download larger than memory never sits in a temp
  // directory that may be a ram disk.
  @override
  Future<int?> download(String token, String name) async {
    final place = await getSaveLocation(suggestedName: name);
    if (place == null) return null;
    final started = await call('download', {
      'token': token,
      'name': name,
      'path': place.path,
    });
    return started['download'] as int;
  }

  // the file is already where the user asked for it.
  @override
  Future<bool> save(int id) async => true;

  @override
  Future<void> discard(int id) => call('download.cancel', {'id': id});

  @override
  Future<void> close() => _server.close();
}
