// the links the app hands out and takes in (DESIGN.md "share token and
// link", "library record and link"). the token rides in the url fragment,
// which a browser never sends to the server hosting the app. the core reads
// links itself; the app only needs to build them for the page it runs on,
// and to tell a link to a library from a link to a file.

const String _kFragmentMark = '#';
const String _kNameMark = '/';
const String _kLibraryPrefix = 'library/';

/// where the web app is hosted; the base of links made by native builds.
const String kHostedBase = 'http://veilid.tech/mudita/';

/// the url share links are built on. a web build passes the [page] it is
/// running in, so links made on a dev or e2e server point back at that
/// server; a native build has no page and uses the hosted app.
String linkBaseFor(Uri? page) {
  if (page == null) return kHostedBase;
  return Uri(
    scheme: page.scheme,
    host: page.host,
    port: page.hasPort ? page.port : null,
    path: page.path,
  ).toString();
}

/// the share link for [token] under [base], carrying [name] so that whoever
/// opens it sees what the file is called. an empty name is left out.
String linkFor(String base, String token, [String name = '']) {
  final named = name.isEmpty ? '' : '$_kNameMark${Uri.encodeComponent(name)}';
  return '$base$_kFragmentMark${Uri.encodeComponent(token)}$named';
}

/// the link to the whole library behind [token], under [base].
String libraryLinkFor(String base, String token) =>
    '$base$_kFragmentMark$_kLibraryPrefix${Uri.encodeComponent(token)}';

// what [input] carries after any url: the fragment of a link, or the input
// itself. null for blank input or a url with no fragment.
String? _payload(String input) {
  final text = input.trim();
  final mark = text.indexOf(_kFragmentMark);
  final payload = mark < 0 ? text : text.substring(mark + 1);
  final looksLikeUrl = mark < 0 && text.contains('://');
  return payload.isEmpty || looksLikeUrl ? null : payload;
}

/// what to ask the core about, when [input] is a share link, a token, or a
/// token and a name: the part that names the file, still encoded as it
/// came. null when there is nothing to use: blank input, a url with no
/// fragment, or a link to a library, which is no file.
String? fileLinkFromInput(String input) {
  final payload = _payload(input);
  return payload == null || payload.startsWith(_kLibraryPrefix)
      ? null
      : payload;
}

/// the library link in [input], as the core takes it: `library/` and the
/// token. null for anything else.
String? libraryLinkFromInput(String input) {
  final payload = _payload(input);
  if (payload == null || !payload.startsWith(_kLibraryPrefix)) return null;
  return payload.length == _kLibraryPrefix.length ? null : payload;
}
