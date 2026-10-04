// what the core asks of a browser, said in the browser's own language:
// these calls are promises over handles and streams, and each is a line
// here where it would be a page of bindings in rust. where downloads are
// kept is in opfs.js and indexed.js.

const NOT_SECURE = 'the page is not a secure context';
const NO_FILE_SYSTEM = 'this browser has no origin private file system';
const NO_LOCKS = 'this browser has no web locks';
const FILE_SYSTEM_REFUSED = 'the origin private file system is closed to this page';

// the bytes of a file the user picked, from `start` up to `end`.
export async function readFile(file, start, end) {
  return new Uint8Array(await file.slice(start, end).arrayBuffer());
}

// why downloads cannot be kept as opfs.js keeps them, or null when they
// can. a page served over plain http has neither api, and a private window
// may have the file system and refuse to open it.
export async function opfsLack() {
  if (!globalThis.isSecureContext) return NOT_SECURE;
  if (!navigator.storage?.getDirectory) return NO_FILE_SYSTEM;
  if (!navigator.locks) return NO_LOCKS;
  try {
    await navigator.storage.getDirectory();
  } catch (_) {
    return FILE_SYSTEM_REFUSED;
  }
  return null;
}
