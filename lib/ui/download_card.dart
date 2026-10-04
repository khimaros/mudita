// the download side of either app: the row that takes a link or token; the
// card under it for what the links led to and waits to be asked for; and
// the rows the panel of transfers has for downloads once they are asked
// for. nothing is downloaded that the user has not asked for.

import 'dart:async';

import 'package:flutter/material.dart';

import '../core/open_folder.dart';
import 'app_shell.dart';
import 'bindu_model.dart';
import 'format.dart';
import 'parts.dart';
import 'transfers_panel.dart';

class AddLinkRow extends StatefulWidget {
  const AddLinkRow({super.key, required this.onOpen});

  /// told what was entered, and answers whether it was a link at all.
  final bool Function(String input) onOpen;

  @override
  State<AddLinkRow> createState() => _AddLinkRowState();
}

class _AddLinkRowState extends State<AddLinkRow> {
  final _input = TextEditingController();
  final _focus = FocusNode();

  @override
  void dispose() {
    _input.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _submit(String text, ScaffoldMessengerState messenger) {
    if (widget.onOpen(text)) return _input.clear();
    say(messenger, 'paste a share link or token');
  }

  // a browser keeps the clipboard from a page on plain http, and may ask
  // the user first on any page. the keyboard always works, so the user is
  // pointed there.
  Future<void> _paste() async {
    final messenger = ScaffoldMessenger.of(context);
    final text = await readClipboard();
    if (text != null) return _submit(text, messenger);
    _focus.requestFocus();
    say(messenger, 'press ctrl+v in the field');
  }

  @override
  Widget build(BuildContext context) => AppCard(
    dashed: true,
    children: [
      AppRow(
        icon: Icons.link,
        content: TextField(
          key: const Key('token-input'),
          controller: _input,
          focusNode: _focus,
          style: kTextMono,
          cursorColor: kColorAccent,
          onSubmitted: (text) => _submit(text, ScaffoldMessenger.of(context)),
          decoration: fieldDecoration(context, hint: 'paste a link or token'),
        ),
        actions: [
          AppButton(
            key: const Key('paste'),
            icon: Icons.content_paste,
            label: 'paste',
            onPressed: () => unawaited(_paste()),
          ),
        ],
      ),
    ],
  );
}

/// what the links given led to and has not been asked for: a row for each,
/// under the row that took them. a link is looked up, then waits with its
/// file's name and size for "download", which makes a transfer of it; one
/// that leads nowhere says why. [onStarted] is told when one is asked for.
class PendingCard extends StatelessWidget {
  const PendingCard({
    super.key,
    required this.model,
    required this.items,
    required this.onStarted,
  });

  final BinduModel model;
  final List<Incoming> items;
  final VoidCallback onStarted;

  @override
  Widget build(BuildContext context) => AppCard(
    key: const Key('pending'),
    children: [for (final item in items) _row(item)],
  );

  Widget _row(Incoming item) {
    final key = Key('incoming-${item.id}');
    void dismiss() => unawaited(model.dismiss(item));
    final found = item.found;
    final failure = item.failure;
    if (failure != null) {
      return failedRow(
        key: key,
        name: found == null ? item.link : item.name,
        reason: failure,
        onDismiss: dismiss,
        buttonWidth: kActionWidth,
      );
    }
    if (found == null) {
      return TransferRow(
        key: key,
        busy: true,
        name: item.link,
        caption: 'looking up',
        captionKey: const Key('download-looking'),
        actions: [_leave(dismiss, compact: true)],
      );
    }
    return _waiting(key, item, dismiss);
  }

  // a file with no name in its link is known by whatever the peers sharing
  // it call it; with more than one such name the user says which to use.
  Widget _waiting(Key key, Incoming item, VoidCallback dismiss) {
    final names = item.found!.names;
    return TransferRow(
      key: key,
      icon: Icons.insert_drive_file_outlined,
      name: item.name,
      nameKey: const Key('download-name'),
      caption: formatBytes(item.found!.size),
      captionKey: const Key('download-ready'),
      actions: [
        AppButton(
          key: const Key('download'),
          icon: Icons.download,
          label: 'download',
          kind: ButtonKind.primary,
          onPressed: () {
            unawaited(model.startDownload(item));
            onStarted();
          },
        ),
        _leave(dismiss, compact: true),
      ],
      below: names.length < 2 ? null : _names(item, names),
    );
  }

