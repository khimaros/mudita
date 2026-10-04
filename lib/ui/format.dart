// small pure helpers for showing numbers to people.

const int _kUnitStep = 1024;
const List<String> _kUnits = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];

/// a byte count in binary units, e.g. `30.0 KiB`. whole bytes get no decimals.
String formatBytes(int bytes) {
  var value = bytes.toDouble();
  var unit = 0;
  while (value >= _kUnitStep && unit < _kUnits.length - 1) {
    value /= _kUnitStep;
    unit++;
  }
  return '${value.toStringAsFixed(unit == 0 ? 0 : 1)} ${_kUnits[unit]}';
}

/// a count of files, e.g. `1 file`, `12 files`.
String countFiles(int count) => count == 1 ? '1 file' : '$count files';

String _twoDigits(int value) => value.toString().padLeft(2, '0');

/// a moment as its local date and time to the minute, e.g. `2026-10-05 14:07`.
String formatDateTime(DateTime moment) {
  final local = moment.toLocal();
  final date = [local.year, local.month, local.day].map(_twoDigits).join('-');
  return '$date ${formatTime(local)}';
}

/// a moment as its local time of day to the minute, e.g. `14:07`.
String formatTime(DateTime moment) {
  final local = moment.toLocal();
  return '${_twoDigits(local.hour)}:${_twoDigits(local.minute)}';
}

/// a 0..1 fraction as a whole percentage, e.g. `42%`.
String formatPercent(double fraction) => '${(fraction * 100).floor()}%';

/// how much of [total] is [done], as 0..1. a file with no pieces to wait
/// for is whole.
double fractionOf(int done, int total) => total == 0 ? 1 : done / total;

/// how far along a download is, e.g. `41% (5 of 12 pieces)`.
String formatProgress(int done, int total) =>
    '${formatPercent(fractionOf(done, total))} ($done of $total pieces)';
