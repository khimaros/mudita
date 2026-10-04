// the log on screen: one running list of what the core has logged, worded
// as the cli logs it. bindu keeps it in a bar along the bottom that opens
// into a panel; the library's window gives it a card.

import 'package:flutter/material.dart';
import 'package:logging/logging.dart';

import '../core/log_history.dart';
import 'app_shell.dart';

const double _kGap = 12;
const double _kLogHeight = 220;
const String _kEmpty = 'nothing yet.';
const String _kTitle = 'log';

String _twoDigits(int value) => value.toString().padLeft(2, '0');

/// one record as shown: its local time of day, its level, then its message.
String formatRecord(LogRecord record) {
  final time = record.time;
  final clock = [time.hour, time.minute, time.second].map(_twoDigits).join(':');
  return '$clock ${record.level.name} ${record.message}';
}

// newest first, so the latest line shows without scrolling.
String _text(LogHistory history) => history.records.isEmpty
    ? _kEmpty
    : history.records.reversed.map(formatRecord).join('\n');

// every line kept, scrolling within whatever height it is given.
class _Lines extends StatelessWidget {
  const _Lines({required this.history, this.style});

  final LogHistory history;
  final TextStyle? style;

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    child: ListenableBuilder(
      listenable: history,
      builder: (context, _) => SelectableText(
        _text(history),
        key: const Key('activity-log'),
        style: style ?? const TextStyle(fontFamily: 'monospace'),
      ),
    ),
  );
}

class LogCard extends StatelessWidget {
  const LogCard({super.key, required this.history, this.fill = false});

  final LogHistory history;

  /// take all the height the parent gives, for a log with a tab to itself,
  /// instead of a fixed height among other cards.
  final bool fill;

  @override
  Widget build(BuildContext context) {
    final lines = _Lines(history: history);
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(_kTitle, style: Theme.of(context).textTheme.titleLarge),
            const SizedBox(height: _kGap),
            if (fill)
              Expanded(
                child: SizedBox(width: double.infinity, child: lines),
              )
            else
              ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: _kLogHeight),
                child: lines,
              ),
          ],
        ),
      ),
    );
  }
}

/// the log as a bar along the bottom of the screen showing its latest line.
/// pressing it opens a panel with every line; pressing the panel's top
/// strip closes it again.
class LogBar extends StatefulWidget {
  const LogBar({super.key, required this.history});

  final LogHistory history;

  @override
  State<LogBar> createState() => _LogBarState();
}

class _LogBarState extends State<LogBar> {
  bool _open = false;

  void _toggle() => setState(() => _open = !_open);

  @override
  Widget build(BuildContext context) => Material(
    color: _open ? kColorSurface : kColorBg,
    shape: const Border(top: BorderSide(color: kColorLine)),
    child: _open ? _panel(context) : _bar(),
  );

  // the strip that opens and closes the log: all of it is the button, open
  // or closed, and only the arrow and what is beside the title differ.
  Widget _strip({required Key key, required IconData arrow, Widget? beside}) =>
      InkWell(
        key: key,
        onTap: _toggle,
        child: Container(
          height: kStatusHeight,
          padding: const EdgeInsets.symmetric(horizontal: kSpaceRow),
          child: Row(
            spacing: kSpaceRow,
            children: [
              const Icon(Icons.list, size: kIconSize, color: kColorDim),
              const Text(_kTitle, style: kTextCaption),
              Expanded(child: beside ?? const SizedBox.shrink()),
              Icon(arrow, size: kIconSize, color: kColorDim),
            ],
          ),
        ),
      );

  Widget _bar() => _strip(
    key: const Key('log-bar'),
    arrow: Icons.expand_less,
    beside: ListenableBuilder(
      listenable: widget.history,
      builder: (context, _) => Text(
        widget.history.records.isEmpty
            ? _kEmpty
            : formatRecord(widget.history.records.last),
        key: const Key('log-latest'),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: kTextMono.copyWith(color: kColorDim),
      ),
    ),
  );

  Widget _panel(BuildContext context) => Column(
    mainAxisSize: MainAxisSize.min,
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      _strip(key: const Key('log-close'), arrow: Icons.expand_more),
      Container(
        padding: const EdgeInsets.fromLTRB(
          kSpaceRow + kIconSize + kSpaceRow,
          0,
          kSpaceRow,
          kSpaceRow,
        ),
        constraints: const BoxConstraints(maxHeight: _kLogHeight),
        child: _Lines(
          history: widget.history,
          style: kTextMono.copyWith(color: kColorDim),
        ),
      ),
    ],
  );
}
