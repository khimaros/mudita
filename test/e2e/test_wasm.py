"""the mudita core in a browser (the wasm/ crate, in chrome) over the live network.

the page under wasm_page/ is the core and nothing else: it starts a node and
leaves the core's exports on window.muditaCore, so what is proven here is
the module any app would load, before there is an app on it.

the page is served as the app is deployed, over plain http from a name that
is not a secure context, where the core keeps downloads in indexeddb. the
tests of storage run on a secure origin as well, where it keeps them in the
origin private file system.
"""

import base64
import json
import os
import re
import time

import pytest
from conftest import (
    INSECURE_STORAGE,
    PAYLOAD_BYTES,
    READY_TIMEOUT_MS,
    REPO_ROOT,
    SECURE_STORAGE,
    STARTED_LOG,
    TRANSFER_TIMEOUT_MS,
    WEB_LINK_BASE,
    CliSeeder,
    cli_fetch,
    expected_storage,
    link_for,
    open_page,
    site,
)
from playwright.sync_api import Error as PlaywrightError
from playwright.sync_api import Page

PAGE_ROOT = REPO_ROOT / "test/e2e/wasm_page"
WASM_ROOT = REPO_ROOT / "web/wasm"
POLL_SECONDS = 1
# the sender's lines as the page's log handler is given them: the words of
# the command line's log, without its timestamp.
SLOT_MESSAGE = r"serving from slot \d+"
STARTED_MESSAGE = r"download [0-9a-f]{16} started \(1 active\)"
# what the page logs about where it keeps downloads, by the storage it has.
STORAGE_MESSAGES = {
    INSECURE_STORAGE: "keeping downloads in indexeddb: the page is not a secure context",
    SECURE_STORAGE: "keeping downloads in the origin private file system",
}
# the browser store groups 256 pieces (about 7.5 MiB) per chunk.
LARGE_PAYLOAD_BYTES = 9_000_000
# true in the page once no download has anything in the browser's storage.
EMPTIED = "(await window.muditaPage.kept()).length === 0"


@pytest.fixture(scope="session")
def sites():
    """serve the page, with the built core under /wasm/, on both origins."""
    assert (WASM_ROOT / "mudita.js").exists(), "run `make core-wasm` first"
    mounts = {"/wasm/": WASM_ROOT}
    with site(PAGE_ROOT, mounts) as insecure, site(PAGE_ROOT, mounts, True) as secure:
        yield {INSECURE_STORAGE: insecure, SECURE_STORAGE: secure}


@pytest.fixture
def page_url(sites):
    """the page as the app is deployed: not a secure context."""
    return sites[INSECURE_STORAGE]


@pytest.fixture(params=[INSECURE_STORAGE, SECURE_STORAGE])
def either_url(request, sites):
    """the page on each kind of origin, and so over each kind of storage."""
    return sites[request.param]


def wait_for(page: Page, what: str, expression: str, timeout_ms: int):
    """poll an async javascript expression until it is truthy, and return it."""
    deadline = time.monotonic() + timeout_ms / 1000
    while time.monotonic() < deadline:
        failure = page.evaluate("window.muditaFailure")
        assert failure is None, failure
        if found := page.evaluate(f"(async () => {expression})()"):
            return found
        time.sleep(POLL_SECONDS)
    logs = page.evaluate("window.muditaPage?.logs")
    raise AssertionError(f"timed out waiting for {what}: {logs}")


def open_node(browser, url: str, context=None, namespace: str = "") -> Page:
    """open the page and wait for its veilid node.

    the node is the page's own: in a browser context of its own, or in
    `context` under a `namespace` no other page there has.
    """
    page = open_page(browser, url + (namespace and f"?namespace={namespace}"), context)
    ready = "window.muditaCore && (await window.muditaPage.ask('status')).ready"
    wait_for(page, "the node", ready, READY_TIMEOUT_MS)
    storage = expected_storage(url)
    assert page.evaluate("window.muditaCore.storageKind()") == storage
    logs = page.evaluate("window.muditaPage.logs")
    said = {"level": "INFO", "target": "mudita", "message": STORAGE_MESSAGES[storage]}
    assert said in logs, logs
    return page


