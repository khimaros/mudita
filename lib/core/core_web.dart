// the core in a browser: the same rust core, built as a wasm module and
// loaded into the page (`web/wasm/mudita.js`, made by `make core-wasm`).
// files here are browser File objects, so sharing one hands the core the
// File, and a download lands in the origin private file system until the
// user saves it.

import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';

import 'package:logging/logging.dart';
import 'package:web/web.dart' as web;

import 'core.dart';
import 'log_history.dart';

const String _kModule = 'wasm/mudita.js';
// the browser's stores for this node; one per origin.
const String _kNamespace = 'mudita';
const String _kCoreTarget = 'mudita';
const String _kTargetSeparator = '::';
// the events of a drag over the page, and what a drag of files calls what
// it carries.
const String _kDragOver = 'dragover';
const String _kDropped = 'drop';
const String _kFilesType = 'Files';

const Map<String, Level> _kLevels = {
  'TRACE': Level.FINER,
  'DEBUG': Level.FINE,
  'INFO': Level.INFO,
  'WARN': Level.WARNING,
  'ERROR': Level.SEVERE,
};

extension type _Module(JSObject _) implements JSObject {
  @JS('default')
  external JSPromise<JSAny?> init();
  external JSPromise<JSAny?> start(JSString namespace);
  external JSPromise<JSString> call(JSString line);
  external JSPromise<JSNumber> shareFile(web.File file);
  external JSPromise<JSNumber> downloadFile(JSString link);
  external JSPromise<web.File> downloadedFile(JSNumber id);
  external JSPromise<JSAny?> discardDownload(JSNumber id);
  external void setLogHandler(JSFunction handler);
  external JSPromise<JSAny?> stop();
}

Future<Core> startCore({required bool library}) => WasmCore.start();

// the module rejects with the message as a plain string.
Future<T> _guard<T>(Future<T> Function() ask) async {
  try {
    return await ask();
  } on CoreException {
    rethrow;
  } catch (e) {
    throw CoreException('$e');
  }
}

// a core event joins the app's log under the core's own name for where it
// came from: `mudita::peer` is logged as `mudita.peer`.
void _relay(JSString level, JSString target, JSString message) {
  final name = target.toDart
      .replaceFirst(_kCoreTarget, kLogName)
      .replaceAll(_kTargetSeparator, '.');
  Logger(name).log(_kLevels[level.toDart] ?? Level.INFO, message.toDart);
}

class WasmCore implements Core {
  WasmCore._(this._module) {
    web.document.addEventListener(_kDragOver, _onDragOver);
    web.document.addEventListener(_kDropped, _onDrop);
  }

  final _Module _module;
  int _nextId = 0;
  final StreamController<Drop> _drops = StreamController.broadcast();

  // a browser answers a file dropped on a page by leaving the page for the
  // file, unless both events are claimed. drags of anything else are left
  // to the page.
  late final JSFunction _onDragOver = ((web.DragEvent event) {
    if (_carriesFiles(event)) event.preventDefault();
  }).toJS;
  late final JSFunction _onDrop = ((web.DragEvent event) {
    if (!_carriesFiles(event)) return;
    event.preventDefault();
    final files = event.dataTransfer!.files;
    final first = files.item(0);
    if (first == null) return;
    _drops.add((count: files.length, share: () => shareFile(first)));
  }).toJS;

  bool _carriesFiles(web.DragEvent event) =>
      event.dataTransfer?.types.toDart.any(
        (type) => type.toDart == _kFilesType,
      ) ??
      false;

  @override
  Stream<Drop>? get drops => _drops.stream;

  static Future<WasmCore> start() => _guard(() async {
    // resolved against the page, since a bare relative path is no module
    // specifier.
    final url = Uri.base.removeFragment().resolve(_kModule).toString();
    final module = (await importModule(url.toJS).toDart) as _Module;
    await module.init().toDart;
    module.setLogHandler(_relay.toJS);
    await module.start(_kNamespace.toJS).toDart;
    return WasmCore._(module);
  });

  @override
  Future<Map<String, dynamic>> call(
    String method, [
    Map<String, dynamic>? params,
  ]) => _guard(() async {
    final line = jsonEncode({
      'id': _nextId++,
      'method': method,
      'params': params ?? const <String, dynamic>{},
    });
    final answer =
        jsonDecode((await _module.call(line.toJS).toDart).toDart)
            as Map<String, dynamic>;
    if (answer case {'result': final Map<String, dynamic> result}) {
      return result;
    }
    throw CoreException('${answer['error']}');
  });

  /// share a file the page already holds. what [pickAndShare] does once the
  /// user has chosen one; on its own for tests, which make their own files.
  Future<int> shareFile(web.File file) =>
      _guard(() async => (await _module.shareFile(file).toDart).toDartInt);

  @override
  Future<int?> pickAndShare() async {
    final file = await _pick();
    return file == null ? null : shareFile(file);
  }

  // the browser's own file dialog, through a file input that is never
  // shown. a browser tells of a dismissed dialog with `cancel`.
  Future<web.File?> _pick() {
    final picked = Completer<web.File?>();
    final input = web.HTMLInputElement()..type = 'file';
    input.onchange = ((web.Event _) => picked.complete(
      input.files?.item(0),
    )).toJS;
    input.oncancel = ((web.Event _) => picked.complete(null)).toJS;
    input.click();
    return picked.future;
  }

  // the name rides in the link, which is where the core takes it from.
  @override
  Future<int?> download(String token, String name) => _guard(() async {
    final link = '$token/${Uri.encodeComponent(name)}';
    return (await _module.downloadFile(link.toJS).toDart).toDartInt;
  });

  // the finished file is on disk in the browser's own storage; an object
  // url hands it to the browser's downloads without loading it whole.
  @override
  Future<bool> save(int id) => _guard(() async {
    final file = await _module.downloadedFile(id.toJS).toDart;
    final url = web.URL.createObjectURL(file);
    final anchor = web.HTMLAnchorElement()
      ..href = url
      ..download = file.name;
    web.document.body!.append(anchor);
    anchor
      ..click()
      ..remove();
    return true;
  });

  @override
  Future<void> discard(int id) =>
      _guard(() => _module.discardDownload(id.toJS).toDart);

  @override
  Future<void> close() {
    web.document.removeEventListener(_kDragOver, _onDragOver);
    web.document.removeEventListener(_kDropped, _onDrop);
    unawaited(_drops.close());
    return _guard(() => _module.stop().toDart);
  }
}
