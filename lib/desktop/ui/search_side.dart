// finding files in other libraries: a search across every library the
// server has discovered, filling in as they answer, with a card for each
// file found.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../core/share_link.dart';
import '../../ui/app_shell.dart';
import '../../ui/parts.dart';
import '../desktop.dart';
import '../labels.dart';
import '../library_model.dart';
import 'pieces.dart';

class SearchSide extends StatefulWidget {
  const SearchSide({
    super.key,
    required this.model,
    required this.desktop,
    required this.onStarted,
  });

  final LibraryModel model;
  final Desktop desktop;

  /// told when a download is asked for.
  final VoidCallback onStarted;

  @override
  State<SearchSide> createState() => _SearchSideState();
}

class _SearchSideState extends State<SearchSide> {
  final _input = TextEditingController();
  SearchModel? _search;

  @override
  void dispose() {
    _search?.cancel();
    _input.dispose();
    super.dispose();
  }

  // searching again is how a search is refreshed.
  void _start() {
    final query = _input.text.trim();
    if (query.isEmpty) return;
    _search?.cancel();
    setState(() => _search = widget.model.search(query));
  }

  // the hits change with the search, and whether one is downloaded with
  // the model.
  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([widget.model, _search]),
    builder: (context, _) {
      final search = _search;
      return CardStack(
        children: [
          StartRow(
            icon: Icons.search,
            busy: search != null && !search.done,
            input: _input,
            inputKey: const Key('search-input'),
            hint: 'search file names',
            onSubmitted: _start,
            actions: [
              AppButton(
                key: const Key('search'),
                icon: Icons.search,
                label: 'search',
                kind: ButtonKind.primary,
                onPressed: _start,
              ),
            ],
          ),
          if (search != null) ..._results(search),
        ],
      );
    },
  );

  List<Widget> _results(SearchModel search) {
    final error = search.error;
    if (error != null || (search.done && search.hits.isEmpty)) {
      return [
        AppCard(
          children: [
            AppRow(
              icon: error == null ? Icons.search_off : Icons.error_outline,
              iconColor: error == null ? kColorDim : kColorRed,
              content: Text(
                error ?? 'nothing found in ${search.asked} libraries',
                key: const Key('search-status'),
                style: kTextCaption.copyWith(
                  color: error == null ? kColorDim : kColorRed,
                ),
              ),
            ),
          ],
        ),
      ];
    }
    return [
      for (final hit in search.hits)
        _HitCard(
          key: Key('hit-${hit.file.root}'),
          hit: hit,
          model: widget.model,
          desktop: widget.desktop,
          onStarted: widget.onStarted,
        ),
    ];
  }
}

/// one file a search found: what it is, where it is, and a way to have it.
class _HitCard extends StatelessWidget {
  const _HitCard({
    super.key,
    required this.hit,
    required this.model,
    required this.desktop,
    required this.onStarted,
  });

  final SearchHit hit;
  final LibraryModel model;
  final Desktop desktop;
  final VoidCallback onStarted;

  @override
  Widget build(BuildContext context) {
    final file = hit.file;
    final token = file.token;
    final here = model.standing(file) == FileStanding.downloaded;
    return AppCard(
      children: [
        AppRow(
          icon: here ? Icons.check : Icons.insert_drive_file_outlined,
          iconColor: here ? kColorGreen : kColorDim,
          content: FileTitle(
            name: file.name,
            caption: hitCaption(hit, downloaded: here),
            captionKey: const Key('hit-caption'),
          ),
          actions: [
            AppButton(
              key: const Key('copy-link'),
              icon: Icons.copy,
              label: 'copy link',
              onPressed: token == null
                  ? null
                  : () => unawaited(
                      copyAndSay(
                        context,
                        linkFor(kHostedBase, token, file.name),
                        'link',
                      ),
                    ),
            ),
            FileButton(
              model: model,
              desktop: desktop,
              file: file,
              onStarted: onStarted,
              kind: ButtonKind.primary,
            ),
          ],
        ),
      ],
    );
  }
}
