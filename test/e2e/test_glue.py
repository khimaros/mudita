"""the wasm crate's javascript (wasm/src/*.js) in chrome, without a node.

these need a browser and no network: the modules are loaded as they are
into an empty page, on an origin that is not a secure context (as the app
is deployed, where downloads are kept in indexeddb) and on one that is
(where they are kept in the origin private file system).
"""

import pytest
from conftest import READY_TIMEOUT_MS, REPO_ROOT, open_page, site
from playwright.sync_api import Error as PlaywrightError
from playwright.sync_api import Page

GLUE_ROOT = REPO_ROOT / "wasm/src"
PREFIX = "mudita-download-"
FOLDERS = "mudita-downloads"
# a call of one of a module's exports. bytes do not cross from python, so
# an argument {"bytes": n, "of": b} stands for n bytes of b, and bytes that
# come back do so as a list.
CALL = """async ([module, name, args]) => {
    const made = args.map((arg) =>
        arg?.bytes === undefined ? arg : new Uint8Array(arg.bytes).fill(arg.of));
    const result = await (await import(`/glue/${module}.js`))[name](...made);
    if (result instanceof File) {
        const bytes = new Uint8Array(await result.arrayBuffer());
        return { name: result.name, bytes: Array.from(bytes) };
    }
    return result instanceof Uint8Array ? Array.from(result) : result;
}"""
DATABASES = f"""async () => (await indexedDB.databases())
    .map((held) => held.name).filter((name) => name.startsWith('{PREFIX}'))"""
FOLDERS_KEPT = f"""async () => {{
    const origin = await navigator.storage.getDirectory();
    const root = await origin.getDirectoryHandle('{FOLDERS}', {{ create: true }});
    const folders = [];
    for await (const folder of root.keys()) folders.push(folder);
    return folders;
}}"""
# a little under and a little over what the page's storage is cut down to.
QUOTA_SLACK_BYTES = 200_000
TOO_MUCH_BYTES = 5_000_000


@pytest.fixture(scope="session")
def empty_page(tmp_path_factory):
    root = tmp_path_factory.mktemp("glue")
    (root / "index.html").write_text("<!doctype html><title>glue</title>")
    return root


@pytest.fixture(scope="session")
def plain_url(empty_page):
    with site(empty_page, {"/glue/": GLUE_ROOT}) as url:
        yield url


@pytest.fixture(scope="session")
def secure_url(empty_page):
    with site(empty_page, {"/glue/": GLUE_ROOT}, secure=True) as url:
        yield url


def call(page: Page, module: str, name: str, *args):
    return page.evaluate(CALL, [module, name, list(args)])


def some(count: int, of: int) -> dict:
    return {"bytes": count, "of": of}


def test_only_a_secure_page_can_have_the_file_system(browser, plain_url, secure_url):
    plain = open_page(browser, plain_url)
    assert call(plain, "glue", "opfsLack") == "the page is not a secure context"
    assert call(open_page(browser, secure_url), "glue", "opfsLack") is None


def test_indexeddb_keeps_chunks_and_hands_them_back(browser, plain_url):
    page, name = open_page(browser, plain_url), PREFIX + "a.1"
    call(page, "indexed", "openDownload", name)
    # the last chunk first, as it may well be finished first.
    call(page, "indexed", "storeChunk", name, 1, some(1000, 2))
    call(page, "indexed", "storeChunk", name, 0, some(3000, 1))
    assert call(page, "indexed", "loadChunk", name, 0, 10, 15) == [1] * 5
    assert call(page, "indexed", "loadChunk", name, 1, 990, 1000) == [2] * 10
    with pytest.raises(PlaywrightError, match="has not been written"):
        call(page, "indexed", "loadChunk", name, 2, 0, 10)
    with pytest.raises(PlaywrightError, match="has not been written"):
        call(page, "indexed", "joinStored", name, 3, "short.bin")
    joined = call(page, "indexed", "joinStored", name, 2, "whole.bin")
    assert joined == {"name": "whole.bin", "bytes": [1] * 3000 + [2] * 1000}
    assert page.evaluate(DATABASES) == [name]
    call(page, "indexed", "dropDownload", name)
    assert page.evaluate(DATABASES) == []
    with pytest.raises(PlaywrightError, match="is not kept here"):
        call(page, "indexed", "loadChunk", name, 0, 0, 10)


