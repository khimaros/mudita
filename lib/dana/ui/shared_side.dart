// the user's own libraries: a row that starts a new one from a folder, and
// a card for each. one card is open at a time, showing whether the library
// is discoverable, its folders, and its files as the tree they are on disk,
// each with a link of its own.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../core/file_tree.dart';
import '../../core/share_link.dart';
import '../../ui/app_shell.dart';
import '../../ui/format.dart';
import '../../ui/parts.dart';
import '../../ui/transfers_panel.dart';
import '../desktop.dart';
import '../labels.dart';
import '../library_model.dart';
import 'pieces.dart';

// the library downloads go to is called this unless the user renames it.
const String _kDownloadsName = 'downloads';

// how far a tree row is set in for each folder it is under: an arrow and
// the gap after it.
const double _kTreeStep = kIconSize + kSpaceIn;

class SharedSide extends StatefulWidget {
  const SharedSide({super.key, required this.model, required this.desktop});

  final LibraryModel model;
  final Desktop desktop;

  @override
  State<SharedSide> createState() => _SharedSideState();
}

class _SharedSideState extends State<SharedSide> {
  final _name = TextEditingController();

  // the library whose card is open, and the one being renamed, by name.
  String? _open;
  String? _renaming;

  // the folders of the tree that are open, each by its library and path.
  final Set<String> _openFolders = {};

  LibraryModel get _model => widget.model;

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  Future<void> _create() async {
    final folder = await widget.desktop.pickFolder();
    if (folder == null) return;
    await _model.addLibrary(_name.text.trim(), folder);
    _name.clear();
  }

  Future<void> _addFolder(String library) async {
    final folder = await widget.desktop.pickFolder();
    if (folder != null) await _model.addFolder(library, folder);
  }

  // the open card and the tree's open folders are kept by name, so they
  // follow the library to its new one.
  Future<void> _rename(String name, String to) async {
    setState(() => _renaming = null);
    if (to.isEmpty || to == name) return;
    await _model.renameLibrary(name, to);
    if (!mounted || !_model.libraries.any((library) => library.name == to)) {
      return;
    }
    setState(() {
      if (_open == name) _open = to;
      final moved = _openFolders
          .where((key) => key.startsWith('$name\n'))
          .toList();
      _openFolders
        ..removeAll(moved)
        ..addAll(moved.map((key) => '$to${key.substring(name.length)}'));
    });
  }

  Future<void> _stopSharing(String name) async {
    final sure = await showDialog<bool>(
      context: context,
      builder: (context) => _ConfirmStop(name: name),
    );
    if (sure ?? false) await _model.removeLibrary(name);
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _model,
    builder: (context, _) => CardStack(
      children: [
        StartRow(
          icon: Icons.add,
          input: _name,
          inputKey: const Key('new-library-name'),
          hint: 'name a new library',
          onSubmitted: () => unawaited(_create()),
          actions: [
            AppButton(
              key: const Key('rescan'),
              icon: Icons.refresh,
              tooltip: 'look for changes in the folders now',
              onPressed: () => unawaited(_model.rescan()),
            ),
            AppButton(
              key: const Key('choose-folder'),
              icon: Icons.folder_outlined,
              label: 'choose folder',
              onPressed: () => unawaited(_create()),
            ),
          ],
        ),
        for (final library in _model.libraries)
          AppCard(
            key: Key('own-${library.name}'),
            children: [
              _heading(library),
              if (_open == library.name) ..._opened(library),
            ],
          ),
      ],
    ),
  );

