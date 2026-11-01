"""shared helpers for the end-to-end tests, which run over the live veilid network."""

import contextlib
import functools
import http.server
import json
import os
import re
import signal
import socket
import subprocess
import sys
import threading
import time
import urllib.parse
from pathlib import Path

import pytest
from playwright.sync_api import Page, sync_playwright

REPO_ROOT = Path(__file__).resolve().parents[2]
CLI = Path(os.environ.get("MUDITA_CLI", REPO_ROOT / "target/release/dana"))
# attaching a fresh node and publishing a record can take a couple of minutes.
PUBLISH_TIMEOUT_SECONDS = 240
FETCH_TIMEOUT_SECONDS = 600
STOP_TIMEOUT_SECONDS = 30
# a download notice is retried for about this long before it is dropped.
LOG_TIMEOUT_SECONDS = 60
LOG_POLL_SECONDS = 0.5
SERVER_POLL_SECONDS = 2
# every line `danasend` logs about its downloaders: an rfc 3339 utc
# timestamp, the level, and the module that logged it.
LOG_PREFIX = r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+Z +INFO mudita::peer: "
STARTED_LOG = LOG_PREFIX + r"download ([0-9a-f]{16}) started \(1 active\)$"
FINISHED_LOG = (
    LOG_PREFIX
    + r"download ([0-9a-f]{16}) finished in \d+s \(\d+ pieces served in total\)$"
)
# several pieces plus a short final one.
PAYLOAD_BYTES = 100_000
# test-only: the highest slot a sender will take (see core/src/peer.rs).
TEST_SLOTS_ENV = "MUDITA_TEST_SLOTS"
# sent from the secure origin only: a browser ignores them on plain http.
ISOLATION_HEADERS = {
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Embedder-Policy": "require-corp",
    "Cross-Origin-Resource-Policy": "cross-origin",
}
# the app is deployed over plain http, which is not a secure context, and a
# browser treats http://localhost as one. this made-up name is resolved to
# this machine by the browser (see the `browser` fixture), and is not.
INSECURE_HOST = "mudita.test"
SECURE_HOST = "localhost"
# which storage the core picks for downloads on each kind of origin.
INSECURE_STORAGE = "indexeddb"
SECURE_STORAGE = "opfs"
READY_TIMEOUT_MS = 240_000
TRANSFER_TIMEOUT_MS = 600_000
# links are made for the hosted app; only what follows the '#' matters here.
WEB_LINK_BASE = "http://veilid.tech/mudita/#"
# the link `danasend` logs, which says what the file is called there.
LINK_LOG = r"INFO dana: link: (\S+)$"


def link_for(token: str, name: str = "") -> str:
    """the share link of a token, as the core writes it: with a name or bare."""
    quoted = urllib.parse.quote(token, safe="")
    named = "/" + urllib.parse.quote(name, safe="") if name else ""
    return WEB_LINK_BASE + quoted + named


