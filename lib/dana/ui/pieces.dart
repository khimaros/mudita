// what the sides of the library's window are put together from, beyond the
// parts every mudita screen shares: a stack of cards, the dashed row at the
// top of a side where something is started, and the lines of a listing.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../core/types.dart';
import '../../ui/app_shell.dart';
import '../../ui/parts.dart';
import '../desktop.dart';
import '../library_model.dart';

/// what can be done with a file of someone else's, by where it stands:
/// download it, nothing while it is on its way, or see it in its folder.
/// the keys of its forms are [keyPrefix], what the form does, [keySuffix].
class FileButton extends StatelessWidget {
  const FileButton({
    super.key,
    required this.model,
    required this.desktop,
    required this.file,
    required this.onStarted,
    this.kind = ButtonKind.plain,
    this.compact = false,
    this.keyPrefix = '',
    this.keySuffix = '',
  });

  final LibraryModel model;
  final Desktop desktop;
  final RemoteFile file;

  /// told when the download is asked for: it shows in the panel of
  /// transfers, which is opened for it.
  final VoidCallback onStarted;

  /// show each form as its icon alone, where rows are many.
  final bool compact;

  /// how the button to download looks.
  final ButtonKind kind;
  final String keyPrefix;
  final String keySuffix;

  Key _key(String does) => Key('$keyPrefix$does$keySuffix');

  @override
  Widget build(BuildContext context) {
    final saved = model.downloaded(file)?.saved;
    return switch (model.standing(file)) {
      FileStanding.wanted => AppButton(
        key: _key('download'),
        icon: Icons.download,
        label: 'download',
        compact: compact,
        kind: kind,
        onPressed: () {
          unawaited(model.downloadFile(file));
          onStarted();
        },
      ),
      FileStanding.downloading => AppButton(
        key: _key('downloading'),
        icon: Icons.downloading,
        label: 'downloading',
        compact: compact,
        onPressed: null,
      ),
      // a finished download says where it is, unless its server cannot.
      FileStanding.downloaded => AppButton(
        key: _key('show-in-folder'),
        icon: Icons.open_in_new,
        label: 'show in folder',
        compact: compact,
        onPressed: saved == null
            ? null
            : () => unawaited(desktop.openFolder(folderOf(saved))),
      ),
    };
  }
}

/// the cards of one side, one under the other.
class CardStack extends StatelessWidget {
  const CardStack({super.key, required this.children});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    spacing: kSpaceCard,
    children: children,
  );
}

/// where something is started: a field to say what, and the buttons that
/// start it. enter in the field is [onSubmitted].
class StartRow extends StatelessWidget {
  const StartRow({
    super.key,
    required this.icon,
    required this.input,
    required this.inputKey,
    required this.hint,
    required this.onSubmitted,
    required this.actions,
    this.busy = false,
  });

  final IconData icon;
  final TextEditingController input;
  final Key inputKey;
  final String hint;
  final VoidCallback onSubmitted;
  final List<Widget> actions;

  /// whether what was started is still going.
  final bool busy;

  @override
  Widget build(BuildContext context) => AppCard(
    dashed: true,
    children: [
      AppRow(
        icon: icon,
        busy: busy,
        content: TextField(
          key: inputKey,
          controller: input,
          style: kTextBody,
          cursorColor: kColorAccent,
          onSubmitted: (_) => onSubmitted(),
          decoration: fieldDecoration(context, hint: hint),
        ),
        actions: actions,
      ),
    ],
  );
}
