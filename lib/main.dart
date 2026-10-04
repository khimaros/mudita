// the entrypoint of bindu, mudita's one-file form: reach the core and show
// the one screen. on web, a link in the url fragment is looked up straight
// away.

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_web_plugins/url_strategy.dart';

import 'core/core.dart';
import 'core/open_folder.dart';
import 'core/share_link.dart';
import 'e2e/test_hook.dart';
import 'ui/app_shell.dart';
import 'ui/core_gate.dart';
import 'ui/home_page.dart';
import 'ui/web_url.dart';

// only true for e2e builds (--dart-define=MUDITA_E2E=true); gates the test
// hook.
const bool _e2e = bool.fromEnvironment('MUDITA_E2E');

void main() {
  setUpLogging();
  // the url fragment belongs to the share token. left alone, flutter's hash
  // routing would rewrite it to "#/" on startup, wiping the token from the
  // address bar and firing a bogus hashchange.
  setUrlStrategy(null);
  runApp(
    MuditaApp(
      connect: _connect,
      linkBase: linkBaseFor(kIsWeb ? Uri.base : null),
      initialLink: kIsWeb ? Uri.base.toString() : null,
      // links pasted into the already-open tab.
      links: hashChanges().map((uri) => uri.toString()),
    ),
  );
}

// the hook is installed as soon as there is a core for it to drive, which
// is what a browser test waits for.
Future<Core> _connect() async {
  final core = await openCore(library: false);
  if (_e2e) installTestHook(core);
  return core;
}

class MuditaApp extends StatelessWidget {
  const MuditaApp({
    super.key,
    required this.connect,
    required this.linkBase,
    this.initialLink,
    this.links = const Stream.empty(),
  });

  final Future<Core> Function() connect;
  final String linkBase;
  final String? initialLink;
  final Stream<String> links;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'mudita',
      debugShowCheckedModeBanner: false,
      themeMode: ThemeMode.dark,
      theme: appTheme(Brightness.light),
      darkTheme: appTheme(Brightness.dark),
      home: CoreGate(
        connect: connect,
        builder: (core, history) => HomePage(
          core: core,
          history: history,
          linkBase: linkBase,
          initialLink: initialLink,
          links: links,
          openFolder: kOpenFolder,
        ),
      ),
    );
  }
}