class CliSeeder:
    """a `danasend` process sharing one file until stopped."""

    def __init__(self, path: Path, tag: str = "send", slots: int | None = None):
        # the tag keeps the logs of two senders of one file apart.
        self.log_path = path.with_suffix(f".{tag}.log")
        # `slots` narrows the slots this sender will take, so that a test
        # can fill a swarm with two senders instead of nine.
        narrowed = {} if slots is None else {TEST_SLOTS_ENV: str(slots)}
        with self.log_path.open("w") as log:
            self.process = subprocess.Popen(
                [CLI, "send", path],
                stdout=subprocess.PIPE,
                stderr=log,
                text=True,
                env=os.environ | narrowed,
            )
        self.token = self.process.stdout.readline().strip()
        assert self.token, "dana send exited without printing a share token"

    def link(self) -> str:
        """the share link the sender logged, with the file's name in it."""
        return self.wait_for_log(LINK_LOG).group(1)

    def kill(self) -> None:
        """stop without a word, as a sender that crashed: its slot stays."""
        self.process.kill()
        self.process.wait(timeout=STOP_TIMEOUT_SECONDS)

    def wait_for_log(self, pattern: str) -> re.Match:
        """wait for a line `danasend` logs about its downloaders."""
        deadline = time.monotonic() + LOG_TIMEOUT_SECONDS
        while time.monotonic() < deadline:
            if match := re.search(pattern, self.log_path.read_text(), re.MULTILINE):
                return match
            time.sleep(LOG_POLL_SECONDS)
        raise AssertionError(f"{pattern!r} not in:\n{self.log_path.read_text()}")

    def stop(self) -> None:
        self.process.send_signal(signal.SIGINT)
        try:
            self.process.wait(timeout=STOP_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            self.process.kill()


def cli_fetch(
    link: str, dest: Path | None = None, cwd: Path | None = None
) -> subprocess.CompletedProcess:
    """run `danafetch`; its log is in the result's stderr, and echoed.

    without `dest` the file is saved in `cwd`, under whatever name the link
    or the file's peers give it.
    """
    placed = [] if dest is None else ["-o", dest]
    fetched = subprocess.run(
        [CLI, "fetch", link, *placed],
        timeout=FETCH_TIMEOUT_SECONDS,
        check=False,
        stderr=subprocess.PIPE,
        text=True,
        cwd=cwd,
    )
    sys.stderr.write(fetched.stderr)
    return fetched


class Server:
    """a `danaserve` process with a home, state and control socket of its own."""

    def __init__(self, home: Path, config: dict, plain: bool = False):
        """`config` is the server's config file, in the old shape or the new."""
        self.home = home
        # a plain server has no library: only what its clients ask of it.
        self.plain = plain
        self.socket = home / "ctl.sock"
        self.downloads = home / "Downloads" / "mudita"
        self.log_path = home / "serve.log"
        config, written = home / "serve.json", config
        config.write_text(json.dumps(written))
        # nothing of the user's own home or runtime directory is touched.
        self.env = {
            **os.environ,
            "HOME": str(home),
            "XDG_STATE_HOME": str(home / "state"),
            "XDG_CONFIG_HOME": str(home / "config"),
            "MUDITA_SOCKET": str(self.socket),
            # the `probe` control method, by which a test asks one library
            # for a piece of a file.
            "MUDITA_TEST_PROBE": "1",
        }
        self.env.pop("XDG_RUNTIME_DIR", None)
        self.config = config
        self.start()

    def start(self) -> None:
        """start the process, as at first or again after `stop`."""
        mode = ["--no-library"] if self.plain else ["--config", self.config]
        with self.log_path.open("a") as log:
            self.process = subprocess.Popen(
                [CLI, "serve", *mode], env=self.env, stderr=log
            )

    def wait_until_ready(self) -> None:
        def ready():
            return self.socket.exists() and self.call("status")["ready"]

        self.wait_for("the node", ready)

    def wait_for_library(self, name: str = "shared") -> str:
        """wait until the record of the library called `name` is published; its token.

        `shared` is what the one folder of a server started without
        libraries set out is called as a library.
        """

        def token():
            if not self.socket.exists():
                return None
            own = self.call("status")["libraries"]
            return next((one["token"] for one in own if one["name"] == name), None)

        return self.wait_for("the library record", token)

    def wait_for_links(self) -> dict[str, str]:
        """wait until every library's record is published; each one's link by name."""

        def links():
            if not self.socket.exists():
                return None
            own = self.call("status")["libraries"]
            return all(one["link"] for one in own) and {
                one["name"]: one["link"] for one in own
            }

        return self.wait_for("the libraries' links", links)

    def run(self, *args: str) -> subprocess.CompletedProcess:
        """run a remote control command, such as `status`, against this server."""
        return subprocess.run(
            [CLI, *args], env=self.env, capture_output=True, text=True, check=False
        )

    def ask(self, method: str, **params) -> dict:
        """one request over the control socket; returns the whole response."""
        with socket.socket(socket.AF_UNIX) as client:
            client.connect(str(self.socket))
            request = {"id": 1, "method": method, "params": params}
            with client.makefile("rw") as stream:
                stream.write(json.dumps(request) + "\n")
                stream.flush()
                return json.loads(stream.readline())

    def call(self, method: str, **params):
        """one request that must succeed; returns its result."""
        response = self.ask(method, **params)
        assert "error" not in response, response
        return response["result"]

    def find_library(self, names: set[str]) -> str | None:
        """the id of the library discovered here whose catalog is exactly `names`.

        the lobby is shared with everyone on the live network, so other
        libraries are discovered too, and some of them will not answer.
        """
        if not self.socket.exists() or not self.call("status")["ready"]:
            return None
        for library in self.call("discover")["libraries"]:
            page = self.ask("browse", library=library["id"], offset=0).get("result")
            if page and {file["name"] for file in page["files"]} == names:
                return library["id"]
        return None

    def wait_for(self, what: str, check, timeout: float = PUBLISH_TIMEOUT_SECONDS):
        """poll until check() returns something truthy, and return it."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            assert self.process.poll() is None, self.log_path.read_text()
            if found := check():
                return found
            time.sleep(SERVER_POLL_SECONDS)
        raise AssertionError(
            f"timed out waiting for {what}:\n{self.log_path.read_text()}"
        )

    def wait_until_shared(self, count: int) -> list[dict]:
        """wait until `count` files are shared, and return how `files` lists them."""

        def shared():
            if not self.socket.exists() or not self.call("status")["ready"]:
                return None
            files = self.call("files", offset=0, limit=count + 1)["files"]
            done = [file for file in files if file["state"] == "shared"]
            return done if len(done) == count else None

        return self.wait_for(f"{count} shared files", shared)

    def stop(self) -> None:
        self.process.send_signal(signal.SIGTERM)
        try:
            self.process.wait(timeout=STOP_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            self.process.kill()


def pytest_runtest_logreport(report):
    """show a failure when it happens: pytest keeps them for the end of a
    run, and this one takes most of an hour."""
    if report.failed:
        print(f"\n{report.longreprtext}", file=sys.stderr, flush=True)


class SiteHandler(http.server.SimpleHTTPRequestHandler):
    """serves a directory, with extra headers on every response."""

    # url prefixes served from somewhere other than the directory.
    mounts: dict[str, Path] = {}
    headers: dict[str, str] = {}

    def translate_path(self, path):
        # a handler may serve more than one request, so the directory it
        # was made with is put back.
        root = self.directory
        for prefix, folder in self.mounts.items():
            if path.startswith(prefix):
                self.directory, path = str(folder), path[len(prefix) - 1 :]
        try:
            return super().translate_path(path)
        finally:
            self.directory = root

    def end_headers(self):
        for name, value in self.headers.items():
            self.send_header(name, value)
        super().end_headers()

    def log_message(self, *args):
        pass


@contextlib.contextmanager
def site(root: Path, mounts: dict[str, Path] | None = None, secure: bool = False):
    """serve `root` on a free local port; gives its url.

    as the app is deployed unless `secure`: over plain http from a name
    that is not this machine's own, with no headers of ours. a secure site
    is cross-origin isolated on localhost.
    """
    headers = ISOLATION_HEADERS if secure else {}
    served = type("Site", (SiteHandler,), {"mounts": mounts or {}, "headers": headers})
    handler = functools.partial(served, directory=root)
    server = http.server.ThreadingHTTPServer((SECURE_HOST, 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    host = SECURE_HOST if secure else INSECURE_HOST
    try:
        yield f"http://{host}:{server.server_port}/"
    finally:
        server.shutdown()


def is_secure(url: str) -> bool:
    return urllib.parse.urlsplit(url).hostname == SECURE_HOST


def open_page(browser, url: str, context=None) -> Page:
    """open `url` in `context`, or in a browser context of its own.

    a context has its own storage, so a page alone in one is a veilid node
    of its own. the page must be the kind of origin it was served as.
    """
    page = (context or browser.new_context()).new_page()
    page.set_default_timeout(TRANSFER_TIMEOUT_MS)
    # shown with a failing test: a page that never loads says nothing else.
    page.on("pageerror", lambda error: print(f"page error: {error}", file=sys.stderr))
    page.on(
        "requestfailed",
        lambda request: print(
            f"failed: {request.url} {request.failure}", file=sys.stderr
        ),
    )
    page.goto(url)
    assert page.evaluate("window.isSecureContext") is is_secure(url)
    return page


def expected_storage(url: str) -> str:
    return SECURE_STORAGE if is_secure(url) else INSECURE_STORAGE


@pytest.fixture
def browser():
    resolve = f"--host-resolver-rules=MAP {INSECURE_HOST} 127.0.0.1"
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(channel="chrome", args=[resolve])
        yield browser
        browser.close()


@pytest.fixture
def servers(tmp_path):
    """starts `danaserve` processes on demand and stops them afterwards."""
    started = []

    def start(
        name: str,
        files: dict[str, bytes],
        plain: bool = False,
        libraries: dict[str, dict] | None = None,
    ) -> Server:
        """a server sharing `files`, which are paths under its `shared` folder.

        that folder is the one thing shared, written as a config from when
        a server had one library, unless `libraries` sets them out: by name,
        each with the `folders` under `shared` it has and, where it says so,
        whether it is `listed`: one that does not say is reached by its link
        alone, as any new library is.
        """
        home = tmp_path / name
        folder = home / "shared"
        folder.mkdir(parents=True)
        for file, data in files.items():
            (folder / file).parent.mkdir(parents=True, exist_ok=True)
            (folder / file).write_bytes(data)
        config = {"folders": [str(folder)]}
        if libraries is not None:
            set_out = [
                {
                    "name": called,
                    "folders": [str(folder / inside) for inside in one["folders"]],
                    **({"discoverable": one["listed"]} if "listed" in one else {}),
                }
                for called, one in libraries.items()
            ]
            config = {"libraries": set_out}
        started.append(Server(home, config, plain))
        return started[-1]

    yield start
    for server in started:
        server.stop()


@pytest.fixture
def payload(tmp_path) -> Path:
    path = tmp_path / "payload.bin"
    path.write_bytes(os.urandom(PAYLOAD_BYTES))
    return path


@pytest.fixture
def cli_seeder(payload):
    seeder = CliSeeder(payload)
    yield seeder
    seeder.stop()
