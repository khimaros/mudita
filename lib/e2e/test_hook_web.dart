import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';
import 'dart:js_interop_unsafe';
import 'dart:typed_data';

import 'package:web/web.dart' as web;

import '../core/core.dart';
import '../core/core_web.dart';
import '../core/share_link.dart';
import '../core/types.dart';

const String _kHookName = 'muditaTest';
const String _kFallbackName = 'download';
const Duration _kPoll = Duration(milliseconds: 500);

/// exposes `window.muditaTest`, driving the same core the ui does. installed
/// only for e2e builds. every async call returns a promise that rejects
/// with the failure's message.
///
/// - `ready()`: true once the core can reach the network
/// - `seed(name, bytes, type)`: share a Uint8Array; resolves to the token
///   once a fetcher can reach it. replaces any earlier seed. `type` is
///   ignored: the protocol carries no types.
/// - `seedBase64(name, base64, type)`: the same, from standard base64
/// - `served()`: pieces the current seed has served
/// - `stopSeed()`: stop sharing
/// - `fetch(linkOrToken)`: download the file and hand it to the browser as a
///   download; resolves to {name, size}. replaces any earlier fetch, and
///   goes on serving the file to other downloaders
/// - `lookUp(linkOrToken)`: find out what a link leads to without
///   downloading it; resolves to {name, size}
/// - `browse(libraryLink)`: list a library; resolves to an array of
///   {folder, name, size, token}, each token good for `fetch`
/// - `stopFetch()`: abandon the current fetch, or stop it serving
void installTestHook(Core core) {
  final wasm = core as WasmCore;
  var ready = false;
  var served = 0;
  int? seedId;
  int? fetchId;

  Future<Json?> entry(String method, String list, int? id) async {
    for (final entry in jsonList((await core.call(method))[list])) {
      if (entry['id'] == id) return entry;
    }
    return null;
  }

  // what the synchronous getters report: the core's state as last asked.
  Timer.periodic(_kPoll, (_) async {
    try {
      ready = (await core.call('status'))['ready'] as bool;
      served =
          ((await entry('shares', 'shares', seedId))?['served'] ?? 0) as int;
    } on CoreException {
      ready = false;
    }
  });

  // wait until [state] says the entry is done, failing if it says error.
  Future<Json> until(String method, String list, int id, String state) async {
    while (true) {
      final now = await entry(method, list, id);
      if (now?['state'] == state) return now!;
      if (now?['state'] == 'error') throw CoreException('${now!['error']}');
      await Future<void>.delayed(_kPoll);
    }
  }

  Future<void> stopSeeding() async {
    final id = seedId;
    seedId = null;
    if (id != null) await core.call('share.stop', {'id': id});
  }

  Future<String> seedBytes(String name, Uint8List bytes) async {
    await stopSeeding();
    final file = web.File([bytes.toJS].toJS, name);
    final id = seedId = await wasm.shareFile(file);
    return (await until('shares', 'shares', id, 'shared'))['token'] as String;
  }

  Future<void> stopFetching() async {
    final id = fetchId;
    fetchId = null;
    if (id != null) await core.discard(id);
  }

  Future<Found> lookUp(String input) async {
    final link = fileLinkFromInput(input);
    if (link == null) throw ArgumentError('no file link in "$input"');
    return foundOf(await core.call('lookup', {'link': link}));
  }

  // downloads through the same path the ui takes, so the file arrives as a
  // browser download the test can inspect.
  Future<Found> fetchAndSave(String input) async {
    await stopFetching();
    final found = await lookUp(input);
    final name = found.name.isEmpty ? _kFallbackName : found.name;
    final id = fetchId = (await core.download(found.token, name))!;
    await until('downloads', 'downloads', id, 'done');
    await core.save(id);
    return found;
  }

  Future<List<RemoteFile>> browseAll(String input) async {
    final link = libraryLinkFromInput(input);
    if (link == null) throw ArgumentError('no library link in "$input"');
    final files = <RemoteFile>[];
    while (true) {
      final page = await core.call('browse', {
        'link': link,
        'offset': files.length,
      });
      final more = jsonList(page['files']).map(remoteFile).toList();
      files.addAll(more);
      if (more.isEmpty || files.length >= (page['total'] as int)) return files;
    }
  }

  JSObject named(Found found) => JSObject()
    ..setProperty('name'.toJS, found.name.toJS)
    ..setProperty('size'.toJS, found.size.toJS);

  JSObject listed(RemoteFile file) => JSObject()
    ..setProperty('folder'.toJS, file.folder.toJS)
    ..setProperty('name'.toJS, file.name.toJS)
    ..setProperty('size'.toJS, file.size.toJS)
    ..setProperty('token'.toJS, file.token?.toJS);

  final hook = JSObject()
    ..setProperty('ready'.toJS, (() => ready.toJS).toJS)
    ..setProperty(
      'seed'.toJS,
      ((JSString name, JSUint8Array bytes, JSString? _) => _promise(
        seedBytes(name.toDart, bytes.toDart).then((token) => token.toJS),
      )).toJS,
    )
    ..setProperty(
      'seedBase64'.toJS,
      ((JSString name, JSString data, JSString? _) => _promise(
        seedBytes(
          name.toDart,
          base64Decode(data.toDart),
        ).then((token) => token.toJS),
      )).toJS,
    )
    ..setProperty('served'.toJS, (() => served.toJS).toJS)
    ..setProperty(
      'stopSeed'.toJS,
      (() => _promise(stopSeeding().then((_) => null))).toJS,
    )
    ..setProperty(
      'fetch'.toJS,
      ((JSString input) => _promise(
        fetchAndSave(input.toDart).then(named),
      )).toJS,
    )
    ..setProperty(
      'lookUp'.toJS,
      ((JSString input) => _promise(lookUp(input.toDart).then(named))).toJS,
    )
    ..setProperty(
      'browse'.toJS,
      ((JSString input) => _promise(
        browseAll(
          input.toDart,
        ).then((files) => files.map(listed).toList().toJS),
      )).toJS,
    )
    ..setProperty(
      'stopFetch'.toJS,
      (() => _promise(stopFetching().then((_) => null))).toJS,
    );
  globalContext.setProperty(_kHookName.toJS, hook);
}

// a promise that rejects with the failure's message as plain text, which is
// what a test can show; a dart exception crossing over as it is arrives as
// an opaque object.
JSPromise<JSAny?> _promise(Future<JSAny?> future) => JSPromise(
  ((JSFunction resolve, JSFunction reject) {
    future.then(
      (value) => resolve.callAsFunction(null, value),
      onError: (Object e) => reject.callAsFunction(null, '$e'.toJS),
    );
  }).toJS,
);
