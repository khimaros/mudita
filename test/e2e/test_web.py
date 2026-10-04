"""transfers involving the web app (veilid in wasm, in chrome) over the live network.

the app is driven through window.muditaTest, which exists only in the e2e
build: flutter web paints to a canvas, so there is no dom to click through.
"""

import base64
import os
import time
from pathlib import Path

import pytest
from conftest import (
    FINISHED_LOG,
    PAYLOAD_BYTES,
    READY_TIMEOUT_MS,
    REPO_ROOT,
    STARTED_LOG,
    TRANSFER_TIMEOUT_MS,
    WEB_LINK_BASE,
    CliSeeder,
    cli_fetch,
    expected_storage,
    open_page,
    site,
)
from playwright.sync_api import Page

WEB_ROOT = REPO_ROOT / "build/web-e2e"
# the browser store groups 256 pieces (about 7.5 MiB) per chunk.
LARGE_PAYLOAD_BYTES = 9_000_000
NAME_TIMEOUT_SECONDS = 120
# where the core keeps downloads, asked of the module the app has loaded:
# importing it again gives the same one.
STORAGE_KIND = """async () => {
    const core = await import(new URL('wasm/mudita.js', document.baseURI).href);
    return core.storageKind();
}"""


@pytest.fixture(scope="session")
def web_url():
    """serve the e2e web build as it is deployed: not a secure context."""
    with site(WEB_ROOT) as url:
        yield url


@pytest.fixture(scope="session")
def secure_web_url():
    """serve the e2e web build on a secure origin."""
    with site(WEB_ROOT, secure=True) as url:
        yield url


def open_node(browser, url: str) -> Page:
    """open the app in its own browser context, which is its own veilid node."""
    page = open_page(browser, url)
    page.wait_for_function(
        "window.muditaTest && window.muditaTest.ready()", timeout=READY_TIMEOUT_MS
    )
    assert page.evaluate(STORAGE_KIND) == expected_storage(url)
    return page


def hook_call(page: Page, call: str, argument):
    """await a muditaTest call, failing with the app's own error message.

    a dart failure reaches javascript boxed in a generic error, so it is
    unboxed here; otherwise a test would fail without saying why.
    """
    return page.evaluate(
        f"""async (arg) => {{
            try {{ return await window.muditaTest.{call}; }}
            catch (e) {{ throw new Error(String(e.error ?? e)); }}
        }}""",
        argument,
    )


def web_seed(page: Page, name: str, data: bytes) -> str:
    encoded = base64.b64encode(data).decode()
    return hook_call(page, "seedBase64(arg[0], arg[1], '')", [name, encoded])


def web_fetch(page: Page, link: str) -> tuple[str, bytes]:
    """download through the app and return what the browser saved."""
    with page.expect_download(timeout=TRANSFER_TIMEOUT_MS) as download:
        hook_call(page, "fetch(arg)", link)
    saved = Path(download.value.path())
    return download.value.suggested_filename, saved.read_bytes()


def test_web_fetches_from_cli(browser, web_url, cli_seeder, payload):
    page = open_node(browser, web_url)
    name, received = web_fetch(page, cli_seeder.token)
    assert name == payload.name
    assert received == payload.read_bytes()
    started = cli_seeder.wait_for_log(STARTED_LOG)
    finished = cli_seeder.wait_for_log(FINISHED_LOG)
    assert started.group(1) == finished.group(1)


def test_web_fetches_from_cli_on_a_secure_origin(
    browser, secure_web_url, cli_seeder, payload
):
    """where the page may have it, a download is kept in its file system."""
    page = open_node(browser, secure_web_url)
    name, received = web_fetch(page, cli_seeder.token)
    assert name == payload.name
    assert received == payload.read_bytes()


def test_web_fetches_a_multi_chunk_file(browser, web_url, tmp_path):
    """big enough that the browser store writes more than one chunk to disk."""
    source = tmp_path / "large.bin"
    source.write_bytes(os.urandom(LARGE_PAYLOAD_BYTES))
    seeder = CliSeeder(source)
    try:
        page = open_node(browser, web_url)
        name, received = web_fetch(page, seeder.token)
    finally:
        seeder.stop()
    assert name == source.name
    assert received == source.read_bytes()


def test_cli_fetches_from_web(browser, web_url, payload, tmp_path):
    page = open_node(browser, web_url)
    token = web_seed(page, payload.name, payload.read_bytes())
    dest = tmp_path / "received.bin"
    assert cli_fetch(token, dest).returncode == 0
    assert dest.read_bytes() == payload.read_bytes()


def test_web_and_cli_share_one_link(browser, web_url, cli_seeder, payload):
    """the core in a page and the cli lead to the same record for the same file."""
    page = open_node(browser, web_url)
    assert web_seed(page, payload.name, payload.read_bytes()) == cli_seeder.token


def test_a_web_download_serves_the_file_on(
    browser, web_url, cli_seeder, payload, tmp_path
):
    """a page that has downloaded the file shares it after the sender is gone."""
    page = open_node(browser, web_url)
    web_fetch(page, cli_seeder.token)
    cli_seeder.stop()
    dest = tmp_path / "received.bin"
    assert cli_fetch(cli_seeder.token, dest).returncode == 0
    assert dest.read_bytes() == payload.read_bytes()


def look_up_named(page, token, name):
    """look a bare token up until it has its name: that comes from the
    sender's slot, which was written a moment ago and may not have reached
    this node's corner of the dht."""
    deadline = time.monotonic() + NAME_TIMEOUT_SECONDS
    while (found := hook_call(page, "lookUp(arg)", token))["name"] != name:
        assert time.monotonic() < deadline, "the sender's slot never arrived"
    return found


def test_web_looks_a_link_up_without_downloading(browser, web_url, cli_seeder, payload):
    """what the page shows for a file link before the user asks for the file."""
    page = open_node(browser, web_url)
    found = look_up_named(page, cli_seeder.token, payload.name)
    assert found == {"name": payload.name, "size": PAYLOAD_BYTES}


def test_web_browses_a_library_link_and_downloads_from_it(browser, web_url, servers):
    """a library link opened in the browser lists the folders; a file downloads."""
    notes, photo = os.urandom(PAYLOAD_BYTES), os.urandom(500)
    library = servers("alice", {"notes.bin": notes, "trips/photo.bin": photo})
    library.wait_until_shared(2)
    library.call("library.rename", name="shared", to="the shelf")
    link = WEB_LINK_BASE + "library/" + library.wait_for_library("the shelf")
    page = open_node(browser, web_url)
    listed = hook_call(page, "browse(arg)", link)
    by_place = {(entry["folder"], entry["name"]): entry for entry in listed}
    assert set(by_place) == {("", "notes.bin"), ("trips", "photo.bin")}
    assert by_place["", "notes.bin"]["size"] == PAYLOAD_BYTES
    name, received = web_fetch(page, by_place["trips", "photo.bin"]["token"])
    assert name == "photo.bin"
    assert received == photo


def test_web_fetches_from_web(browser, web_url, payload):
    seeder = open_node(browser, web_url)
    token = web_seed(seeder, payload.name, payload.read_bytes())
    fetcher = open_node(browser, web_url)
    look_up_named(fetcher, token, payload.name)
    name, received = web_fetch(fetcher, token)
    assert name == payload.name
    assert received == payload.read_bytes()
