// the top level of the library's browser: the libraries the server has
// discovered, a row each, in a card of the same form as a library's own
// (lib/ui/library_browser.dart), and a way to have the server look again.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../ui/app_shell.dart';
import '../../ui/library_browser.dart';
import '../../ui/parts.dart';
import '../labels.dart';
import '../library_model.dart';

// the column that says when a library was last seen.
const double _kSeenWidth = 210;

/// a library to go into: one the server knows by its id, or else the one a
/// link leads to; what to call it until it says; and its link for others,
/// when the server gives one.
typedef Opened = ({String? id, String? link, String label, String? copyLink});

/// the library a library [link] leads to.
Opened openedByLink(String link) =>
    (id: null, link: link, label: kUnnamedLibrary, copyLink: null);

class LibraryList extends StatelessWidget {
  const LibraryList({super.key, required this.model, required this.onOpened});

  final LibraryModel model;

  /// told the library whose row was pressed.
  final ValueChanged<Opened> onOpened;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: model,
    builder: (context, _) => AppCard(
      inset: kBrowserInset,
      head: BrowserRow(
        height: kControlHeight,
        lead: const BrowserPicture(kLibraryIcon),
        content: const BrowserWay(
          parts: [(key: null, name: kLibrariesRoot, onPressed: null)],
        ),
        trailing: [
          if (model.looking)
            const BrowserSpinner(key: Key('libraries-loading')),
          AppButton(
            key: const Key('discover-refresh'),
            icon: Icons.refresh,
            label: 'refresh',
            onPressed: () => unawaited(model.lookForLibraries()),
          ),
        ],
      ),
      children: [
        const BrowserRow(
          content: Text('name', style: kTextCaption),
          trailing: [_Seen('last seen'), SizeText('files')],
        ),
        // an empty list is "none" only once the server has been heard.
        if (model.discovered.isEmpty && !model.looking)
          const BrowserRow(
            content: Text(
              'no libraries found yet',
              key: Key('libraries-empty'),
              style: kTextCaption,
            ),
          ),
        for (final library in model.discovered) _library(library),
      ],
    ),
  );

  // a library's row is pressed to go into it. its link is to be had from
  // its own head, once inside.
  Widget _library(KnownLibrary library) {
    final seen = lastSeen(library.seen, DateTime.now());
    final label = libraryLabel(library);
    return OpeningRow(
      rowKey: Key('library-${library.id}'),
      picture: kLibraryIcon,
      name: label,
      trailing: [
        _Seen(
          seen.text,
          textKey: const Key('library-seen'),
          color: seen.recent ? kColorDim : kColorAmber,
        ),
        SizeText('${library.files ?? ''}', key: const Key('library-files')),
      ],
      onPressed: () => onOpened((
        id: library.id,
        link: null,
        label: label,
        copyLink: library.link,
      )),
    );
  }
}

// what stands in the column for when a library was last seen.
class _Seen extends StatelessWidget {
  const _Seen(this.text, {this.textKey, this.color = kColorDim});

  final String text;
  final Key? textKey;
  final Color color;

  @override
  Widget build(BuildContext context) => SizedBox(
    width: _kSeenWidth,
    child: Text(
      text,
      key: textKey,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: kTextCaption.copyWith(color: color),
    ),
  );
}
