// the app's link to `mudita serve`, the library server (DESIGN.md "the
// server and its remote control"). the app holds no veilid node of its own
// when it shows the library: it starts the server if none is running and
// asks it for everything over the control socket, newline-delimited json.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:logging/logging.dart';

import 'core.dart';
import 'log_history.dart';

final Logger _log = Logger('$kLogName.server');

const String _kSocketName = 'mudita.sock';
const String _kSocketEnv = 'MUDITA_SOCKET';
const String _kBinaryEnv = 'MUDITA_SERVER';
// the server binary ships beside the app under this name, because the app's
// own binary is already called mudita.
const String _kBundledBinary = 'mudita-server';
const String _kGone = 'the server went away';
const String _kNotStarted = 'the server did not start';

// a server binds its socket before it attaches to the network, so this is
// time for a process to start, not for a node to come up: and for the last
// server to finish leaving, which a new one waits out (30 seconds at most).
const Duration _kConnectTimeout = Duration(seconds: 45);
const Duration _kConnectRetry = Duration(milliseconds: 250);

// what the server writes: its own timestamp, level and module, then the
// message.
final RegExp _kServerLine = RegExp(
  r'^\S+\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+\S+:\s(.*)$',
);
const Map<String, Level> _kServerLevels = {
  'TRACE': Level.FINER,
  'DEBUG': Level.FINE,
  'INFO': Level.INFO,
  'WARN': Level.WARNING,
  'ERROR': Level.SEVERE,
};

/// one line of the server's log as the app logs it: the server's message
/// at the server's level, without its timestamp and module, so that the
/// window's log reads the same as every other. a line in no known shape is
/// passed on whole.
({Level level, String message}) parseServerLine(String line) {
  final match = _kServerLine.firstMatch(line);
  return match == null
      ? (level: Level.INFO, message: line)
      : (level: _kServerLevels[match.group(1)]!, message: match.group(2)!);
}

/// where the server listens: as the server itself decides it.
String defaultSocketPath() {
  final env = Platform.environment;
  final given = env[_kSocketEnv];
  if (given != null) return given;
  final runtime = env['XDG_RUNTIME_DIR'];
  if (runtime != null) return '$runtime/$_kSocketName';
  final state = env['XDG_STATE_HOME'] ?? '${env['HOME']}/.local/state';
  return '$state/mudita/serve/$_kSocketName';
}

/// the server binary to start: named by the environment, else the one
/// bundled beside the app.
String defaultServerBinary() =>
    Platform.environment[_kBinaryEnv] ??
    '${File(Platform.resolvedExecutable).parent.path}/$_kBundledBinary';

class Server {
  Server._(this._socket, this._process) {
    // a write to a server that has gone fails here, not where it was made.
    _socket.done.then((_) {}, onError: (Object _) => _onClosed());
    _socket
        .cast<List<int>>()
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .listen(_onLine, onDone: _onClosed, onError: (Object _) => _onClosed());
  }

  final Socket _socket;
  // the server process, when this app started it; null for one found running.
  final Process? _process;
  final Map<int, Completer<Map<String, dynamic>>> _waiting = {};
  int _nextId = 0;
  bool _gone = false;

  /// connect to the server at [socketPath], starting [binary] first when
  /// nothing is listening there. a server found running is left running on
  /// [close]; one started here is stopped. [flags] are passed to a server
  /// that is started, such as `--no-library` for one that only shares
  /// single files.
  static Future<Server> start({
    String? socketPath,
    String? binary,
    List<String> flags = const [],
  }) async {
    final path = socketPath ?? defaultSocketPath();
    final running = await _connect(path);
    if (running != null) {
      // its log is its own: said so that an empty log here is no puzzle.
      _log.info('using the server already running at $path');
      return Server._(running, null);
    }
    final process = await Process.start(binary ?? defaultServerBinary(), [
      'serve',
      ...flags,
      // so that the server stops when this app does, however it ends: a
      // window that is killed never gets to run [close].
      '--attached',
      '--socket',
      path,
    ]);
    // the server logs to stderr; its lines join the app's own log.
    // a server that cannot start says why in its last line.
    var last = '';
    process.stderr
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .map(parseServerLine)
        .listen((line) => _log.log(line.level, last = line.message));
    var exited = false;
    unawaited(process.exitCode.then((_) => exited = true));
    final deadline = DateTime.now().add(_kConnectTimeout);
    while (!exited && DateTime.now().isBefore(deadline)) {
      final socket = await _connect(path);
      if (socket != null) return Server._(socket, process);
      await Future<void>.delayed(_kConnectRetry);
    }
    process.kill();
    throw CoreException(last.isEmpty ? _kNotStarted : '$_kNotStarted: $last');
  }

  static Future<Socket?> _connect(String path) async {
    try {
      final address = InternetAddress(path, type: InternetAddressType.unix);
      return await Socket.connect(address, 0);
    } on SocketException {
      return null;
    }
  }

  /// ask the server to do [method], and return its result. throws
  /// [CoreException] when it answers with an error or goes away.
  Future<Map<String, dynamic>> call(
    String method, [
    Map<String, dynamic>? params,
  ]) {
    if (_gone) return Future.error(const CoreException(_kGone));
    final id = _nextId++;
    final answer = _waiting[id] = Completer();
    _socket.writeln(
      jsonEncode({'id': id, 'method': method, 'params': ?params}),
    );
    return answer.future;
  }

  void _onLine(String line) {
    final Map<String, dynamic> message;
    try {
      message = jsonDecode(line) as Map<String, dynamic>;
    } on FormatException {
      return;
    }
    final answer = _waiting.remove(message['id']);
    if (answer == null) return;
    if (message case {'result': final Map<String, dynamic> result}) {
      answer.complete(result);
    } else {
      answer.completeError(CoreException('${message['error']}'));
    }
  }

  void _onClosed() {
    if (!_gone) _log.severe(_kGone);
    _gone = true;
    for (final answer in _waiting.values) {
      answer.completeError(const CoreException(_kGone));
    }
    _waiting.clear();
  }

  /// hang up, and stop the server if this app started it: closing its stdin
  /// is what an attached server takes as its parent leaving, on which it
  /// vacates its slots and exits.
  Future<void> close() async {
    await _socket.close();
    await _process?.stdin.close();
  }
}