  // the library downloads go to cannot be taken away, so that is not
  // offered, and its pencil stands next to "copy link".
  Widget _heading(OwnLibrary library) {
    final name = library.name;
    final token = library.token;
    final kept =
        name == _kDownloadsName ||
        library.folders.contains(_model.downloadFolder);
    return InkWell(
      key: Key('toggle-$name'),
      onTap: () => setState(() => _open = _open == name ? null : name),
      child: AppRow(
        icon: _open == name ? Icons.expand_more : Icons.chevron_right,
        content: _renaming == name ? _renameField(name) : _title(library),
        actions: [
          AppButton(
            key: const Key('rename'),
            icon: Icons.edit_outlined,
            tooltip: 'rename',
            onPressed: () => setState(() => _renaming = name),
          ),
          if (!kept)
            AppButton(
              key: const Key('stop-sharing'),
              icon: Icons.close,
              label: 'stop sharing',
              kind: ButtonKind.danger,
              onPressed: () => unawaited(_stopSharing(name)),
            ),
          AppButton(
            key: const Key('copy-library-link'),
            icon: Icons.copy,
            label: 'copy link',
            kind: ButtonKind.primary,
            // there is no link until the server has published the record.
            onPressed: token == null
                ? null
                : () => unawaited(
                    copyAndSay(
                      context,
                      libraryLinkFor(kHostedBase, token),
                      'library link',
                    ),
                  ),
          ),
        ],
      ),
    );
  }

  // a library someone is downloading from says so, so that it stands out.
  Widget _title(OwnLibrary library) {
    final downloading = _model.downloadingFrom(library.name);
    return FileTitle(
      name: library.name,
      caption: ownCaption(library, _model.files),
      captionKey: const Key('own-caption'),
      note: downloading == 0 ? null : transfersLabel(downloading),
      noteKey: const Key('own-downloading'),
    );
  }

  // the name edited where it stands: enter keeps it, leaving the field
  // does not.
  Widget _renameField(String name) => TextFormField(
    key: const Key('rename-input'),
    initialValue: name,
    autofocus: true,
    style: kTextBody,
    cursorColor: kColorAccent,
    decoration: fieldDecoration(context),
    onFieldSubmitted: (to) => unawaited(_rename(name, to.trim())),
    onTapOutside: (_) => setState(() => _renaming = null),
  );

  List<Widget> _opened(OwnLibrary library) {
    final name = library.name;
    final mine = _model.files.where((file) => file.library == name).toList();
    final addFolder = AppButton(
      key: const Key('add-folder'),
      icon: Icons.add,
      label: 'add folder',
      onPressed: () => unawaited(_addFolder(name)),
    );
    return [
      AppRow(
        icon: Icons.visibility_outlined,
        content: const RowText('discoverable'),
        actions: [
          Switch(
            key: const Key('discoverable'),
            value: library.listed,
            activeTrackColor: kColorAccent,
            inactiveTrackColor: kColorLineStrong,
            thumbColor: const WidgetStatePropertyAll(kColorOnAccent),
            trackOutlineColor: const WidgetStatePropertyAll(Colors.transparent),
            onChanged: (listed) =>
                unawaited(_model.setListed(name, listed: listed)),
          ),
        ],
      ),
      if (library.folders.isEmpty)
        AppRow(
          icon: Icons.folder_outlined,
          content: const RowText('no folders yet', style: kTextCaption),
          actions: [addFolder],
        ),
      for (final (index, folder) in library.folders.indexed) ...[
        _folderRow(name, folder, index == 0 ? addFolder : null),
        ..._tree(
          name,
          folder,
          fileTreeOf(mine, (file) => foldersUnder(folder, file.path)),
          0,
        ),
      ],
    ];
  }

  // the folder downloads go to stays shared, so removing it is not offered.
  Widget _folderRow(String library, String folder, Widget? addFolder) => AppRow(
    icon: Icons.folder_outlined,
    content: RowText(folder, style: kTextMono),
    actions: [
      if (folder != _model.downloadFolder)
        AppButton(
          key: Key('remove-folder-$folder'),
          icon: Icons.close,
          label: 'remove',
          onPressed: () => unawaited(_model.removeFolder(library, folder)),
        ),
      ?addFolder,
    ],
  );

