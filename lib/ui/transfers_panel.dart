// what is being transferred, in either app: a button in the top bar that
// sums it up, and the panel it opens down the side of the window, with a
// row for each of the user's downloads and each of other people's
// downloads from this node. what a row is and does is its app's business:
// the panel is handed the rows.

import 'package:flutter/material.dart';

import 'app_shell.dart';
import 'format.dart';
import 'parts.dart';

// how wide the bar of a transfer is, in the panel's head and in every row.
const double _kBarWidth = kSizeWidth;

// what ends a row before its buttons: a percentage, or "done".
const double _kEndWidth = 42;

const IconData _kIcon = Icons.swap_vert;

/// one transfer as the panel lists it: whether it is under way, how many
/// of its pieces are there, and its [row].
typedef Transfer = ({bool running, int done, int total, Widget row});

/// how many transfers there are, in words.
String transfersLabel(int count) => switch (count) {
  0 => 'no transfers',
  1 => '1 transfer',
  _ => '$count transfers',
};

/// what the button says: the transfers under way and, when it is known,
/// how far along they are together.
String transfersSummary(int running, double? progress) =>
    running == 0 || progress == null
    ? transfersLabel(running)
    : '${formatPercent(progress)} of ${transfersLabel(running)}';

/// how far along the transfers under way are, taken together: the pieces
/// there over the pieces in all. one that does not know its size yet is
/// left out; null when that leaves none.
double? transfersProgress(Iterable<Transfer> transfers) {
  final sized = transfers.where(
    (transfer) => transfer.running && transfer.total > 0,
  );
  if (sized.isEmpty) return null;
  final done = sized.fold(0, (sum, transfer) => sum + transfer.done);
  final total = sized.fold(0, (sum, transfer) => sum + transfer.total);
  return done / total;
}

/// the button in the top bar: how many transfers are under way, over a
/// fill that is as wide as they are far along. it opens the panel.
class TransfersButton extends StatelessWidget {
  const TransfersButton({
    super.key,
    required this.running,
    required this.progress,
    required this.open,
    required this.onPressed,
  });

  final int running;

  /// how far along what is running is, 0..1; null when that is not known.
  final double? progress;

  /// whether the panel is open, which the button shows in its outline.
  final bool open;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final ink = running == 0 ? kColorDim : kColorText;
    return Tooltip(
      message: 'transfers',
      child: Container(
        key: const Key('transfers-button'),
        width: kTransfersButtonWidth,
        height: kControlHeight,
        clipBehavior: Clip.antiAlias,
        decoration: BoxDecoration(
          color: kColorRaised,
          borderRadius: BorderRadius.circular(kRadiusControl),
          border: Border.all(color: open ? kColorAccent : kColorLineStrong),
        ),
        child: Stack(
          fit: StackFit.expand,
          children: [
            FractionallySizedBox(
              alignment: Alignment.centerLeft,
              widthFactor: (progress ?? 0).clamp(0, 1).toDouble(),
              child: const ColoredBox(
                key: Key('transfers-fill'),
                color: kColorAccentFill,
              ),
            ),
            Material(
              type: MaterialType.transparency,
              child: InkWell(onTap: onPressed, child: _label(ink)),
            ),
          ],
        ),
      ),
    );
  }

  Widget _label(Color ink) => Row(
    mainAxisAlignment: MainAxisAlignment.center,
    spacing: kSpaceIn,
    children: [
      Icon(_kIcon, size: kIconSize, color: ink),
      Flexible(
        child: Text(
          transfersSummary(running, progress),
          key: const Key('transfers-label'),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: kTextCaption.copyWith(color: ink),
        ),
      ),
    ],
  );
}

/// the panel: a head that sums up, then the user's downloads, then other
/// people's downloads from this node. within each, what is under way
/// comes first.
class TransfersPanel extends StatelessWidget {
  const TransfersPanel({
    super.key,
    required this.downloads,
    required this.uploads,
    required this.onClose,
  });

