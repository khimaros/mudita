// someone else's library as folders to go into, one level at a time: its
// folders first and then its files, in a card whose head is the way to
// where one is. the one view of a library in both apps: it is handed a way
// to read a page of the catalog and a button for each file, and knows
// neither app's model. reading someone else's library takes no library of
// one's own. the rows and the head it is made of are here for the list of
// libraries too, which the library's window puts above it
// (lib/dana/ui/library_list.dart).

import 'dart:async';

import 'package:flutter/material.dart';

import '../core/core.dart';
import '../core/file_tree.dart';
import '../core/types.dart';
import 'app_shell.dart';
import 'format.dart';
import 'parts.dart';

/// what a library is called before it has said, or if it has no name.
const String kUnnamedLibrary = 'library';

/// the picture of a library.
const IconData kLibraryIcon = Icons.local_library_outlined;

/// this card keeps what is in it closer to its edges than other cards do:
/// by as much as its head has above and below, so the head reads as a bar.
const double kBrowserInset = kSpaceRow;

// the first column of every row is as wide as a button, since on a file's
// row it holds one.
const double _kLeadWidth = kControlHeight;

// a row of the listing is a little taller than a row elsewhere, as tall as
// the switch: it is a thing to press, or holds one.
const double _kRowHeight = kNavHeight;

const double _kSpinnerStroke = 2;
const String _kSeparator = '/';

/// the catalog of one library, read to its end a page after another, each
/// page shown as it comes.
class LibraryListing extends ChangeNotifier {
  LibraryListing(this._browse);

  // the page of the library's catalog from an offset on.
  final Future<CatalogPage> Function(int offset) _browse;
  bool _reading = false;
  bool _gone = false;

  /// the files listed so far, in the order the library gives them: sorted
  /// by folder.
  final List<RemoteFile> files = [];

  /// how many files the library has in all, once it has said.
  int? total;

  /// what the library calls itself; empty until it says, or for no name.
  String name = '';

  /// why the last page asked for could not be read, if it could not.
  String? error;

  /// whether every file is listed.
  bool get listed => total != null && files.length >= total!;

  /// what there is to say of a listing that is not all there: why it
  /// stopped, that the library is being looked up, or how far along it is.
  /// null once every file is listed.
  String? get status {
    if (error != null) return 'could not read the library: $error';
    if (total == null) return 'looking up the library';
    if (listed) return null;
    return 'listing: ${files.length} of ${countFiles(total!)}';
  }

  /// read on to the end of the catalog. after a failure this carries on
  /// from what is already listed.
  Future<void> read() async {
    if (_reading) return;
    _reading = true;
    error = null;
    notifyListeners();
    try {
      while (!listed && !_gone) {
        final page = await _browse(files.length);
        total = page.total;
        name = page.library;
        files.addAll(page.files);
        notifyListeners();
        if (page.files.isEmpty) break;
      }
    } on CoreException catch (e) {
      error = e.message;
    }
    _reading = false;
    notifyListeners();
  }

  // an answer may come after the screen that asked has gone.
  @override
  void notifyListeners() {
    if (!_gone) super.notifyListeners();
  }

  @override
  void dispose() {
    _gone = true;
    super.dispose();
  }
}

class LibraryBrowser extends StatefulWidget {
  /// a browser is for one opening of one library: another is given another
  /// key, so that it starts afresh, at the top.
  const LibraryBrowser({
    super.key,
    required this.browse,
    required this.fileButton,
    required this.redraw,
    this.fallbackName = kUnnamedLibrary,
    this.copyLink,
    this.onRoot,
    this.onClose,
  });

  /// the page of the library's catalog from [offset] on.
  final Future<CatalogPage> Function(int offset) browse;

  /// what can be done with one file, as a button at the start of its row.
  final Widget Function(BuildContext context, RemoteFile file) fileButton;

  /// what [fileButton] gives changes when this does.
  final Listenable redraw;

  /// what to call the library until it says, or if it has no name.
  final String fallbackName;

  /// the library's link, to hand on, when there is one.
  final String? copyLink;

  /// where the libraries are listed, on a screen that lists them: the way
  /// starts there, and an arrow leads back a level at a time.
  final VoidCallback? onRoot;

  /// takes the library off a screen that shows one at a time.
  final VoidCallback? onClose;

  @override
  State<LibraryBrowser> createState() => _LibraryBrowserState();
}

class _LibraryBrowserState extends State<LibraryBrowser> {
  late final LibraryListing _listing = LibraryListing(widget.browse);

  // the folder being shown: the names on the way to it from the top.
  List<String> _path = const [];

  @override
  void initState() {
    super.initState();
    unawaited(_listing.read());
  }

  @override
  void dispose() {
    _listing.dispose();
    super.dispose();
  }

