// what both of mudita's entrypoints, bindu (one file) and dana (the
// library), set up alike before showing their own screens: the root logger
// and the look. the look is one set of measures, named here so that no
// widget carries a number of its own.

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:logging/logging.dart';

// space: inside a control, between rows and columns, around and between
// cards, and around the page.
const double kSpaceIn = 8;
const double kSpaceRow = 13;
const double kSpaceCard = 21;
const double kSpacePage = 34;

// corners: containers, controls, bars.
const double kRadiusBox = 13;
const double kRadiusControl = 8;
const double kRadiusBar = 4;

// heights: every button, field and chip; the send/receive switch; the bar
// along the top; the log bar along the bottom.
const double kControlHeight = 34;
const double kNavHeight = 42;
const double kTopBarHeight = 60;
const double kStatusHeight = 44;

// the row grid: [lead] [content] [action], and a right-aligned size.
const double kLeadWidth = 26;
const double kActionWidth = 126;
const double kSizeWidth = 89;

// keeps the cards readable on a wide desktop window: the one-file page, and
// the library's window, whose rows hold more.
const double kColumnWidth = 680;
const double kMainWidth = 890;

// the button in the top bar that sums up the transfers, the panel it
// opens down the side of the window, and a worded button in that panel,
// which is narrower than a row's action so that a row fits.
const double kTransfersButtonWidth = 178;
const double kPanelWidth = 520;
const double kPanelButtonWidth = 104;

// the place a file is dropped or chosen, while nothing is shared.
const double kDropHeight = 233;

const double kIconSize = 18;
const double kIconBig = 34;

// the two sides both apps have are called and drawn the same in each.
const String kLabelDownload = 'download';
const String kLabelShare = 'share';
const IconData kIconDownload = Icons.download;
const IconData kIconShare = Icons.upload;
const double kDotSize = 8;
const double kProgressHeight = 8;
const double kLineWidth = 1;
const double kDashLength = 4;
const double kDisabledOpacity = 0.5;

// the logo: a ring and a dot in a tile the size of a control.
const double kLogoRingInset = 6;
const double kLogoRingWidth = 2;
const double kLogoDotInset = 12;

// matches the gnome/libadwaita default "blue" accent (#3584e4).
const Color kColorAccent = Color(0xFF3584E4);
const Color kColorAccentSoft = Color(0x293584E4);
const Color kColorAccentRing = Color(0x8C3584E4);
// what fills the transfers button as far as the transfers have got, and
// what dims the page behind the panel.
const Color kColorAccentFill = kColorAccentRing;
const Color kColorShade = Color(0x9908090C);
const Color kColorBg = Color(0xFF121318);
const Color kColorSurface = Color(0xFF1B1C22);
// the bar across the top of a card that has one, a shade off the card.
const Color kColorHead = Color(0xFF22242C);
const Color kColorRaised = Color(0xFF262830);
const Color kColorWell = Color(0xFF0C0D11);
const Color kColorLine = Color(0xFF2F313B);
const Color kColorLineStrong = Color(0xFF454857);
const Color kColorText = Color(0xFFE6E6EA);
const Color kColorDim = Color(0xFF9A9CAB);
const Color kColorOnAccent = Color(0xFFFFFFFF);
const Color kColorGreen = Color(0xFF4CAF7A);
const Color kColorAmber = Color(0xFFE0A030);
const Color kColorRed = Color(0xFFE5626A);

const double _kLineHeight = 21;
const String _kMonoFamily = 'monospace';

const TextStyle kTextBody = TextStyle(
  color: kColorText,
  fontSize: 14,
  height: _kLineHeight / 14,
);
const TextStyle kTextTitle = TextStyle(
  color: kColorText,
  fontSize: 21,
  fontWeight: FontWeight.w600,
);
const TextStyle kTextName = TextStyle(
  color: kColorText,
  fontSize: 16,
  height: _kLineHeight / 16,
  fontWeight: FontWeight.w600,
);
const TextStyle kTextCaption = TextStyle(
  color: kColorDim,
  fontSize: 13,
  height: _kLineHeight / 13,
);
const TextStyle kTextMono = TextStyle(
  color: kColorText,
  fontFamily: _kMonoFamily,
  fontSize: 13,
  height: _kLineHeight / 13,
);

// what is left of a control's height around one line of text.
const double kControlPadding = (kControlHeight - _kLineHeight) / 2;

/// configure the root logger every logger in the app reports to. the ui's
/// log section listens there for itself; a debug build also echoes every
/// record, diagnostics included, to the console.
void setUpLogging() {
  Logger.root.level = kDebugMode ? Level.ALL : Level.INFO;
  if (!kDebugMode) return;
  Logger.root.onRecord.listen(
    (record) => debugPrint(
      '${record.time.toIso8601String()} ${record.level.name} '
      '${record.loggerName}: ${record.message}',
    ),
  );
}

ThemeData appTheme(Brightness brightness) => ThemeData(
  colorScheme: ColorScheme.fromSeed(
    seedColor: kColorAccent,
    brightness: brightness,
  ),
  useMaterial3: true,
);