  Widget _names(Incoming item, List<String> names) => Wrap(
    spacing: kSpaceIn,
    runSpacing: kSpaceIn,
    children: [
      for (final known in names)
        AppChip(
          key: Key('download-name-$known'),
          label: known,
          selected: known == item.name,
          onPressed: () => model.chooseName(item, known),
        ),
    ],
  );
}

// leaving the list is one act, whatever it is called: a download under way
// is cancelled by it, and a finished one is shared on no longer. a row
// with a button of its own has this one as an icon.
AppButton _leave(
  VoidCallback onPressed, {
  bool running = false,
  bool compact = false,
  double width = kPanelButtonWidth,
}) {
  final label = running ? 'cancel' : 'dismiss';
  return AppButton(
    key: Key(label),
    icon: Icons.close,
    label: label,
    compact: compact,
    width: width,
    kind: running ? ButtonKind.danger : ButtonKind.plain,
    onPressed: onPressed,
  );
}

// what a row says under a file's name: its size when that is known, and a
// few words on where it stands.
String _about(int? size, String? words) =>
    [if (size != null) formatBytes(size), ?words].join(', ');

/// the row of a download under way: its size, and since when if that was
/// seen. with no [total] yet, the file is still being looked up and the
/// bar says only that.
TransferRow runningRow({
  required Key key,
  required String name,
  required int? size,
  required DateTime? since,
  required int done,
  required int? total,
  required VoidCallback onCancel,
}) {
  final progress = total == null ? null : fractionOf(done, total);
  return TransferRow(
    key: key,
    icon: Icons.download,
    name: name,
    nameKey: const Key('download-name'),
    caption: _about(size, since == null ? null : 'since ${formatTime(since)}'),
    captionKey: const Key('download-running'),
    bar: true,
    progress: progress,
    end: progress == null ? '' : formatPercent(progress),
    actions: [_leave(onCancel, running: true)],
  );
}

/// the row of a download that is whole: [standing] says where it stands
/// with the user, and [extra] is what else can be done with it.
TransferRow finishedRow({
  required Key key,
  required String name,
  required int? size,
  required String standing,
  required VoidCallback onDismiss,
  List<Widget> extra = const [],
}) => TransferRow(
  key: key,
  icon: Icons.check,
  iconColor: kColorGreen,
  name: name,
  nameKey: const Key('download-name'),
  caption: _about(size, standing),
  captionKey: const Key('download-done'),
  end: 'done',
  actions: [_leave(onDismiss), ...extra],
);

/// the row of a link that led nowhere, or a download that failed: the
/// reason has the line under the name to itself, so that it is said whole.
TransferRow failedRow({
  required Key key,
  required String name,
  required String reason,
  required VoidCallback onDismiss,
  List<Widget> extra = const [],
  double buttonWidth = kPanelButtonWidth,
}) => TransferRow(
  key: key,
  icon: Icons.close,
  iconColor: kColorRed,
  name: name,
  nameKey: const Key('download-name'),
  caption: reason,
  captionKey: const Key('download-error'),
  captionColor: kColorRed,
  actions: [
    _leave(onDismiss, width: buttonWidth),
    ...extra,
  ],
);

/// where a finished download stands: saved, and when if that was seen.
String savedWords(DateTime? at) =>
    at == null ? 'saved' : 'saved at ${formatTime(at)}';

/// the small button that shows a finished download in its folder, where
/// the core said where the file is.
AppButton showInFolderButton(
  String saved,
  Future<void> Function(String folder) openFolder,
) => AppButton(
  key: const Key('show-in-folder'),
  icon: Icons.open_in_new,
  label: 'show in folder',
  compact: true,
  onPressed: () => unawaited(openFolder(folderOf(saved))),
);

/// [item], a download that was asked for, as the panel of transfers lists
/// it. where a folder can be shown ([openFolder]) a finished file is shown
/// in its own; elsewhere it is handed to the user again.
Transfer incomingTransfer(
  BinduModel model,
  Incoming item, {
  Future<void> Function(String folder)? openFolder,
}) {
  final key = Key('incoming-${item.id}');
  final download = item.download;
  final size = download?.size ?? item.found?.size;
  void dismiss() => unawaited(model.dismiss(item));
  final row = switch (item.state) {
    IncomingState.done => finishedRow(
      key: key,
      name: item.name,
      size: size,
      standing: item.saved ? savedWords(item.finishedAt) : 'not saved yet',
      onDismiss: dismiss,
      extra: [?_afterwards(model, item, openFolder)],
    ),
    IncomingState.failed => failedRow(
      key: key,
      name: item.name,
      reason: '${item.failure}',
      onDismiss: dismiss,
    ),
    _ => runningRow(
      key: key,
      name: item.name,
      size: size,
      since: item.startedAt,
      done: download?.done ?? 0,
      total: download?.total,
      onCancel: dismiss,
    ),
  };
  return (
    running: item.state == IncomingState.downloading,
    done: download?.done ?? 0,
    total: download?.total ?? 0,
    row: row,
  );
}

// what can still be done with a finished file: on a desktop it is where
// the user said, to be shown there; in a browser it can be saved again.
Widget? _afterwards(
  BinduModel model,
  Incoming item,
  Future<void> Function(String folder)? openFolder,
) {
  final saved = item.download?.saved;
  if (openFolder != null) {
    return saved == null ? null : showInFolderButton(saved, openFolder);
  }
  return AppButton(
    key: const Key('save-file'),
    icon: Icons.save_alt,
    label: item.saved ? 'save again' : 'save',
    compact: true,
    onPressed: () => unawaited(model.save(item)),
  );
}