  void _show(List<String> path) => setState(() => _path = path);

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([widget.redraw, _listing]),
    builder: (context, _) {
      final tree = fileTreeOf(_listing.files, (file) => foldersIn(file.folder));
      // a level is there as soon as a page has named it, so one that was
      // gone into is still there; the top stands in for any that is not.
      final here = tree.at(_path) ?? tree;
      return AppCard(
        head: _head(context),
        inset: kBrowserInset,
        children: [
          if (_listing.files.isNotEmpty)
            const BrowserRow(
              content: Text('name', style: kTextCaption),
              trailing: [SizeText('size')],
            ),
          ..._folders(here),
          ..._files(context, here),
        ],
      );
    },
  );

  // the head of the card is the way here. it says something of the listing
  // only while there is something to say: that it is still arriving, or
  // why it stopped.
  Widget _head(BuildContext context) {
    final link = widget.copyLink;
    final onClose = widget.onClose;
    final status = _listing.status;
    final failed = _listing.error != null;
    return BrowserRow(
      height: kControlHeight,
      lead: _lead(),
      content: BrowserWay(parts: _parts()),
      trailing: [
        if (status != null && !failed)
          const BrowserSpinner(key: Key('library-loading')),
        if (status != null)
          Flexible(
            child: Text(
              status,
              key: Key(failed ? 'library-error' : 'library-status'),
              textAlign: TextAlign.right,
              style: kTextCaption.copyWith(
                color: failed ? kColorRed : kColorDim,
              ),
            ),
          ),
        if (failed)
          AppButton(
            key: const Key('library-refresh'),
            icon: Icons.refresh,
            label: 'refresh',
            onPressed: () => unawaited(_listing.read()),
          ),
        if (onClose != null)
          AppButton(
            key: const Key('close-library'),
            icon: Icons.close,
            label: 'close',
            compact: true,
            onPressed: onClose,
          ),
        if (link != null)
          AppButton(
            key: const Key('copy-link'),
            icon: Icons.copy,
            label: 'copy link',
            onPressed: () => unawaited(copyAndSay(context, link, 'link')),
          ),
      ],
    );
  }

  // where there is a list of libraries to go back to, an arrow one level
  // back: out of the folder, or from the top to the list. where there is
  // none, the picture of a library.
  Widget _lead() {
    final onRoot = widget.onRoot;
    if (onRoot == null) return const BrowserPicture(kLibraryIcon);
    return Tooltip(
      message: 'back',
      child: Material(
        type: MaterialType.transparency,
        child: InkResponse(
          key: const Key('back'),
          onTap: () => _path.isEmpty
              ? onRoot()
              : _show(_path.sublist(0, _path.length - 1)),
          child: const Icon(
            Icons.chevron_left,
            size: kIconSize,
            color: kColorText,
          ),
        ),
      ),
    );
  }

  // the way here: the list of libraries where there is one, the library,
  // and each folder gone into. every part but the last leads back to its
  // level.
  List<WayPart> _parts() {
    final onRoot = widget.onRoot;
    final name = _listing.name.isEmpty ? widget.fallbackName : _listing.name;
    final levels = [name, ..._path];
    return [
      if (onRoot != null)
        (
          key: const Key('path-libraries'),
          name: kLibrariesRoot,
          onPressed: onRoot,
        ),
      for (final (depth, level) in levels.indexed)
        (
          key: Key('path-$depth'),
          name: level,
          onPressed: depth == levels.length - 1
              ? null
              : () => _show(_path.sublist(0, depth)),
        ),
    ];
  }

  // a folder is its own row to press: there is nothing else to do with it.
  // it is told from a file by its picture, by coming first, and by
  // counting files for a size.
  List<Widget> _folders(FileTree<RemoteFile> here) => [
    for (final name in here.folders.keys.toList()..sort())
      OpeningRow(
        rowKey: Key('browse-folder-${[..._path, name].join(_kSeparator)}'),
        picture: Icons.folder_outlined,
        name: name,
        trailing: [SizeText(countFiles(here.folders[name]!.count))],
        onPressed: () => _show([..._path, name]),
      ),
  ];

  // a file's row starts with what can be done with it, where its picture
  // would be.
  List<Widget> _files(BuildContext context, FileTree<RemoteFile> here) => [
    for (final file in [
      ...here.files,
    ]..sort((a, b) => a.name.compareTo(b.name)))
      BrowserRow(
        key: Key('browse-file-${file.folder}$_kSeparator${file.name}'),
        height: _kRowHeight,
        lead: widget.fileButton(context, file),
        content: RowText(file.name),
        trailing: [SizeText(formatBytes(file.size))],
      ),
  ];
}

/// what the list of libraries is called, at the start of the way.
const String kLibrariesRoot = 'libraries';

