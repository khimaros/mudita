// loads the mudita core, starts its node, and leaves the core's exports on
// window.muditaCore for test_wasm.py. window.muditaPage holds what a test
// would otherwise spell out each time: playwright passes values, and a file
// is not one, so bytes cross as base64.

import init, * as core from '/wasm/mudita.js';

// two pages in one browser context share its storage, so each is given a
// node of its own with `?namespace=`.
const NAMESPACE =
  new URLSearchParams(location.search).get('namespace') ?? 'mudita-e2e';
// where the core keeps downloads (see the wasm crate's opfs.js, indexed.js).
const DOWNLOAD_FOLDERS = 'mudita-downloads';
const DOWNLOAD_DATABASES = 'mudita-download-';

const logs = [];

// one control request: its result, or its error thrown.
async function ask(method, params = {}) {
  const line = JSON.stringify({ id: 1, method, params });
  const response = JSON.parse(await core.call(line));
  if ('error' in response) throw new Error(response.error);
  return response.result;
}

function shareBase64(name, encoded) {
  const bytes = Uint8Array.from(atob(encoded), (char) => char.charCodeAt(0));
  return core.shareFile(new File([bytes], name));
}

// a finished download's name and bytes.
async function downloaded(id) {
  const file = await core.downloadedFile(id);
  const url = await new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
  return { name: file.name, encoded: url.slice(url.indexOf(',') + 1) };
}

// what downloads have in the browser's storage, of any page's: the names
// of their folders or of their databases.
async function kept() {
  if (core.storageKind() === 'indexeddb') {
    const names = (await indexedDB.databases()).map((held) => held.name);
    return names.filter((name) => name.startsWith(DOWNLOAD_DATABASES));
  }
  const origin = await navigator.storage.getDirectory();
  const root = await origin.getDirectoryHandle(DOWNLOAD_FOLDERS, {
    create: true,
  });
  const folders = [];
  for await (const folder of root.keys()) folders.push(folder);
  return folders;
}

window.muditaPage = { ask, logs, shareBase64, downloaded, kept };
try {
  await init();
  core.setLogHandler((level, target, message) =>
    logs.push({ level, target, message }),
  );
  await core.start(NAMESPACE);
  window.muditaCore = core;
} catch (failure) {
  window.muditaFailure = String(failure);
}
