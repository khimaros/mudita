// the few pieces every screen is put together from, each drawn to the
// measures in app_shell.dart: one card, one row, one button, one field, one
// chip, one progress line and the bar along the top.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'app_shell.dart';
import 'format.dart';

const String _kAppName = 'mudita';
const double _kSpinnerStroke = 2;
// what joins a caption to the note after it.
const String _kNoteJoin = ', ';

enum ButtonKind { plain, primary, danger }

/// the one button: an icon and a label in the width of a row's action, or
/// the icon alone in a square when there is no [label].
class AppButton extends StatelessWidget {
  const AppButton({
    super.key,
    required this.icon,
    required this.onPressed,
    this.label,
    this.tooltip,
    this.compact = false,
    this.width = kActionWidth,
    this.kind = ButtonKind.plain,
  });

  final IconData icon;
  final String? label;

  /// how wide a button with a label is: as wide as a row's action, unless
  /// its place is narrower.
  final double width;

  /// what a button with no label says it does, when pointed at.
  final String? tooltip;

  /// show the icon alone, in a square, and say [label] when pointed at.
  final bool compact;
  final ButtonKind kind;

  /// null shows the button as unavailable.
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final primary = kind == ButtonKind.primary;
    final ink = switch (kind) {
      ButtonKind.primary => kColorOnAccent,
      ButtonKind.danger => kColorRed,
      ButtonKind.plain => kColorText,
    };
    final label = compact ? null : this.label;
    final tooltip = this.tooltip ?? (compact ? this.label : null);
    return Opacity(
      opacity: onPressed == null ? kDisabledOpacity : 1,
      child: SizedBox(
        width: label == null ? kControlHeight : width,
        height: kControlHeight,
        child: Material(
          color: primary ? kColorAccent : kColorRaised,
          clipBehavior: Clip.antiAlias,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(kRadiusControl),
            side: BorderSide(color: primary ? kColorAccent : kColorLineStrong),
          ),
          child: InkWell(
            onTap: onPressed,
            child: Tooltip(
              // an empty message shows nothing.
              message: tooltip ?? '',
              child: Row(
                mainAxisAlignment: MainAxisAlignment.center,
                spacing: kSpaceIn,
                children: [
                  Icon(icon, size: kIconSize, color: ink),
                  if (label != null)
                    Flexible(
                      child: Text(
                        label,
                        overflow: TextOverflow.ellipsis,
                        style: kTextCaption.copyWith(
                          color: ink,
                          fontWeight: primary ? FontWeight.w600 : null,
                        ),
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// a switch between the few sides of a screen, as one control across the
/// column.
class SegmentSwitch extends StatelessWidget {
  const SegmentSwitch({
    super.key,
    required this.segments,
    required this.selected,
    required this.onChanged,
  });

  final List<({String key, IconData icon, String label})> segments;
  final int selected;
  final ValueChanged<int> onChanged;

  @override
  Widget build(BuildContext context) => Container(
    height: kNavHeight,
    clipBehavior: Clip.antiAlias,
    decoration: BoxDecoration(
      borderRadius: BorderRadius.circular(kRadiusBox),
      border: Border.all(color: kColorLineStrong),
    ),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final (index, segment) in segments.indexed)
          Expanded(child: _segment(index, segment)),
      ],
    ),
  );

  Widget _segment(int index, ({String key, IconData icon, String label}) at) {
    final on = index == selected;
    final ink = on ? kColorOnAccent : kColorDim;
    return Material(
      color: on ? kColorAccent : Colors.transparent,
      child: InkWell(
        key: Key(at.key),
        onTap: () => onChanged(index),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          spacing: kSpaceIn,
          children: [
            Icon(at.icon, size: kIconSize, color: ink),
            Text(
              at.label,
              style: kTextBody.copyWith(
                color: ink,
                fontWeight: on ? FontWeight.w600 : null,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// the one container: rows stacked in a bordered box. [dashed] is the look
/// of a place to add something, as against a thing that is there. a [head]
/// is a bar of its own across the top of the card, in a shade of its own,
/// with a line under it and the rows below.
class AppCard extends StatelessWidget {
  const AppCard({
    super.key,
    required this.children,
    this.dashed = false,
    this.head,
    this.inset = kSpaceCard,
  });

  final List<Widget> children;
  final bool dashed;
  final Widget? head;

  /// how far what is in the card is kept in from its edges.
  final double inset;

  Widget _rows(EdgeInsets padding) => Padding(
    padding: padding,
    child: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      spacing: kSpaceRow,
      children: children,
    ),
  );

  // the bar reaches the card's edges, so the card's outline is drawn over
  // it, and its corners are cut to the card's. the bar keeps the card's
  // inset at its sides and a row's gap above and below what is in it,
  // which makes it as tall as the bar along the top of the window; under
  // the line the rows start a row's gap down. (a dashed card has no head.)
  Widget _headed(Widget head) {
    final radius = BorderRadius.circular(kRadiusBox);
    return DecoratedBox(
      position: DecorationPosition.foreground,
      decoration: BoxDecoration(
        borderRadius: radius,
        border: Border.all(color: kColorLine),
      ),
      child: ClipRRect(
        borderRadius: radius,
        child: ColoredBox(
          color: kColorSurface,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Container(
                key: const Key('card-head'),
                color: kColorHead,
                padding: EdgeInsets.symmetric(
                  horizontal: inset,
                  vertical: kSpaceRow,
                ),
                child: head,
              ),
              const SizedBox(
                key: Key('card-divider'),
                height: kLineWidth,
                child: ColoredBox(color: kColorLine),
              ),
              _rows(EdgeInsets.fromLTRB(inset, kSpaceRow, inset, inset)),
            ],
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final head = this.head;
    if (head != null) return _headed(head);
    final rows = _rows(EdgeInsets.all(inset));
    final radius = BorderRadius.circular(kRadiusBox);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: dashed ? kColorWell : kColorSurface,
        borderRadius: radius,
        border: dashed ? null : Border.all(color: kColorLine),
      ),
      child: dashed
          ? CustomPaint(foregroundPainter: const _DashedBorder(), child: rows)
          : rows,
    );
  }
}

// flutter draws no dashed borders of its own.
class _DashedBorder extends CustomPainter {
  const _DashedBorder();

  @override
  void paint(Canvas canvas, Size size) {
    final outline = Path()
      ..addRRect(
        RRect.fromRectAndRadius(
          (Offset.zero & size).deflate(kLineWidth / 2),
          const Radius.circular(kRadiusBox),
        ),
      );
    final paint = Paint()
      ..color = kColorLineStrong
      ..style = PaintingStyle.stroke
      ..strokeWidth = kLineWidth;
    for (final metric in outline.computeMetrics()) {
      for (var at = 0.0; at < metric.length; at += 2 * kDashLength) {
        canvas.drawPath(metric.extractPath(at, at + kDashLength), paint);
      }
    }
  }

  @override
  bool shouldRepaint(_DashedBorder old) => false;
}

/// the one row, on the grid every row shares: an icon in the lead column,
/// the content, then the actions. [busy] puts a spinner where the icon is.
class AppRow extends StatelessWidget {
  const AppRow({
    super.key,
    required this.content,
    this.icon,
    this.iconColor = kColorDim,
    this.busy = false,
    this.busyKey,
    this.actions = const [],
  });

  final IconData? icon;
  final Color iconColor;
  final bool busy;
  final Key? busyKey;
  final Widget content;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) => ConstrainedBox(
    constraints: const BoxConstraints(minHeight: kControlHeight),
    child: Row(
      spacing: kSpaceRow,
      children: [
        SizedBox(
          width: kLeadWidth,
          child: Center(
            child: busy
                ? SizedBox.square(
                    key: busyKey,
                    dimension: kIconSize,
                    child: const CircularProgressIndicator(
                      strokeWidth: _kSpinnerStroke,
                      color: kColorDim,
                    ),
                  )
                : Icon(icon, size: kIconSize, color: iconColor),
          ),
        ),
        Expanded(child: content),
        ...actions,
      ],
    ),
  );
}

/// what a row says about a file: its name, and a line about it below.
class FileTitle extends StatelessWidget {
  const FileTitle({
    super.key,
    required this.name,
    required this.caption,
    this.nameKey,
    this.captionKey,
    this.captionColor = kColorDim,
    this.nameStyle = kTextName,
    this.note,
    this.noteKey,
  });

  final String name;
  final String caption;

  /// how the name is set: as a title, unless it is one row among many.
  final TextStyle nameStyle;
  final Key? nameKey;
  final Key? captionKey;
  final Color captionColor;

  /// a few words after the caption that stand out from it: something
  /// going on now.
  final String? note;
  final Key? noteKey;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    mainAxisSize: MainAxisSize.min,
    children: [
      Text(
        name,
        key: nameKey,
        style: nameStyle,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      Wrap(
        children: [
          Text(
            caption,
            key: captionKey,
            style: kTextCaption.copyWith(color: captionColor),
          ),
          if (note case final note?) ...[
            Text(_kNoteJoin, style: kTextCaption.copyWith(color: captionColor)),
            Text(
              note,
              key: noteKey,
              style: kTextCaption.copyWith(color: kColorAccent),
            ),
          ],
        ],
      ),
    ],
  );
}

/// a bar filled as far as [value], 0..1. a null [value] is work whose end
/// is not known yet.
class ProgressBar extends StatelessWidget {
  const ProgressBar({super.key, required this.value});

  final double? value;

  @override
  Widget build(BuildContext context) => ClipRRect(
    borderRadius: BorderRadius.circular(kRadiusBar),
    child: LinearProgressIndicator(
      value: value,
      minHeight: kProgressHeight,
      color: kColorAccent,
      backgroundColor: kColorRaised,
    ),
  );
}

/// a few words where a row's action goes, for a row with nothing to press.
class ActionText extends StatelessWidget {
  const ActionText(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) => SizedBox(
    width: kActionWidth,
    child: Text(text, textAlign: TextAlign.right, style: kTextCaption),
  );
}

/// one line of text in a row: a file's name, a folder's path.
class RowText extends StatelessWidget {
  const RowText(this.text, {super.key, this.style = kTextBody});

  final String text;
  final TextStyle style;

  @override
  Widget build(BuildContext context) =>
      Text(text, maxLines: 1, overflow: TextOverflow.ellipsis, style: style);
}

/// a size, or a count, in the column rows keep for one.
class SizeText extends StatelessWidget {
  const SizeText(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) => SizedBox(
    width: kSizeWidth,
    child: Text(text, textAlign: TextAlign.right, style: kTextCaption),
  );
}

/// a row saying how far along something is: a few words, a bar, and the
/// percentage where a row's action goes.
class ProgressRow extends StatelessWidget {
  const ProgressRow({
    super.key,
    required this.icon,
    required this.label,
    required this.value,
    this.labelKey,
  });

  final IconData icon;
  final String label;
  final double? value;
  final Key? labelKey;

  @override
  Widget build(BuildContext context) {
    final value = this.value;
    return AppRow(
      icon: icon,
      content: Row(
        spacing: kSpaceRow,
        children: [
          Text(label, key: labelKey, style: kTextCaption),
          Expanded(child: ProgressBar(value: value)),
        ],
      ),
      actions: [ActionText(value == null ? '' : formatPercent(value))],
    );
  }
}

/// how a field looks, whether it is typed into or only read. the [hint] is
/// set in the screen's own face, whatever face the field's text is in.
InputDecoration fieldDecoration(BuildContext context, {String? hint}) {
  OutlineInputBorder outline(Color color) => OutlineInputBorder(
    borderRadius: BorderRadius.circular(kRadiusControl),
    borderSide: BorderSide(color: color),
  );
  return InputDecoration(
    isDense: true,
    filled: true,
    fillColor: kColorWell,
    hoverColor: Colors.transparent,
    hintText: hint,
    hintStyle: DefaultTextStyle.of(
      context,
    ).style.merge(kTextBody).copyWith(color: kColorDim),
    contentPadding: const EdgeInsets.symmetric(
      horizontal: kSpaceRow,
      vertical: kControlPadding,
    ),
    enabledBorder: outline(kColorLineStrong),
    focusedBorder: outline(kColorAccent),
  );
}

/// a value shown in a field, to be read, selected and copied: one line of
/// fixed-width text.
class ValueField extends StatelessWidget {
  const ValueField({super.key, required this.value, this.valueKey});

  final String value;
  final Key? valueKey;

  @override
  Widget build(BuildContext context) => InputDecorator(
    decoration: fieldDecoration(context),
    child: SelectableText(value, key: valueKey, maxLines: 1, style: kTextMono),
  );
}

/// one of a few choices, the height of every other control.
class AppChip extends StatelessWidget {
  const AppChip({
    super.key,
    required this.label,
    required this.selected,
    required this.onPressed,
  });

  final String label;
  final bool selected;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) => Material(
    color: selected ? kColorAccentSoft : kColorRaised,
    clipBehavior: Clip.antiAlias,
    shape: RoundedRectangleBorder(
      borderRadius: BorderRadius.circular(kRadiusControl),
      side: BorderSide(color: selected ? kColorAccent : kColorLineStrong),
    ),
    child: InkWell(
      onTap: onPressed,
      child: Container(
        height: kControlHeight,
        padding: const EdgeInsets.symmetric(horizontal: kSpaceRow),
        child: Center(
          widthFactor: 1,
          child: Text(label, style: kTextCaption.copyWith(color: kColorText)),
        ),
      ),
    ),
  );
}

/// the bar along the top: the logo, the name, and whatever the screen puts
/// at its far end.
class TopBar extends StatelessWidget {
  const TopBar({super.key, required this.trailing});

  final Widget trailing;

  @override
  Widget build(BuildContext context) => Container(
    height: kTopBarHeight,
    padding: const EdgeInsets.symmetric(horizontal: kSpaceRow),
    decoration: const BoxDecoration(
      border: Border(bottom: BorderSide(color: kColorLine)),
    ),
    child: Row(
      spacing: kSpaceRow,
      children: [
        const _Logo(),
        const Expanded(child: Text(_kAppName, style: kTextTitle)),
        trailing,
      ],
    ),
  );
}

class _Logo extends StatelessWidget {
  const _Logo();

  @override
  Widget build(BuildContext context) => Container(
    width: kControlHeight,
    height: kControlHeight,
    decoration: BoxDecoration(
      color: kColorWell,
      borderRadius: BorderRadius.circular(kRadiusControl),
      border: Border.all(color: kColorLine),
    ),
    child: Stack(
      children: [
        Positioned.fill(
          child: Container(
            margin: const EdgeInsets.all(kLogoRingInset),
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              border: Border.all(
                color: kColorAccentRing,
                width: kLogoRingWidth,
              ),
            ),
          ),
        ),
        Positioned.fill(
          child: Container(
            margin: const EdgeInsets.all(kLogoDotInset),
            decoration: const BoxDecoration(
              shape: BoxShape.circle,
              color: kColorAccent,
            ),
          ),
        ),
      ],
    ),
  );
}

/// tell the user something in passing. the messenger is taken, not found,
/// so that this can follow an await.
void say(ScaffoldMessengerState messenger, String message) => messenger
  ..hideCurrentSnackBar()
  ..showSnackBar(SnackBar(content: Text(message)));

/// the text on the clipboard, or null when there is none to be had. every
/// failure is caught: a browser without a clipboard fails with an error of
/// its own, which is no exception.
Future<String?> readClipboard() async {
  try {
    final text = (await Clipboard.getData(Clipboard.kTextPlain))?.text ?? '';
    return text.trim().isEmpty ? null : text;
  } catch (_) {
    return null;
  }
}

/// put [text] on the clipboard and say so, calling it [what]. a browser
/// may refuse the clipboard to a page on plain http; that is said too.
Future<void> copyAndSay(BuildContext context, String text, String what) async {
  final messenger = ScaffoldMessenger.of(context);
  try {
    await Clipboard.setData(ClipboardData(text: text));
    say(messenger, '$what copied');
  } on PlatformException {
    say(messenger, 'could not copy: select the $what and press ctrl+c');
  }
}