def kept(page: Page) -> list[str]:
    """what downloads have in the browser's storage, of any page there."""
    return page.evaluate("window.muditaPage.kept()")


def ask(page: Page, method: str, **params):
    """one control request to the page's engine; returns its result."""
    return page.evaluate("([m, p]) => window.muditaPage.ask(m, p)", [method, params])


def web_share(page: Page, name: str, data: bytes) -> str:
    """share bytes as a file the user picked, and return its token."""
    encoded = base64.b64encode(data).decode()
    share = page.evaluate(
        "([name, data]) => window.muditaPage.shareBase64(name, data)", [name, encoded]
    )
    token = f"(await window.muditaPage.ask('shares')).shares.find(s => s.id === {share})?.token"
    return wait_for(page, "the share", token, READY_TIMEOUT_MS)


def web_start(page: Page, link: str) -> int:
    return page.evaluate("link => window.muditaCore.downloadFile(link)", link)


def web_settle(page: Page, download: int) -> dict:
    """wait for a download to finish or fail, and return how it is listed."""
    held = f"(await window.muditaPage.ask('downloads')).downloads.find(d => d.id === {download})"
    done = f"['done', 'error'].includes(({held})?.state) && {held}"
    return wait_for(page, "the download", done, TRANSFER_TIMEOUT_MS)


def web_saved(page: Page, download: int) -> tuple[str, bytes]:
    """what the page would save of a finished download."""
    saved = page.evaluate("id => window.muditaPage.downloaded(id)", download)
    return saved["name"], base64.b64decode(saved["encoded"])


def web_finish(page: Page, download: int) -> tuple[str, bytes]:
    settled = web_settle(page, download)
    assert settled["state"] == "done", settled
    return web_saved(page, download)


def web_fetch(page: Page, link: str) -> tuple[str, bytes]:
    return web_finish(page, web_start(page, link))


def page_logged(page: Page, message: str) -> bool:
    logs = page.evaluate("window.muditaPage.logs")
    return any(
        re.fullmatch(message, line["message"])
        and line["level"] == "INFO"
        and line["target"] == "mudita::peer"
        for line in logs
    )


def test_the_page_starts_a_node(browser, page_url):
    page = open_node(browser, page_url)
    status = ask(page, "status")
    assert status["ready"] is True
    assert status["folders"] == []
    assert status["libraries"] == []
    assert ask(page, "shares") == {"shares": []}


def test_wasm_fetches_from_cli(browser, page_url, cli_seeder, payload):
    """a bare token names nothing: the file is called what its sender calls it."""
    page = open_node(browser, page_url)
    name, received = web_fetch(page, cli_seeder.token)
    assert name == payload.name
    assert received == payload.read_bytes()


def test_wasm_saves_a_named_link_under_its_name(browser, page_url, cli_seeder, payload):
    page = open_node(browser, page_url)
    link = link_for(cli_seeder.token, "my notes #1.bin")
    assert web_fetch(page, link) == ("my notes #1.bin", payload.read_bytes())


def test_cli_fetches_from_wasm(browser, page_url, payload, tmp_path):
    """the page's own lines about its downloader are the command line's."""
    page = open_node(browser, page_url)
    token = web_share(page, payload.name, payload.read_bytes())
    assert page_logged(page, SLOT_MESSAGE)
    dest = tmp_path / "received.bin"
    assert cli_fetch(token, dest).returncode == 0
    assert dest.read_bytes() == payload.read_bytes()
    started = f"window.muditaPage.logs.some(l => /^{STARTED_MESSAGE}$/.test(l.message))"
    wait_for(page, "the download's log line", started, READY_TIMEOUT_MS)
    assert page_logged(page, STARTED_MESSAGE)


