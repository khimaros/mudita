// a stand-in for `dana serve`: listens on a unix socket in a temp
// directory and answers the control protocol from the canned state of a
// `FakeLibrary`, which also records what it was asked.

import 'dart:convert';
import 'dart:io';

import 'fake_library.dart';

export 'fake_library.dart' show Json, fileJson, sharedJson;

class FakeServer {
  FakeServer._(this._directory, this._listener) {
    _listener.listen(_serve);
  }

  final Directory _directory;
  final ServerSocket _listener;
  final List<Socket> _clients = [];

  /// what the server holds and answers from.
  final FakeLibrary state = FakeLibrary();

  String get path => '${_directory.path}/mudita.sock';

  static Future<FakeServer> start() async {
    final directory = Directory.systemTemp.createTempSync('dana-');
    final address = InternetAddress(
      '${directory.path}/mudita.sock',
      type: InternetAddressType.unix,
    );
    return FakeServer._(directory, await ServerSocket.bind(address, 0));
  }

  Future<void> stop() async {
    for (final client in _clients) {
      client.destroy();
    }
    await _listener.close();
    _directory.deleteSync(recursive: true);
  }

  void _serve(Socket client) {
    _clients.add(client);
    client
        .cast<List<int>>()
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .listen((line) {
          final request = jsonDecode(line) as Json;
          final result = state.ask(request);
          client.writeln(
            jsonEncode({
              'id': request['id'],
              'result': ?result,
              if (result == null) 'error': state.refusalOf(request),
            }),
          );
        }, onError: (Object _) {});
  }
}
