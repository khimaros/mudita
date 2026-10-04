// the share side: a place to add a file, which gives way to the card of
// the file being shared, from hashing to handing out its link while the
// core serves it; and the rows the panel of transfers has for other
// people's downloads from this node, in either app.

import 'dart:async';
import 'dart:math';

import 'package:flutter/material.dart';

import '../core/share_link.dart';
import '../core/types.dart';
import 'app_shell.dart';
import 'format.dart';
import 'parts.dart';
import 'transfers_panel.dart';

const String _kHashing = 'hashing';
const String _kShared = 'shared';
const String _kFailed = 'error';
const String _kFinished = 'finished';
const String _kGone = 'gone';
// enough of a downloader's id to tell one from another.
const int _kIdShown = 4;

/// where a file to send comes from: the file dialog, and a drop on the page
/// where the platform tells of one. shown while nothing is shared.
class AddFileArea extends StatelessWidget {
  const AddFileArea({super.key, required this.droppable, required this.onPick});

  final bool droppable;

  /// null while a file is being chosen.
  final VoidCallback? onPick;

  @override
  Widget build(BuildContext context) => AppCard(
    dashed: true,
    children: [
      SizedBox(
        height: kDropHeight - 2 * kSpaceCard,
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          spacing: kSpaceRow,
          children: [
            const Icon(Icons.upload, size: kIconBig, color: kColorDim),
            Text(
              droppable
                  ? 'drop a file here to send it'
                  : 'choose a file to send',
              key: const Key('add-file-hint'),
              style: kTextName,
            ),
            AppButton(
              key: const Key('pick-file'),
              icon: Icons.folder_outlined,
              label: 'choose file',
              onPressed: onPick,
            ),
          ],
        ),
      ),
    ],
  );
}

/// someone's download of one of this node's files, as the panel of
/// transfers lists it: which file, and under it from which library, by
/// whom, and since when, or when it finished, or when they went away, and
/// how far along. there is nothing to press: it is theirs.
Transfer uploadTransfer(Upload upload) {
  final Downloader(:id, :state, :done, :total, :started, :seen) =
      upload.downloader;
  final running = state != _kFinished && state != _kGone;
  final progress = fractionOf(done, total);
  final caption = [
    ?upload.library,
    'downloader ${id.substring(0, min(_kIdShown, id.length))}',
    switch (state) {
      _kFinished => 'finished at ${formatTime(seen)}',
      _kGone => 'went away at ${formatTime(seen)}',
      _ => 'since ${formatTime(started)}',
    },
  ].join(', ');
  return (
    running: running,
    done: done,
    total: total,
    row: TransferRow(
      key: Key('upload-$id'),
      icon: switch (state) {
        _kFinished => Icons.check,
        _kGone => Icons.close,
        _ => Icons.upload,
      },
      iconColor: state == _kFinished ? kColorGreen : kColorDim,
      name: upload.file,
      caption: caption,
      captionKey: const Key('upload-caption'),
      bar: running,
      progress: running ? progress : null,
      // one that went away still says how far it got.
      end: state == _kFinished ? 'done' : formatPercent(progress),
      dimmed: state == _kGone,
    ),
  );
}

class ShareCard extends StatelessWidget {
  const ShareCard({
    super.key,
    required this.share,
    required this.linkBase,
    required this.onStop,
  });

  final Share share;

  /// the url share links are built on (see linkBaseFor).
  final String linkBase;
  final VoidCallback onStop;

  @override
  Widget build(BuildContext context) {
    final token = share.token;
    final failed = share.state == _kFailed;
    final shared = share.state == _kShared && token != null;
    return AppCard(
      children: [
        AppRow(
          icon: failed ? Icons.error_outline : Icons.insert_drive_file_outlined,
          iconColor: failed ? kColorRed : kColorDim,
          content: FileTitle(
            name: share.name,
            caption: '${formatBytes(share.size)}, ${_state(shared)}',
            captionKey: Key(failed ? 'share-error' : 'share-state'),
            captionColor: failed ? kColorRed : kColorDim,
          ),
          actions: [
            AppButton(
              key: const Key('stop-sharing'),
              icon: Icons.close,
              label: shared
                  ? 'stop sharing'
                  : failed
                  ? 'dismiss'
                  : 'cancel',
              kind: failed ? ButtonKind.plain : ButtonKind.danger,
              onPressed: onStop,
            ),
          ],
        ),
        if (shared)
          _link(context, linkFor(linkBase, token, share.name))
        else if (!failed)
          ProgressRow(
            icon: Icons.upload,
            label: _state(shared),
            labelKey: const Key('share-progress'),
            value: share.state == _kHashing ? share.progress : null,
          ),
      ],
    );
  }

  String _state(bool shared) => switch (share.state) {
    _kFailed => 'could not share: ${share.error}',
    _kHashing => 'hashing',
    _ when shared => 'ready to share',
    _ => 'publishing',
  };

  Widget _link(BuildContext context, String link) => AppRow(
    icon: Icons.link,
    content: ValueField(value: link, valueKey: const Key('share-link')),
    actions: [
      AppButton(
        key: const Key('copy-link'),
        icon: Icons.copy,
        label: 'copy link',
        kind: ButtonKind.primary,
        onPressed: () => unawaited(copyAndSay(context, link, 'link')),
      ),
    ],
  );
}