def test_an_indexeddb_sweep_spares_what_a_live_tab_holds(browser, plain_url):
    first = open_page(browser, plain_url)
    live, other = PREFIX + "a.1", "someone-elses"
    call(first, "indexed", "openDownload", live)
    call(first, "indexed", "storeChunk", live, 0, some(100, 7))
    first.evaluate("name => { indexedDB.open(name); }", other)
    second = open_page(browser, plain_url, first.context)
    stale = PREFIX + "b.1"
    call(second, "indexed", "openDownload", stale)
    call(second, "indexed", "sweepDownloads", PREFIX)
    # the sweeping page's own are from a node it has stopped, and go at once.
    assert second.evaluate(DATABASES) == [live]
    call(first, "indexed", "storeChunk", live, 1, some(100, 8))
    assert call(first, "indexed", "loadChunk", live, 0, 0, 3) == [7] * 3
    joined = call(first, "indexed", "joinStored", live, 2, "kept.bin")
    assert joined["bytes"] == [7] * 100 + [8] * 100
    # once the first tab is gone, so is what it held, and nothing else.
    first.close()
    second.wait_for_function(
        f"({DATABASES})().then((left) => !left.length)", timeout=READY_TIMEOUT_MS
    )
    names = second.evaluate(
        "async () => (await indexedDB.databases()).map(d => d.name)"
    )
    assert names == [other]


def test_a_full_disk_fails_an_indexeddb_write(browser, plain_url):
    page, name = open_page(browser, plain_url), PREFIX + "a.1"
    call(page, "indexed", "openDownload", name)
    browser_side = page.context.new_cdp_session(page)
    origin = {"origin": plain_url.rstrip("/")}
    used = browser_side.send("Storage.getUsageAndQuota", origin)["usage"]
    quota = {"quotaSize": int(used) + QUOTA_SLACK_BYTES}
    browser_side.send("Storage.overrideQuotaForOrigin", origin | quota)
    with pytest.raises(PlaywrightError, match="storage for this site is full"):
        call(page, "indexed", "storeChunk", name, 0, some(TOO_MUCH_BYTES, 1))
    # what does fit is still written.
    call(page, "indexed", "storeChunk", name, 0, some(1000, 1))
    assert call(page, "indexed", "loadChunk", name, 0, 0, 2) == [1, 1]


def test_the_file_system_keeps_chunks_until_their_page_is_gone(browser, secure_url):
    first = open_page(browser, secure_url)
    call(first, "opfs", "claimSession", "a")
    call(first, "opfs", "writeChunk", "a.1", 1, some(1000, 2))
    call(first, "opfs", "writeChunk", "a.1", 0, some(3000, 1))
    assert call(first, "opfs", "readChunk", "a.1", 0, 10, 15) == [1] * 5
    joined = call(first, "opfs", "joinChunks", "a.1", 2, "whole.bin")
    assert joined == {"name": "whole.bin", "bytes": [1] * 3000 + [2] * 1000}
    call(first, "opfs", "writeChunk", "a.2", 0, some(10, 3))
    call(first, "opfs", "removeFolder", "a.2")
    assert first.evaluate(FOLDERS_KEPT) == ["a.1"]
    # a second tab leaves a live session's folder be, and drops it once
    # that session's page is gone.
    second = open_page(browser, secure_url, first.context)
    call(second, "opfs", "claimSession", "b")
    assert second.evaluate(FOLDERS_KEPT) == ["a.1"]
    first.close()
    third = open_page(browser, secure_url, second.context)
    call(third, "opfs", "claimSession", "c")
    assert third.evaluate(FOLDERS_KEPT) == []
