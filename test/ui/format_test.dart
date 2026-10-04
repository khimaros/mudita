import 'package:flutter_test/flutter_test.dart';
import 'package:mudita/ui/format.dart';

void main() {
  test('progress is a whole percentage and the pieces behind it', () {
    expect(formatProgress(0, 12), '0% (0 of 12 pieces)');
    expect(formatProgress(5, 12), '41% (5 of 12 pieces)');
    expect(formatProgress(12, 12), '100% (12 of 12 pieces)');
  });

  test('a download is not called complete before its last piece', () {
    expect(formatProgress(999, 1000), startsWith('99%'));
  });

  test('a file with no pieces to wait for is whole', () {
    expect(formatProgress(0, 0), '100% (0 of 0 pieces)');
  });

  test('sizes use binary units, and whole bytes no decimals', () {
    expect(formatBytes(999), '999 B');
    expect(formatBytes(30720), '30.0 KiB');
  });

  test('a moment is shown to the minute', () {
    final shown = formatDateTime(DateTime(2026, 10, 5, 14, 7, 59));
    expect(shown, '2026-10-05 14:07');
  });
}
