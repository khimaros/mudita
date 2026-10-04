// THE PLAIN-HTTP WORKAROUND: downloads in indexeddb, for a page that is not
// a secure context and so has neither the origin private file system nor
// web locks (see opfs.js). veilid cannot be reached from an https page
// today, so the app is served over plain http and this is what runs there.
// once it can be, remove this file and indexed.rs and keep every download
// as opfs.js does.
//
// a download is a database of its own, holding one store of blobs: a chunk
// each (see the core's chunked.rs for why chunks), under the chunk's
// number. the browser keeps a stored blob on disk, so a slice of one is
// read without the rest, and a file made of them is not held in memory.

const STORE = 'chunks';
const FULL_ERROR = 'QuotaExceededError';
const FULL = "the browser's storage for this site is full";
const ABORTED = 'the browser gave the write up';
const NOT_OPEN = 'that download is not kept here';
const NOT_WRITTEN = 'that chunk has not been written';

// the databases this page holds open, by name, each for as long as its
// download is kept: an open database is what another tab's sweep cannot
// delete (see sweepDownloads), so one is opened once and never reopened.
const held = new Map();

function failure(error) {
  if (error?.name === FULL_ERROR) return new Error(FULL);
  return error ?? new Error(ABORTED);
}

function settled(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(failure(request.error));
  });
}

// what `work` asks of a download's store, once its transaction is through.
// a write that does not fit on the disk may fail only as the transaction
// is committed, after its request has succeeded, so it is the transaction
// that is waited for.
async function transact(name, mode, work) {
  const database = await held.get(name);
  if (!database) throw new Error(NOT_OPEN);
  return new Promise((resolve, reject) => {
    const transaction = database.transaction(STORE, mode);
    const request = work(transaction.objectStore(STORE));
    transaction.oncomplete = () => resolve(request.result);
    transaction.onabort = () => reject(failure(transaction.error));
  });
}

export async function openDownload(name) {
  const request = indexedDB.open(name);
  request.onupgradeneeded = () => request.result.createObjectStore(STORE);
  held.set(name, settled(request));
  await held.get(name);
}

export function storeChunk(name, chunk, bytes) {
  return transact(name, 'readwrite', (store) =>
    store.put(new Blob([bytes]), chunk),
  );
}

export async function loadChunk(name, chunk, start, end) {
  const blob = await transact(name, 'readonly', (store) => store.get(chunk));
  if (!blob) throw new Error(NOT_WRITTEN);
  return new Uint8Array(await blob.slice(start, end).arrayBuffer());
}

// a finished download as one file, made of its chunks where the browser
// keeps them.
export async function joinStored(name, count, fileName) {
  const blobs = await transact(name, 'readonly', (store) => store.getAll());
  if (blobs.length !== count) throw new Error(NOT_WRITTEN);
  return new File(blobs, fileName);
}

export async function dropDownload(name) {
  const opened = held.get(name);
  held.delete(name);
  try {
    (await opened)?.close();
    await settled(indexedDB.deleteDatabase(name));
  } catch (_) {
    // never opened, or already gone, which is all that was wanted.
  }
}

// drop the downloads of pages that are gone: every database named with
// `prefix`. one that a live tab holds open is not deleted until that tab
// closes it or goes away, so the downloads other tabs are making are left
// alone, and none of this is waited for. what this page itself still holds
// is from a node it has stopped.
export async function sweepDownloads(prefix) {
  await Promise.all([...held.keys()].map(dropDownload));
  try {
    for (const { name } of await indexedDB.databases()) {
      if (name.startsWith(prefix)) indexedDB.deleteDatabase(name);
    }
  } catch (_) {
    // a browser that cannot list its databases keeps what was left behind.
  }
}