  // the rows of [tree], which is at [path] in the library: its folders,
  // each followed by what is in it when open, then its files.
  List<Widget> _tree(
    String library,
    String path,
    FileTree<SharedFile> tree,
    int depth,
  ) {
    final folders = tree.folders.keys.toList()..sort();
    final files = [...tree.files]..sort((a, b) => a.name.compareTo(b.name));
    return [
      for (final folder in folders)
        ...() {
          final at = '$path/$folder';
          final key = '$library\n$at';
          final open = _openFolders.contains(key);
          final inside = tree.folders[folder]!;
          return [
            _TreeRow(
              key: Key('folder-$at'),
              depth: depth,
              arrow: open ? Icons.expand_more : Icons.chevron_right,
              picture: Icons.folder_outlined,
              name: folder,
              size: countFiles(inside.count),
              onPressed: () => setState(
                () => open ? _openFolders.remove(key) : _openFolders.add(key),
              ),
            ),
            if (open) ..._tree(library, at, inside, depth + 1),
          ];
        }(),
      for (final file in files)
        _TreeRow(
          key: Key('file-${file.path}'),
          depth: depth,
          picture: Icons.insert_drive_file_outlined,
          name: file.name,
          size: formatBytes(file.size),
          action: _fileAction(file),
        ),
    ];
  }

  // a file's link has its name in it, so whoever opens it sees what it is.
  Widget _fileAction(SharedFile file) {
    final token = file.token;
    if (token == null) return ActionText(sharingLabel(file));
    return AppButton(
      key: Key('copy-link-${file.path}'),
      icon: Icons.copy,
      label: 'copy link',
      onPressed: () => unawaited(
        copyAndSay(context, linkFor(kHostedBase, token, file.name), 'link'),
      ),
    );
  }
}

/// one line of the tree: the arrow of a folder first, then the picture of
/// what it is, its name, its size, and what can be done with it.
class _TreeRow extends StatelessWidget {
  const _TreeRow({
    super.key,
    required this.depth,
    required this.picture,
    required this.name,
    required this.size,
    this.arrow,
    this.action,
    this.onPressed,
  });

  final int depth;
  final IconData? arrow;
  final IconData picture;
  final String name;
  final String size;
  final Widget? action;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) => InkWell(
    onTap: onPressed,
    child: Container(
      constraints: const BoxConstraints(minHeight: kControlHeight),
      padding: EdgeInsets.only(
        left: kLeadWidth + kSpaceRow + depth * _kTreeStep,
      ),
      child: Row(
        spacing: kSpaceIn,
        children: [
          SizedBox(
            width: kIconSize,
            child: Icon(arrow, size: kIconSize, color: kColorDim),
          ),
          Icon(picture, size: kIconSize, color: kColorDim),
          Expanded(child: RowText(name)),
          // the size and the action stand where they do in every other row.
          Padding(
            padding: const EdgeInsets.only(right: kSpaceRow - kSpaceIn),
            child: SizeText(size),
          ),
          SizedBox(
            width: kActionWidth,
            child: Align(alignment: Alignment.centerRight, child: action),
          ),
        ],
      ),
    ),
  );
}

/// asks before a library stops being shared: its link is forgotten with it.
class _ConfirmStop extends StatelessWidget {
  const _ConfirmStop({required this.name});

  final String name;

  @override
  Widget build(BuildContext context) => AlertDialog(
    backgroundColor: kColorSurface,
    shape: RoundedRectangleBorder(
      borderRadius: BorderRadius.circular(kRadiusBox),
      side: const BorderSide(color: kColorLine),
    ),
    content: Text(
      'stop sharing "$name"? its link is forgotten. the files stay where '
      'they are.',
      key: const Key('confirm-text'),
      style: kTextBody,
    ),
    actions: [
      AppButton(
        key: const Key('confirm-cancel'),
        icon: Icons.arrow_back,
        label: 'keep sharing',
        onPressed: () => Navigator.of(context).pop(false),
      ),
      AppButton(
        key: const Key('confirm-stop'),
        icon: Icons.close,
        label: 'stop sharing',
        kind: ButtonKind.danger,
        onPressed: () => Navigator.of(context).pop(true),
      ),
    ],
  );
}
