// installs a small window.muditaTest surface so a browser e2e can seed and
// fetch over the live network (flutter's canvas ui cannot be driven from the
// dom). only wired up when built with --dart-define=MUDITA_E2E=true, so it
// never ships in a normal build. a no-op off the web.
export 'test_hook_stub.dart' if (dart.library.js_interop) 'test_hook_web.dart';
