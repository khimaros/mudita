// the recent records of the app's own loggers, kept for the ui's log section.
// seeders and fetchers log through package:logging; this listens at the root.

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:logging/logging.dart';

/// the parent of every logger the app creates (`mudita.share`, ...).
const String kLogName = 'mudita';

// enough to scroll back through a long transfer without growing forever.
const int kMaxLogRecords = 200;

// below this a record is a diagnostic for developers, not news for the user.
const Level kShownLevel = Level.INFO;

/// whether [record] belongs in the log the user sees.
bool isShown(LogRecord record) =>
    record.level >= kShownLevel &&
    (record.loggerName == kLogName ||
        record.loggerName.startsWith('$kLogName.'));

class LogHistory extends ChangeNotifier {
  LogHistory() {
    _sub = Logger.root.onRecord.where(isShown).listen(_add);
  }

  late final StreamSubscription<LogRecord> _sub;
  final List<LogRecord> _records = [];

  /// oldest first.
  List<LogRecord> get records => List.unmodifiable(_records);

  void _add(LogRecord record) {
    _records.add(record);
    if (_records.length > kMaxLogRecords) _records.removeAt(0);
    notifyListeners();
  }

  @override
  void dispose() {
    unawaited(_sub.cancel());
    super.dispose();
  }
}
