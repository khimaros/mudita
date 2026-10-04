// what is around the sides of either app's window: the bar along the top,
// with the transfers button and the connection chip; the column the sides
// are set in; the log in a bar along the bottom; and the panel of
// transfers, which comes down the side over all but the top bar.

import 'package:flutter/material.dart';

import '../core/log_history.dart';
import 'app_shell.dart';
import 'log_card.dart';
import 'parts.dart';
import 'transfers_panel.dart';

class AppFrame extends StatelessWidget {
  const AppFrame({
    super.key,
    required this.history,
    required this.chip,
    required this.width,
    required this.panel,
    required this.redraw,
    required this.downloads,
    required this.uploads,
    required this.child,
  });

  /// the log so far.
  final LogHistory history;

  /// whether the core is connected, at the end of the top bar.
  final Widget chip;

  /// how wide the column of the sides is at most.
  final double width;

  /// whether the panel of transfers is open. the app opens it when a
  /// download is started, so that it is seen.
  final ValueNotifier<bool> panel;

  /// what [downloads] and [uploads] give changes when this does.
  final Listenable redraw;

  /// the user's downloads, and other people's downloads from this node,
  /// as the panel lists them.
  final List<Transfer> Function() downloads;
  final List<Transfer> Function() uploads;

  /// the sides.
  final Widget child;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([redraw, panel]),
    builder: (context, _) {
      final down = downloads();
      final up = uploads();
      final all = [...down, ...up];
      return Stack(
        children: [
          Scaffold(
            backgroundColor: kColorBg,
            bottomNavigationBar: LogBar(history: history),
            body: Column(
              children: [
                TopBar(
                  trailing: Row(
                    mainAxisSize: MainAxisSize.min,
                    spacing: kSpaceRow,
                    children: [
                      TransfersButton(
                        running: all.where((one) => one.running).length,
                        progress: transfersProgress(all),
                        open: panel.value,
                        onPressed: () => panel.value = !panel.value,
                      ),
                      chip,
                    ],
                  ),
                ),
                Expanded(child: _column()),
              ],
            ),
          ),
          if (panel.value) _drawer(down, up),
        ],
      );
    },
  );

  Widget _column() => SingleChildScrollView(
    padding: const EdgeInsets.all(kSpacePage),
    child: Center(
      child: ConstrainedBox(
        constraints: BoxConstraints(maxWidth: width),
        child: child,
      ),
    ),
  );

  // the panel, under the top bar and over everything else, the log bar
  // included, with a shade over what it leaves showing: pressing the shade
  // puts the panel away. it is as wide as it likes, or as the window is.
  Widget _drawer(List<Transfer> down, List<Transfer> up) => Positioned.fill(
    top: kTopBarHeight,
    child: LayoutBuilder(
      builder: (context, window) => Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            child: GestureDetector(
              key: const Key('transfers-shade'),
              behavior: HitTestBehavior.opaque,
              onTap: () => panel.value = false,
              child: const ColoredBox(color: kColorShade),
            ),
          ),
          SizedBox(
            width: window.maxWidth < kPanelWidth
                ? window.maxWidth
                : kPanelWidth,
            child: TransfersPanel(
              downloads: down,
              uploads: up,
              onClose: () => panel.value = false,
            ),
          ),
        ],
      ),
    ),
  );
}
