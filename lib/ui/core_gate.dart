// waits for the core before showing a screen that needs it, and lets go of
// the core when that screen goes. both of mudita's entrypoints start here.

import 'dart:async';

import 'package:flutter/material.dart';

import '../core/core.dart';
import '../core/log_history.dart';
import 'app_shell.dart';

const double _kSplashSpinner = 56;
const double _kSplashGap = 18;
// an error from the core is a sentence; the chip shows the start of it.
const double _kChipMaxWidth = 340;

class CoreGate extends StatefulWidget {
  const CoreGate({super.key, required this.connect, required this.builder});

  /// reaches the core, starting it if need be. a parameter so that tests
  /// can hand in a core of their own.
  final Future<Core> Function() connect;

  /// the screen to show once the core is there, with the log kept so far.
  final Widget Function(Core core, LogHistory history) builder;

  @override
  State<CoreGate> createState() => _CoreGateState();
}

class _CoreGateState extends State<CoreGate> {
  // held here, from before the core is reached: what a server says while it
  // starts, or why it could not, belongs in the log too.
  final LogHistory _history = LogHistory();
  Core? _core;
  String? _error;

  @override
  void initState() {
    super.initState();
    unawaited(_connect());
  }

  Future<void> _connect() async {
    setState(() => _error = null);
    try {
      final core = await widget.connect();
      if (!mounted) return unawaited(core.close());
      setState(() => _core = core);
    } on Exception catch (e) {
      if (mounted) setState(() => _error = '$e');
    }
  }

  // a core this app started is stopped with it; one it found is not.
  @override
  void dispose() {
    unawaited(_core?.close());
    _history.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final core = _core;
    if (core != null) return widget.builder(core, _history);
    final error = _error;
    return Scaffold(
      body: Center(
        child: error == null
            // the look and the words of the page's own splash (web/index.html),
            // which this takes over from: to the user it is one wait, not two.
            ? const Column(
                key: Key('starting'),
                mainAxisSize: MainAxisSize.min,
                children: [
                  SizedBox.square(
                    dimension: _kSplashSpinner,
                    child: CircularProgressIndicator(strokeWidth: 5),
                  ),
                  SizedBox(height: _kSplashGap),
                  Text('loading mudita...'),
                ],
              )
            : Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    'mudita could not start: $error',
                    key: const Key('core-error'),
                  ),
                  TextButton(onPressed: _connect, child: const Text('retry')),
                ],
              ),
      ),
    );
  }
}

/// shows whether the core can reach the network yet, or what it last
/// refused. nothing is shared or found until it can.
class ConnectionChip extends StatelessWidget {
  const ConnectionChip({
    super.key,
    required this.listenable,
    required this.ready,
    required this.error,
  });

  final Listenable listenable;
  final bool Function() ready;
  final String? Function() error;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: listenable,
    builder: (context, _) {
      final (label, color) = switch ((error(), ready())) {
        (final String message, _) => (message, kColorRed),
        (_, true) => ('connected', kColorGreen),
        _ => ('connecting', kColorAmber),
      };
      return Container(
        key: const Key('connection'),
        height: kControlHeight,
        constraints: const BoxConstraints(maxWidth: _kChipMaxWidth),
        padding: const EdgeInsets.symmetric(horizontal: kSpaceRow),
        decoration: BoxDecoration(
          color: kColorRaised,
          borderRadius: BorderRadius.circular(kRadiusControl),
          border: Border.all(color: kColorLineStrong),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          spacing: kSpaceIn,
          children: [
            Container(
              width: kDotSize,
              height: kDotSize,
              decoration: BoxDecoration(color: color, shape: BoxShape.circle),
            ),
            Flexible(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: kTextCaption.copyWith(color: kColorText),
              ),
            ),
          ],
        ),
      );
    },
  );
}