  /// each in the order of the app's model.
  final List<Transfer> downloads;
  final List<Transfer> uploads;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) => Material(
    color: kColorSurface,
    shape: const Border(left: BorderSide(color: kColorLineStrong)),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _head(),
        Expanded(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(kSpaceRow),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: kSpaceRow,
              children: [
                if (downloads.isEmpty && uploads.isEmpty)
                  const _Section('no transfers yet', Key('transfers-empty')),
                ..._section('downloading', downloads),
                ..._section('sharing', uploads),
              ],
            ),
          ),
        ),
      ],
    ),
  );

  Widget _head() {
    final all = [...downloads, ...uploads];
    final progress = transfersProgress(all);
    final running = all.where((transfer) => transfer.running).length;
    return Container(
      key: const Key('transfers-head'),
      color: kColorHead,
      padding: const EdgeInsets.all(kSpaceRow),
      child: AppRow(
        icon: _kIcon,
        content: Text(
          transfersLabel(running),
          key: const Key('transfers-title'),
          style: kTextName,
        ),
        actions: [
          if (progress != null) ...[
            SizedBox(
              width: _kBarWidth,
              child: ProgressBar(value: progress),
            ),
            _End(
              formatPercent(progress),
              textKey: const Key('transfers-percent'),
            ),
          ],
          AppButton(
            key: const Key('transfers-close'),
            icon: Icons.close,
            label: 'close',
            compact: true,
            onPressed: onClose,
          ),
        ],
      ),
    );
  }

  // a section with nothing in it is left out, its name with it.
  List<Widget> _section(String name, List<Transfer> transfers) => [
    if (transfers.isNotEmpty) _Section(name, null),
    for (final transfer in transfers)
      if (transfer.running) transfer.row,
    for (final transfer in transfers)
      if (!transfer.running) transfer.row,
  ];
}

// a few dim words over a section of the panel, under the names of its
// rows.
class _Section extends StatelessWidget {
  const _Section(this.text, this.textKey);

  final String text;
  final Key? textKey;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(left: kLeadWidth + kSpaceRow),
    child: Text(text, key: textKey, style: kTextCaption),
  );
}

/// one row for a file on its way or waiting to be: its name and, under
/// it, a line about it, so that neither is cut short; then the bar while
/// there is one, what it comes to, and what can be done.
class TransferRow extends StatelessWidget {
  const TransferRow({
    super.key,
    required this.name,
    required this.caption,
    this.icon,
    this.iconColor = kColorDim,
    this.busy = false,
    this.nameKey,
    this.captionKey,
    this.captionColor = kColorDim,
    this.progress,
    this.bar = false,
    this.end = '',
    this.actions = const [],
    this.dimmed = false,
    this.below,
  });

  final IconData? icon;
  final Color iconColor;

  /// a spinner where the icon is, for something still being found out.
  final bool busy;
  final String name;
  final Key? nameKey;
  final String caption;
  final Key? captionKey;
  final Color captionColor;

  /// whether the row has a bar, and how full it is: null is a transfer
  /// whose end is not known.
  final bool bar;
  final double? progress;

  /// a percentage, or "done".
  final String end;
  final List<Widget> actions;

  /// a transfer that came to nothing is set back.
  final bool dimmed;

  /// what goes under the row and belongs to it, such as names to choose.
  final Widget? below;

  @override
  Widget build(BuildContext context) {
    final below = this.below;
    final row = AppRow(
      icon: icon,
      iconColor: iconColor,
      busy: busy,
      content: FileTitle(
        name: name,
        nameKey: nameKey,
        nameStyle: kTextBody,
        caption: caption,
        captionKey: captionKey,
        captionColor: captionColor,
      ),
      actions: [
        if (bar || progress != null)
          SizedBox(
            width: _kBarWidth,
            child: ProgressBar(value: progress),
          ),
        if (end.isNotEmpty) _End(end, textKey: const Key('transfer-end')),
        ...actions,
      ],
    );
    return Opacity(
      opacity: dimmed ? kDisabledOpacity : 1,
      child: below == null
          ? row
          : Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              spacing: kSpaceRow,
              children: [
                row,
                Padding(
                  padding: const EdgeInsets.only(left: kLeadWidth + kSpaceRow),
                  child: below,
                ),
              ],
            ),
    );
  }
}

// what a row, or the panel's head, comes to: right aligned, so that the
// ends of the rows stand in a column.
class _End extends StatelessWidget {
  const _End(this.text, {required this.textKey});

  final String text;
  final Key textKey;

  @override
  Widget build(BuildContext context) => SizedBox(
    width: _kEndWidth,
    child: Text(
      text,
      key: textKey,
      textAlign: TextAlign.right,
      maxLines: 1,
      style: kTextCaption,
    ),
  );
}