def test_wasm_keeps_a_multi_chunk_download_until_it_is_discarded(
    browser, either_url, tmp_path
):
    """big enough that the page's storage is written more than one chunk."""
    source = tmp_path / "large.bin"
    source.write_bytes(os.urandom(LARGE_PAYLOAD_BYTES))
    seeder = CliSeeder(source)
    try:
        page = open_node(browser, either_url)
        assert kept(page) == []
        download = web_start(page, seeder.link())
        name, received = web_finish(page, download)
    finally:
        seeder.stop()
    assert name == source.name
    assert received == source.read_bytes()
    assert len(kept(page)) == 1
    page.evaluate("id => window.muditaCore.discardDownload(id)", download)
    assert kept(page) == []
    assert ask(page, "downloads") == {"downloads": []}
    with pytest.raises(PlaywrightError, match="no such download"):
        web_saved(page, download)


def test_another_tab_leaves_a_download_alone_and_a_reload_clears_it(
    browser, either_url, cli_seeder, payload
):
    """what a page kept is dropped by the next to start, once that page is gone."""
    first = open_node(browser, either_url, namespace="first")
    download = web_start(first, cli_seeder.link())
    saved = web_finish(first, download)
    assert saved == (payload.name, payload.read_bytes())
    # a second tab starts, and cleans up after tabs that are closed.
    second = open_node(browser, either_url, first.context, "second")
    assert len(kept(second)) == 1
    assert web_saved(first, download) == saved
    # the first tab's own next start finds its last one gone.
    first.reload()
    wait_for(first, "the node", "window.muditaCore", READY_TIMEOUT_MS)
    wait_for(second, "the leftovers to go", EMPTIED, READY_TIMEOUT_MS)


def test_wasm_and_cli_share_one_link(browser, page_url, cli_seeder, payload):
    """the page and the command line are one implementation, and so agree."""
    page = open_node(browser, page_url)
    assert web_share(page, payload.name, payload.read_bytes()) == cli_seeder.token


def test_wasm_looks_a_link_up_without_downloading(
    browser, page_url, cli_seeder, payload
):
    page = open_node(browser, page_url)
    link = json.dumps(WEB_LINK_BASE + cli_seeder.token)
    # the name is in the sender's slot, which may not have reached the page.
    named = f"(found => found.names.length && found)(await window.muditaPage.ask('lookup', {{link: {link}}}))"
    found = wait_for(page, "the file's name", named, READY_TIMEOUT_MS)
    assert (found["name"], found["names"]) == (payload.name, [payload.name])
    assert (found["size"], found["token"]) == (PAYLOAD_BYTES, cli_seeder.token)
    log = cli_seeder.log_path.read_text()
    assert not re.search(STARTED_LOG, log, re.MULTILINE), log


def test_wasm_browses_a_library_link_and_downloads_from_it(browser, page_url, servers):
    notes, photo = os.urandom(PAYLOAD_BYTES), os.urandom(500)
    library = servers("alice", {"notes.bin": notes, "trips/photo.bin": photo})
    library.wait_until_shared(2)
    link = WEB_LINK_BASE + "library/" + library.wait_for_library()
    page = open_node(browser, page_url)
    listed = ask(page, "browse", link=link)
    by_place = {(entry["folder"], entry["name"]): entry for entry in listed["files"]}
    assert set(by_place) == {("", "notes.bin"), ("trips", "photo.bin")}
    assert by_place["", "notes.bin"]["size"] == PAYLOAD_BYTES
    wanted = by_place["trips", "photo.bin"]
    download = ask(page, "download", file=wanted)
    assert web_finish(page, download["download"]) == ("photo.bin", photo)
    # a listed file can be had by its token alone, under a name of one's own.
    download = ask(page, "download", token=wanted["token"], name="mine.bin")
    assert web_finish(page, download["download"]) == ("mine.bin", photo)
