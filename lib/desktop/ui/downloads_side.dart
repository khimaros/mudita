// the library's downloads: a row that takes a file link, and under it the
// card of what the links given led to and waits to be asked for; and the
// row the panel of transfers has for each of the server's downloads: under
// way, saved, or failed.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../core/share_link.dart';
import '../../core/types.dart';
import '../../ui/download_card.dart';
import '../../ui/parts.dart';
import '../../ui/transfers_panel.dart';
import '../desktop.dart';
import '../library_model.dart';
import 'pieces.dart';

const String _kDone = 'done';
const String _kFailed = 'error';
const String _kUnnamed = 'download';

class DownloadsSide extends StatelessWidget {
  const DownloadsSide({
    super.key,
    required this.model,
    required this.onLibrary,
    required this.onStarted,
  });

  final LibraryModel model;

  /// told the library link when one is entered here, where files are.
  final void Function(String link) onLibrary;

  /// told when a download is asked for.
  final VoidCallback onStarted;

  // a link to a file is looked up, and downloaded only when the user says
  // so. false when [input] is no link at all.
  bool _open(String input) {
    final library = libraryLinkFromInput(input);
    final file = fileLinkFromInput(input);
    if (library != null) {
      onLibrary(library);
    } else if (file != null) {
      unawaited(model.lookups.open(file));
    }
    return library != null || file != null;
  }

  // the row, and the rows of files that wait, are the one-file page's own.
  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: model.lookups,
    builder: (context, _) => CardStack(
      children: [
        AddLinkRow(onOpen: _open),
        if (model.lookups.incoming.isNotEmpty)
          PendingCard(
            model: model.lookups,
            items: model.lookups.incoming,
            onStarted: onStarted,
          ),
      ],
    ),
  );
}

/// one of the server's downloads, as the panel of transfers lists it.
/// taking it off the list is one act, whatever it is called: one under way
/// is cancelled by it.
Transfer downloadTransfer(
  LibraryModel model,
  Desktop desktop,
  Download download,
) {
  final Download(:id, :size, :saved) = download;
  final key = Key('download-$id');
  final name = download.name ?? _kUnnamed;
  void remove() => unawaited(model.cancelDownload(id));
  final row = switch (download.state) {
    _kDone => finishedRow(
      key: key,
      name: name,
      size: size,
      standing: savedWords(model.finishedAt(id)),
      onDismiss: remove,
      extra: [if (saved != null) showInFolderButton(saved, desktop.openFolder)],
    ),
    _kFailed => failedRow(
      key: key,
      name: name,
      reason: '${download.error}',
      onDismiss: remove,
      extra: [if (model.canRetry(id)) _tryAgain(model, id)],
    ),
    _ => runningRow(
      key: key,
      name: name,
      size: size,
      since: model.startedAt(id),
      done: download.done,
      total: download.total,
      onCancel: remove,
    ),
  };
  return (
    running: download.state != _kDone && download.state != _kFailed,
    done: download.done,
    total: download.total ?? 0,
    row: row,
  );
}

// one this window did not start cannot be asked for again from here: the
// server does not say what a download was asked with.
Widget _tryAgain(LibraryModel model, int id) => AppButton(
  key: const Key('try-again'),
  icon: Icons.refresh,
  label: 'try again',
  compact: true,
  onPressed: () => unawaited(model.retryDownload(id)),
);
