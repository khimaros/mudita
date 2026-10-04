// the entrypoint of mudita with dana, its library, for linux desktop: it
// shares whole folders and browses and searches what other libraries share.
// the library itself is `mudita serve`; this is its window, which starts a
// server when none is running (DESIGN.md "the server and its remote
// control"). run with `flutter run -t lib/dana/main.dart`.

import 'package:flutter/material.dart';

import '../core/core.dart';
import '../core/log_history.dart';
import '../ui/app_shell.dart';
import '../ui/core_gate.dart';
import 'library_model.dart';
import 'ui/home.dart';

const String _kAppName = 'mudita';

void main() {
  setUpLogging();
  runApp(
    MaterialApp(
      title: _kAppName,
      debugShowCheckedModeBanner: false,
      themeMode: ThemeMode.dark,
      theme: appTheme(Brightness.light),
      darkTheme: appTheme(Brightness.dark),
      home: CoreGate(
        connect: () => openCore(library: true),
        builder: (core, history) => _Library(core: core, history: history),
      ),
    ),
  );
}

// owns the model for as long as the window shows the library.
class _Library extends StatefulWidget {
  const _Library({required this.core, required this.history});

  final Core core;
  final LogHistory history;

  @override
  State<_Library> createState() => _LibraryState();
}

class _LibraryState extends State<_Library> {
  late final LibraryModel _model = LibraryModel(widget.core)..start();

  @override
  void dispose() {
    _model.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      LibraryHome(model: _model, history: widget.history);
}
