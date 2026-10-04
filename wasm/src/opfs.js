// downloads in the origin private file system, a folder per download, a
// file per chunk (see the core's chunked.rs for why chunks). a folder is
// named `<session>.<n>`: the session is this page's, so that a page can
// tell its own downloads from those another tab is still making. this
// needs a secure context; a page without one keeps them as indexed.js does.

const ROOT = 'mudita-downloads';
const LOCK_PREFIX = 'mudita-session-';
const SESSION_SEPARATOR = '.';

async function root() {
  const origin = await navigator.storage.getDirectory();
  return origin.getDirectoryHandle(ROOT, { create: true });
}

async function chunkFile(folder, chunk, create) {
  const directory = await (await root()).getDirectoryHandle(folder, { create });
  return directory.getFileHandle(String(chunk), { create });
}

// a chunk is written in one go: every write to a file here costs a copy of
// the file, so a file is written once.
export async function writeChunk(folder, chunk, bytes) {
  const stream = await (await chunkFile(folder, chunk, true)).createWritable();
  await stream.write(bytes);
  await stream.close();
}

export async function readChunk(folder, chunk, start, end) {
  const file = await (await chunkFile(folder, chunk, false)).getFile();
  return new Uint8Array(await file.slice(start, end).arrayBuffer());
}

// a finished download as one file, made of its chunks where they lie on
// disk: it is never read into memory to be put together.
export async function joinChunks(folder, count, name) {
  const parts = [];
  for (let chunk = 0; chunk < count; chunk++) {
    parts.push(await (await chunkFile(folder, chunk, false)).getFile());
  }
  return new File(parts, name);
}

export async function removeFolder(folder) {
  try {
    await (await root()).removeEntry(folder, { recursive: true });
  } catch (_) {
    // already gone, which is all that was wanted.
  }
}

// mark `session` as this page's for as long as the page lives, and drop the
// downloads of sessions whose page is gone. a page holds a lock named after
// its session and never lets go, so a lock that can be had is a dead
// session's.
export async function claimSession(session) {
  navigator.locks.request(LOCK_PREFIX + session, () => new Promise(() => {}));
  for await (const folder of (await root()).keys()) {
    const owner = folder.split(SESSION_SEPARATOR)[0];
    if (owner === session) continue;
    await navigator.locks.request(
      LOCK_PREFIX + owner,
      { ifAvailable: true },
      async (lock) => {
        if (lock) await removeFolder(folder);
      },
    );
  }
}
