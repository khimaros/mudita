// the window of mudita with dana, its library: a bar with the transfers
// button and whether the server is connected, a switch between searching,
// other people's libraries, downloads, and the user's own libraries, the
// cards of the side that is showing, and the log in a bar along the bottom.

import 'package:flutter/material.dart';

import '../../core/log_history.dart';
import '../../ui/app_frame.dart';
import '../../ui/app_shell.dart';
import '../../ui/core_gate.dart';
import '../../ui/parts.dart';
import '../../ui/share_card.dart';
import '../desktop.dart';
import '../library_model.dart';
import 'discover_side.dart';
import 'downloads_side.dart';
import 'library_list.dart';
import 'search_side.dart';
import 'shared_side.dart';

const String _kDiscoverKey = 'side-discover';

// the sides in the order of the switch; the list in `build` follows it.
const List<({String key, IconData icon, String label})> _kSides = [
  (key: 'side-search', icon: Icons.search, label: 'search'),
  (key: _kDiscoverKey, icon: Icons.local_library_outlined, label: 'browse'),
  (key: 'side-downloads', icon: kIconDownload, label: kLabelDownload),
  (key: 'side-shared', icon: kIconShare, label: kLabelShare),
];

class LibraryHome extends StatefulWidget {
  const LibraryHome({
    super.key,
    required this.model,
    required this.history,
    this.desktop = kDesktop,
  });

  final LibraryModel model;

  /// the log so far, kept by whoever reached the server. it has the
  /// server's own lines when this app started the server.
  final LogHistory history;
  final Desktop desktop;

  @override
  State<LibraryHome> createState() => _LibraryHomeState();
}

class _LibraryHomeState extends State<LibraryHome> {
  int _side = 0;

  // the library being browsed on the discover side, if any, and how many
  // have been opened: each opening starts at the library's top.
  Opened? _opened;
  int _opening = 0;

  // whether the panel of transfers is open.
  final _panel = ValueNotifier(false);

  @override
  void dispose() {
    _panel.dispose();
    super.dispose();
  }

  void _open(Opened? opened) => setState(() {
    _opened = opened;
    _opening++;
  });

  // a library link given on another side is opened where libraries are.
  void _openLibrary(String link) {
    _open(openedByLink(link));
    setState(
      () => _side = _kSides.indexWhere((side) => side.key == _kDiscoverKey),
    );
  }

  @override
  Widget build(BuildContext context) {
    final LibraryHome(:model, :desktop) = widget;
    final sides = [
      SearchSide(model: model, desktop: desktop, onStarted: _showTransfers),
      DiscoverSide(
        model: model,
        desktop: desktop,
        opened: _opened,
        opening: _opening,
        onOpened: _open,
        onStarted: _showTransfers,
      ),
      DownloadsSide(
        model: model,
        onLibrary: _openLibrary,
        onStarted: _showTransfers,
      ),
      SharedSide(model: model, desktop: desktop),
    ];
    return AppFrame(
      history: widget.history,
      chip: ConnectionChip(
        listenable: model,
        ready: () => model.ready,
        error: () => model.error,
      ),
      width: kMainWidth,
      panel: _panel,
      redraw: model,
      downloads: () => [
        for (final download in model.downloads)
          downloadTransfer(model, desktop, download),
      ],
      uploads: () => model.uploads.map(uploadTransfer).toList(),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SegmentSwitch(
            segments: _kSides,
            selected: _side,
            onChanged: (side) => setState(() => _side = side),
          ),
          for (final (index, side) in sides.indexed)
            _kept(showing: index == _side, child: side),
        ],
      ),
    );
  }

  // a download asked for is a transfer, and the panel is opened for it to
  // be seen there.
  void _showTransfers() => _panel.value = true;

  // every side stays built, so that what one holds (a search and its
  // results, a library being browsed) is still there on coming back to it.
  Widget _kept({required bool showing, required Widget child}) => Offstage(
    offstage: !showing,
    child: Padding(
      padding: const EdgeInsets.only(top: kSpaceCard),
      child: child,
    ),
  );
}
