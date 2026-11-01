// other people's libraries: a row that opens one by its link, and under it
// the browser, which is at the list of the libraries the server has
// discovered until one of them is gone into.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../core/share_link.dart';
import '../../ui/library_browser.dart';
import '../../ui/parts.dart';
import '../desktop.dart';
import '../library_model.dart';
import 'library_list.dart';
import 'pieces.dart';

class DiscoverSide extends StatefulWidget {
  const DiscoverSide({
    super.key,
    required this.model,
    required this.desktop,
    required this.opened,
    required this.opening,
    required this.onOpened,
    required this.onStarted,
  });

  final LibraryModel model;
  final Desktop desktop;

  /// told when a download is asked for.
  final VoidCallback onStarted;

  /// the library being browsed, if any. kept by the window, since another
  /// side can open one too.
  final Opened? opened;

  /// which opening this is, counted by the window: the same library opened
  /// again is another.
  final int opening;
  final ValueChanged<Opened?> onOpened;

  @override
  State<DiscoverSide> createState() => _DiscoverSideState();
}

class _DiscoverSideState extends State<DiscoverSide> {
  final _input = TextEditingController();

  @override
  void dispose() {
    _input.dispose();
    super.dispose();
  }

  void _open(String text, ScaffoldMessengerState messenger) {
    final library = libraryLinkFromInput(text);
    if (library == null) return say(messenger, 'paste a library link');
    _input.clear();
    widget.onOpened(openedByLink(library));
  }

  Future<void> _paste() async {
    final messenger = ScaffoldMessenger.of(context);
    final text = await readClipboard();
    if (text != null) return _open(text, messenger);
    say(messenger, 'press ctrl+v in the field');
  }

  // the row that takes a link is there at every level: a library can be
  // opened by its link from inside another.
  @override
  Widget build(BuildContext context) {
    final opened = widget.opened;
    return CardStack(
      children: [
        StartRow(
          icon: Icons.link,
          input: _input,
          inputKey: const Key('library-link-input'),
          hint: 'paste a library link to open it',
          onSubmitted: () => _open(_input.text, ScaffoldMessenger.of(context)),
          actions: [
            AppButton(
              key: const Key('paste'),
              icon: Icons.content_paste,
              label: 'paste',
              onPressed: () => unawaited(_paste()),
            ),
          ],
        ),
        if (opened == null)
          LibraryList(model: widget.model, onOpened: widget.onOpened)
        else
          // each opening starts afresh, at the library's top.
          LibraryBrowser(
            key: ValueKey((opened, widget.opening)),
            browse: (offset) => widget.model.browse(
              id: opened.id,
              link: opened.link,
              offset: offset,
            ),
            redraw: widget.model,
            fallbackName: opened.label,
            copyLink: opened.copyLink,
            onRoot: () => widget.onOpened(null),
            // where a file stands is the library's to say: to be had, on
            // its way, or here.
            fileButton: (context, file) => FileButton(
              model: widget.model,
              desktop: widget.desktop,
              file: file,
              onStarted: widget.onStarted,
              compact: true,
              keyPrefix: 'catalog-',
              keySuffix: '-${file.folder}/${file.name}',
            ),
          ),
      ],
    );
  }
}