/// one row of a browser's card, on its own grid: a first column a button
/// wide, with what is in it in the middle; the content; and what ends the
/// row, at the card's inset.
class BrowserRow extends StatelessWidget {
  const BrowserRow({
    super.key,
    required this.content,
    this.lead,
    this.trailing = const [],
    this.height = 0,
  });

  final Widget? lead;
  final Widget content;
  final List<Widget> trailing;

  /// how tall the row is at least; one of text alone is as tall as it.
  final double height;

  @override
  Widget build(BuildContext context) => ConstrainedBox(
    constraints: BoxConstraints(minHeight: height),
    child: Row(
      spacing: kSpaceRow,
      children: [
        SizedBox(
          width: _kLeadWidth,
          child: Center(child: lead),
        ),
        Expanded(child: content),
        ...trailing,
      ],
    ),
  );
}

/// a row that is pressed to go into what it names, a library or a folder:
/// it says so with a small arrow after the name, and has nothing else to
/// press.
class OpeningRow extends StatelessWidget {
  const OpeningRow({
    super.key,
    required this.rowKey,
    required this.picture,
    required this.name,
    required this.trailing,
    required this.onPressed,
  });

  /// the key of what is pressed.
  final Key rowKey;
  final IconData picture;
  final String name;
  final List<Widget> trailing;
  final VoidCallback onPressed;

  // ink shows on the nearest material, which the card's own colour would
  // otherwise cover.
  @override
  Widget build(BuildContext context) => Material(
    type: MaterialType.transparency,
    child: InkWell(
      key: rowKey,
      mouseCursor: SystemMouseCursors.click,
      hoverColor: kColorRaised,
      borderRadius: BorderRadius.circular(kRadiusControl),
      onTap: onPressed,
      child: BrowserRow(
        height: _kRowHeight,
        lead: BrowserPicture(picture),
        content: Row(
          spacing: kSpaceIn,
          children: [
            Flexible(child: RowText(name)),
            const BrowserPicture(Icons.chevron_right),
          ],
        ),
        trailing: trailing,
      ),
    ),
  );
}

/// a small dim picture: what a row is, or that it opens.
class BrowserPicture extends StatelessWidget {
  const BrowserPicture(this.icon, {super.key});

  final IconData icon;

  @override
  Widget build(BuildContext context) =>
      Icon(icon, size: kIconSize, color: kColorDim);
}

/// the sign that something is still arriving.
class BrowserSpinner extends StatelessWidget {
  const BrowserSpinner({super.key});

  @override
  Widget build(BuildContext context) => const SizedBox.square(
    dimension: kIconSize,
    child: CircularProgressIndicator(
      strokeWidth: _kSpinnerStroke,
      color: kColorDim,
    ),
  );
}

/// one part of the way: what it is called, and where pressing it leads,
/// which is nowhere for the part one is at.
typedef WayPart = ({Key? key, String name, VoidCallback? onPressed});

/// the way to where one is, part after part. all of it is set alike and in
/// one colour; a part that leads somewhere shows it when pointed at.
class BrowserWay extends StatelessWidget {
  const BrowserWay({super.key, required this.parts});

  final List<WayPart> parts;

  @override
  Widget build(BuildContext context) => Wrap(
    key: const Key('path'),
    spacing: kSpaceIn,
    crossAxisAlignment: WrapCrossAlignment.center,
    children: [
      for (final (index, part) in parts.indexed) ...[
        if (index > 0)
          Text(_kSeparator, style: kTextName.copyWith(color: kColorDim)),
        if (part.onPressed case final onPressed?)
          _WayLink(linkKey: part.key, name: part.name, onPressed: onPressed)
        else
          Text(part.name, style: kTextName),
      ],
    ],
  );
}

// a part of the way that leads somewhere: underlined while pointed at.
class _WayLink extends StatefulWidget {
  const _WayLink({
    required this.linkKey,
    required this.name,
    required this.onPressed,
  });

  /// the key of what is pressed.
  final Key? linkKey;
  final String name;
  final VoidCallback onPressed;

  @override
  State<_WayLink> createState() => _WayLinkState();
}

class _WayLinkState extends State<_WayLink> {
  bool _over = false;

  @override
  Widget build(BuildContext context) => Material(
    type: MaterialType.transparency,
    child: InkWell(
      key: widget.linkKey,
      onTap: widget.onPressed,
      onHover: (over) => setState(() => _over = over),
      mouseCursor: SystemMouseCursors.click,
      hoverColor: Colors.transparent,
      child: Text(
        widget.name,
        style: kTextName.copyWith(
          decoration: _over ? TextDecoration.underline : TextDecoration.none,
        ),
      ),
    ),
  );
}
