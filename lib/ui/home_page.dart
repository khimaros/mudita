// bindu's one screen: a bar with the transfers button and whether the core
// is connected, a switch between sending and receiving, the cards of the
// side that is showing, and the log in a bar along the bottom. a link to a
// library adds a card on the receive side listing what that library shares.

import 'dart:async';

import 'package:flutter/material.dart';

import '../core/core.dart';
import '../core/log_history.dart';
import '../core/share_link.dart';
import 'app_frame.dart';
import 'app_shell.dart';
import 'bindu_model.dart';
import 'core_gate.dart';
import 'download_card.dart';
import 'library_browser.dart';
import 'parts.dart';
import 'share_card.dart';

// what a link puts between the page's address and what it leads to.
const String _kFragmentMark = '#';

// the two sides of the screen, receiving first.
const List<({String key, IconData icon, String label})> _kSides = [
  (key: 'switch-receive', icon: kIconDownload, label: kLabelDownload),
  (key: 'switch-send', icon: kIconShare, label: kLabelShare),
];

class HomePage extends StatefulWidget {
  const HomePage({
    super.key,
    required this.core,
    required this.history,
    required this.linkBase,
    this.initialLink,
    this.links = const Stream.empty(),
    this.openFolder,
  });

  /// shows a folder to the user, where the platform can: a finished
  /// download is then shown in its own. null in a browser, where one is
  /// handed over again instead.
  final Future<void> Function(String folder)? openFolder;

  final Core core;

  /// the log so far, kept by whoever reached the core.
  final LogHistory history;
  final String linkBase;

  /// the link the app was opened with (the page's own url on web), to a
  /// file or to a library; a bare token is taken too.
  final String? initialLink;

  /// links that arrive while the app is open (one pasted into the tab).
  final Stream<String> links;

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> {
  late final BinduModel _model = BinduModel(widget.core);
  StreamSubscription<String>? _linkSub;
  StreamSubscription<Drop>? _dropSub;

  // which side of the switch is showing.
  bool _receiving = false;

  // the library being shown, when a link to one was opened.
  String? _library;

  // whether the panel of transfers is open.
  final _panel = ValueNotifier(false);

  @override
  void initState() {
    super.initState();
    _model.start();
    _open(widget.initialLink ?? '');
    _linkSub = widget.links.listen(_open);
    _dropSub = widget.core.drops?.listen(_dropped);
  }

  @override
  void dispose() {
    unawaited(_linkSub?.cancel());
    unawaited(_dropSub?.cancel());
    _model.dispose();
    _panel.dispose();
    super.dispose();
  }

  // a link to a library is shown as one; a link to a file is looked up, and
  // downloaded only when the user says so. either brings the receive side
  // forward. false when [input] is no link at all.
  bool _open(String input) {
    final library = libraryLinkFromInput(input);
    final file = fileLinkFromInput(input);
    if (library == null && file == null) return false;
    setState(() {
      _receiving = true;
      _library = library ?? _library;
    });
    if (file != null) unawaited(_model.open(file));
    return true;
  }

  // a file dropped anywhere on the page is sent, whichever side is showing,
  // unless it came with others or one is being sent already.
  void _dropped(Drop drop) {
    final refusal = drop.count != 1
        ? 'one file at a time'
        : _model.sharing
        ? 'stop sharing the current file first'
        : null;
    if (refusal != null) return say(ScaffoldMessenger.of(context), refusal);
    setState(() => _receiving = false);
    unawaited(_model.shareFrom(drop.share));
  }

  @override
  Widget build(BuildContext context) => AppFrame(
    history: widget.history,
    chip: ConnectionChip(
      listenable: _model,
      ready: () => _model.ready,
      error: () => _model.error,
    ),
    width: kColumnWidth,
    panel: _panel,
    redraw: _model,
    downloads: () => [
      for (final item in _model.incoming)
        if (item.started)
          incomingTransfer(_model, item, openFolder: widget.openFolder),
    ],
    uploads: () => _model.uploads.map(uploadTransfer).toList(),
    child: ListenableBuilder(
      listenable: _model,
      builder: (context, _) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SegmentSwitch(
            segments: _kSides,
            selected: _receiving ? 0 : 1,
            onChanged: (side) => setState(() => _receiving = side == 0),
          ),
          _side(showing: !_receiving, children: _send()),
          _side(showing: _receiving, children: _receive()),
        ],
      ),
    ),
  );

  // both sides stay built, so that what one holds (a half-typed link, a
  // library's listing) is still there on coming back to it.
  Widget _side({required bool showing, required List<Widget> children}) =>
      Offstage(
        offstage: !showing,
        child: Padding(
          padding: const EdgeInsets.only(top: kSpaceCard),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            spacing: kSpaceCard,
            children: children,
          ),
        ),
      );

  // one file is sent at a time: the place to add one gives way to its
  // card. who is downloading it is in the panel of transfers.
  List<Widget> _send() {
    final share = _model.share;
    return [
      if (share == null)
        AddFileArea(
          droppable: widget.core.drops != null,
          onPick: _model.picking ? null : () => unawaited(_model.pickFile()),
        )
      else
        ShareCard(
          share: share,
          linkBase: widget.linkBase,
          onStop: () => unawaited(_model.stopSharing()),
        ),
    ];
  }

  // where a link is given, then what the links given led to and wait to be
  // asked for, then the library. one asked for is a transfer, and the panel
  // is opened for it to be seen there.
  List<Widget> _receive() {
    final waiting = _model.incoming.where((item) => !item.started).toList();
    return [
      AddLinkRow(onOpen: _open),
      if (waiting.isNotEmpty)
        PendingCard(model: _model, items: waiting, onStarted: _showTransfers),
      if (_library case final library?) _libraryBrowser(library),
    ];
  }

  void _showTransfers() => _panel.value = true;

  // the library a link led to, in the browser the library's window has,
  // without what belongs to a list of libraries: there is none here to go
  // back to, so it starts at the library and is closed, not left. a file
  // picked out of it is downloaded at once, and shows among the transfers.
  Widget _libraryBrowser(String link) => LibraryBrowser(
    key: ValueKey(link),
    browse: (offset) => _model.browse(link, offset),
    redraw: _model,
    copyLink: '${widget.linkBase}$_kFragmentMark$link',
    onClose: () => setState(() => _library = null),
    fileButton: (context, file) {
      final path = '${file.folder}/${file.name}';
      return _model.has(file)
          ? AppButton(
              key: Key('catalog-downloading-$path'),
              icon: Icons.downloading,
              label: 'downloading',
              compact: true,
              onPressed: null,
            )
          : AppButton(
              key: Key('catalog-download-$path'),
              icon: Icons.download,
              label: 'download',
              compact: true,
              onPressed: () {
                unawaited(_model.downloadFrom(file));
                _showTransfers();
              },
            );
    },
  );
}
