"""`dana serve` libraries and their remote control, over the live veilid network."""

import json
import os
import re
import signal
import socket
import time

from conftest import FETCH_TIMEOUT_SECONDS, STOP_TIMEOUT_SECONDS, cli_fetch, link_for

NOTES = os.urandom(100_000)
PHOTO = os.urandom(45_000)
# how recent "seen" must be for a library that answered moments ago.
SEEN_SLACK_SECONDS = 120
# how long a sender is given to hear that a download of its file finished.
NOTICE_SECONDS = 120
LEAVING_LOG = "leaving the swarm"
LEAVING_POLL_SECONDS = 0.2
LIBRARY_LINK_BASE = "http://veilid.tech/mudita/#library/"


def test_the_socket_answers_before_the_node_is_ready(servers):
    server = servers("alice", {"notes.bin": NOTES})
    server.wait_for("the control socket", server.socket.exists, timeout=10)
    status = server.call("status")
    assert status["folders"] == [str(server.home / "shared")]
    assert status["downloads"] == str(server.downloads)
    if not status["ready"]:
        refused = server.run("search", "notes")
        assert refused.returncode != 0
        assert "not ready yet" in refused.stderr


def test_a_command_without_a_server_says_so(servers, tmp_path):
    lonely = servers("nobody", {})
    lonely.stop()
    status = lonely.run("status")
    assert status.returncode != 0
    assert "no server is running" in status.stderr
    assert not lonely.socket.exists()


def test_status_shows_what_a_server_shares(servers):
    server = servers("alice", {"notes.bin": NOTES, "photo.bin": PHOTO, ".hidden": b"x"})
    shared = server.wait_until_shared(2)
    assert [file["name"] for file in shared] == ["notes.bin", "photo.bin"]
    # each has its link, which says what it is called here.
    for file in shared:
        assert file["link"] == link_for(file["token"], file["name"])
    status = server.run("status")
    assert status.returncode == 0
    assert "ready: yes" in status.stdout
    assert "files: 2 (2 shared)" in status.stdout
    # a config from when a server had one library: it is that library still,
    # called what its folder is, and downloads have one that no search finds.
    assert "libraries:" in status.stdout
    assert "  shared (discoverable, 2 files, 2 shared)" in status.stdout
    assert "  downloads (link only, 0 files, 0 shared)" in status.stdout
    assert all(file["library"] == "shared" for file in shared)


def test_a_plain_fetch_downloads_from_a_library(servers, tmp_path):
    server = servers("alice", {"notes.bin": NOTES})
    token = server.wait_until_shared(1)[0]["token"]
    dest = tmp_path / "received.bin"
    assert cli_fetch(token, dest).returncode == 0
    assert dest.read_bytes() == NOTES


def test_a_library_link_lists_the_library_by_folder(servers):
    alice = servers("alice", {"notes.bin": NOTES, "trips/2024/photo.bin": PHOTO})
    alice.wait_until_shared(2)
    token = alice.wait_for_library()
    named = alice.run("library", "rename", "shared", "the shelf")
    assert named.returncode == 0, named.stderr
    status = alice.run("status").stdout.splitlines()
    at = status.index("  the shelf (discoverable, 2 files, 2 shared)")
    link = status[at + 2].removeprefix("    link: ")
    assert link.startswith(LIBRARY_LINK_BASE)
    assert link.removeprefix(LIBRARY_LINK_BASE) == token.replace(":", "%3A")

    # a library that has never met alice reads her record from the link.
    bob = servers("bob", {})
    bob.wait_for(
        "the node", lambda: bob.socket.exists() and bob.call("status")["ready"]
    )
    # a file is shown where it is inside the library: the name of the
    # folder on her disk is no part of it.
    expected = [("", "notes.bin"), ("trips/2024", "photo.bin")]

    # her slot may not have reached bob's corner of the dht yet.
    def listed():
        page = bob.ask("browse", link=link, offset=0).get("result")
        return page if page and page["total"] == 2 else None

    catalog = bob.wait_for("alice's catalog", listed)
    assert [(file["folder"], file["name"]) for file in catalog["files"]] == expected
    assert catalog["library"] == "the shelf"
    browsed = bob.run("browse", link)
    assert browsed.returncode == 0, browsed.stderr
    assert "  trips/2024/photo.bin  45000 bytes" in browsed.stdout
    assert browsed.stdout.splitlines()[-1] == "2 files in the shelf"
    # having answered, she is known to bob by name, and as seen just now.
    known = bob.call("discover")["libraries"]
    hers = [library for library in known if library["name"] == "the shelf"]
    assert len(hers) == 1, known
    assert abs(hers[0]["seen"] - time.time()) < SEEN_SLACK_SECONDS
    # her own server answers for her link too, from its own catalog.
    assert alice.call("browse", link=link, offset=1)["files"][0]["name"] == "photo.bin"
    refused = bob.run("download", link)
    assert refused.returncode != 0
    assert "link to a library" in refused.stderr


def test_a_library_keeps_its_link_across_restarts(servers):
    server = servers("alice", {"notes.bin": NOTES})
    token = server.wait_for_library()
    server.stop()
    server.start()
    assert server.wait_for_library() == token


def test_a_leaving_server_makes_way_for_the_next(servers):
    """a window opened while the last one's server is leaving gets its own."""
    server = servers("alice", {"notes.bin": NOTES})
    server.wait_until_shared(1)
    leaving = server.process
    leaving.send_signal(signal.SIGTERM)
    while LEAVING_LOG not in server.log_path.read_text():
        assert leaving.poll() is None, "the server left before it could be watched"
        time.sleep(LEAVING_POLL_SECONDS)
    # leaving takes seconds, in which it must not answer anyone new.
    with socket.socket(socket.AF_UNIX) as client:
        assert client.connect_ex(str(server.socket)) != 0
    assert leaving.poll() is None, "the server left before it could be watched"
    server.start()
    server.wait_until_shared(1)
    assert leaving.wait(timeout=STOP_TIMEOUT_SECONDS) == 0


def test_one_library_finds_and_downloads_from_another(servers):
    # the lobby is the live network's, so the names are this run's own: no
    # other library, and no earlier run, shares a file called the same.
    run = os.urandom(4).hex()
    notes, photo = f"notes-alpha-{run}.bin", f"photo-beta-{run}.bin"
    # the same photo under another name: one file, to whoever looks for it.
    twin = f"photo-gamma-{run}.bin"
    alice = servers("alice", {notes: NOTES, photo: PHOTO, twin: PHOTO})
    tokens = {file["name"]: file["token"] for file in alice.wait_until_shared(3)}
    assert tokens[photo] == tokens[twin] != tokens[notes]
    bob = servers("bob", {})
    names = {notes, photo, twin}
    library = bob.wait_for("alice's library", lambda: bob.find_library(names))
    # requests over the live network time out now and then, so one is asked
    # again rather than taken as the answer.
    catalog = bob.wait_for(
        "alice's catalog",
        lambda: bob.ask("browse", library=library, offset=0).get("result"),
    )
    assert [file["name"] for file in catalog["files"]] == [notes, photo, twin]
    # what the lobby led to is the library her one folder is, whose files
    # are at its top: the folder's own name is no part of a catalog.
    assert {file["folder"] for file in catalog["files"]} == {""}
    assert catalog["library"] == "shared"
    # a listed file comes with the token to download it by.
    assert {file["name"]: file["token"] for file in catalog["files"]} == tokens

    found = bob.run("search", "photo", run)
    assert found.returncode == 0, found.stderr
    hits = [line for line in found.stdout.splitlines() if run in line]
    assert len(hits) == 1 and f"{photo} (and 1 more name)" in hits[0], found.stdout
    mine = [hit for hit in bob.call("search.last")["results"] if run in hit["name"]]
    assert len(mine) == 1, mine
    assert (mine[0]["name"], mine[0]["names"]) == (photo, [photo, twin])
    assert (mine[0]["token"], mine[0]["sources"]) == (tokens[photo], 1)

    found = bob.run("search", "ALPHA", run)
    assert found.returncode == 0, found.stderr
    hits = [line.split() for line in found.stdout.splitlines() if notes in line]
    assert len(hits) == 1, found.stdout
    number, name, size = hits[0][:3]
    assert (name, size) == (notes, "100000")
    started = bob.run("download", number)
    assert started.returncode == 0, started.stderr

    def finished():
        downloads = bob.call("downloads")["downloads"]
        assert downloads[0]["state"] != "error", downloads
        return downloads[0]["state"] == "done"

    bob.wait_for("the download", finished, timeout=FETCH_TIMEOUT_SECONDS)
    assert (bob.downloads / notes).read_bytes() == NOTES

    # alice sees who downloaded it, under the library the file is in. the
    # finished notice is given up after a few tries, so an upload that ends
    # as "gone" here is that known gap and not a new fault.
    def uploaded():
        listed = alice.call("uploads")["uploads"]
        ended = [one for one in listed if one["state"] != "downloading"]
        return ended if ended and len(ended) == len(listed) else None

    [upload] = alice.wait_for("bob's last notice", uploaded, timeout=NOTICE_SECONDS)
    assert (upload["name"], upload["library"]) == (notes, "shared")
    assert upload["state"] == "finished", upload
    assert upload["done"] == upload["total"] > 0
    assert upload["size"] == len(NOTES)
    assert "in shared  finished" in alice.run("uploads").stdout
    # what was downloaded is shared on, under the same link.
    reshared = bob.wait_until_shared(1)[0]
    assert reshared["token"] == tokens[notes]


def places(page: dict) -> list[tuple[str, str]]:
    """where a catalog page shows each file: its folder and its name."""
    return [(file["folder"], file["name"]) for file in page["files"]]


def browsed(server, link: str, total: int) -> dict | None:
    """the first catalog page behind a library link, once it lists `total` files.

    a record just published may not have reached the asker's corner of the
    dht, and requests over the live network time out now and then.
    """
    page = server.ask("browse", link=link, offset=0).get("result")
    return page if page and page["total"] == total else None


def test_each_library_of_a_server_is_a_library_of_its_own(servers):
    """a library's link shows its own files alone, and its route serves only those."""
    # the names are this run's own: they are looked for among everything
    # the live network's libraries are called. so are the files' bytes: a
    # file is its bytes to the network, whichever test shared them last.
    run = os.urandom(4).hex()
    tunes, papers = f"tunes-{run}", f"papers-{run}"
    contents = [os.urandom(size) for size in (40_000, 35_000, 31_000)]
    files = dict(zip(["music/a.bin", "music/live/b.bin", "docs/c.bin"], contents))
    set_out = {
        tunes: {"folders": ["music"], "listed": True},
        papers: {"folders": ["docs"]},
    }
    alice = servers("alice", files, libraries=set_out)
    alice.wait_until_shared(3)
    links = alice.wait_for_links()
    assert set(links) == {tunes, papers, "downloads"}
    assert len(set(links.values())) == 3, links

    bob = servers("bob", {})
    bob.wait_until_ready()
    own = bob.wait_for("the library", lambda: browsed(bob, links[tunes], 2))
    # under the name chosen for it, with paths inside it: the name of the
    # folder on disk is not in them, and nothing of her other library is.
    assert own["library"] == tunes
    assert places(own) == [("", "a.bin"), ("live", "b.bin")]
    other = bob.wait_for("the other one", lambda: browsed(bob, links[papers], 1))
    assert (other["library"], places(other)) == (papers, [("", "c.bin")])
    # her own server answers for each of her links from its own catalogs.
    assert places(alice.call("browse", link=links[tunes], offset=0)) == places(own)
    assert places(alice.call("browse", link=links[papers], offset=0)) == places(other)

    # each record's slot carries a route of its own: to bob, who knows a
    # library by its route, they are two libraries.
    known = bob.call("discover")["libraries"]
    ids = {one["name"]: one["id"] for one in known if run in (one["name"] or "")}
    assert set(ids) == {tunes, papers}, known
    assert len(set(ids.values())) == 2, ids

    # a piece of a file is served over the route of the library it is in.
    # over her other library's it is refused in the words of a library that
    # has never had the file.
    root = other["files"][0]["root"]
    carol = servers("carol", {"unrelated.bin": os.urandom(1000)})
    strangers = carol.wait_for_links()

    def probe(link: str) -> str:
        asked = lambda: bob.ask("probe", link=link, root=root).get("result")  # noqa: E731
        return bob.wait_for("an answer", asked)["answer"]

    assert probe(links[papers]) == "piece"
    stranger = probe(strangers["shared"])
    assert stranger != "piece"
    assert probe(links[tunes]) == stranger


def test_only_listed_libraries_are_found_by_a_search(servers):
    """the lobby has a listed library, and not an unlisted one."""
    run = os.urandom(4).hex()
    stall, vault = f"stall-{run}", f"vault-{run}"
    offered, kept = f"offered-{run}.bin", f"kept-{run}.bin"
    files = {
        f"open/{offered}": os.urandom(40_000),
        f"closed/{kept}": os.urandom(35_000),
    }
    # a library is found by searches only where its owner lists it: one
    # set out without a word about that is reached by its link alone.
    set_out = {
        stall: {"folders": ["open"], "listed": True},
        vault: {"folders": ["closed"]},
    }
    alice = servers("alice", files, libraries=set_out)
    alice.wait_until_shared(2)
    alice.wait_for_links()
    bob = servers("bob", {})
    bob.wait_until_ready()

    def in_lobby(wanted: str) -> set[str] | None:
        """what bob has read of the lobby, once `wanted` is in it."""
        bob.call("discover.refresh")
        names = {one["name"] for one in bob.call("discover")["libraries"]}
        return names if wanted in names else None

    met = bob.wait_for("the listed library", lambda: in_lobby(stall))
    assert vault not in met, met

    def found(wanted: set[str]) -> set[str] | None:
        """this run's files as a search finds them, once it finds `wanted`."""
        searched = bob.run("search", run)
        assert searched.returncode == 0, searched.stderr
        results = bob.call("search.last")["results"]
        names = {name for hit in results for name in hit["names"]}
        return names if wanted <= names else None

    assert bob.wait_for("the listed file", lambda: found({offered})) == {offered}
    # switched to listed while it runs, the other is found as well.
    listed = alice.run("library", "set", vault, "--discoverable")
    assert listed.returncode == 0, listed.stderr
    assert f"  {vault} (discoverable, 1 files, 1 shared)" in listed.stdout
    bob.wait_for("the library now listed", lambda: in_lobby(vault))
    bob.wait_for("both files", lambda: found({offered, kept}))
    unlisted = alice.run("library", "set", stall, "--link-only")
    assert f"  {stall} (link only, 1 files, 1 shared)" in unlisted.stdout


def test_every_link_of_a_server_outlives_a_restart_and_a_renaming(servers):
    files = {"music/a.bin": os.urandom(40_000)}
    set_out = {"tunes": {"folders": ["music"], "listed": True}}
    alice = servers("alice", files, libraries=set_out)
    links = alice.wait_for_links()
    assert set(links) == {"tunes", "downloads"}
    renamed = alice.run("library", "rename", "tunes", "my music")
    assert renamed.returncode == 0, renamed.stderr
    assert "  my music (discoverable" in renamed.stdout
    kept = {"my music": links["tunes"], "downloads": links["downloads"]}
    assert alice.wait_for_links() == kept
    alice.stop()
    alice.start()
    assert alice.wait_for_links() == kept
    assert [file["library"] for file in alice.wait_until_shared(1)] == ["my music"]

    # one made while the server runs has a link of its own, and shares what
    # is in the folders given to it. it starts out reached by that link
    # alone: no search finds it until its owner lists it.
    extra = alice.home / "shared" / "extra"
    extra.mkdir()
    (extra / "b.bin").write_bytes(os.urandom(35_000))
    assert alice.run("library", "add", "extra").returncode == 0
    shared = alice.run("library", "share", "extra", str(extra))
    assert shared.returncode == 0, shared.stderr
    assert "  extra (link only" in shared.stdout
    files = {file["name"]: file["library"] for file in alice.wait_until_shared(2)}
    assert files == {"a.bin": "my music", "b.bin": "extra"}
    assert alice.wait_for_links()["extra"] not in kept.values()
    # a folder is in one library, and names are not used twice.
    twice = alice.run("library", "share", "my music", str(extra))
    assert twice.returncode != 0 and "is shared in extra already" in twice.stderr
    assert alice.run("library", "add", "extra").returncode != 0

    assert alice.run("library", "remove", "extra").returncode == 0
    assert set(alice.wait_for_links()) == {"my music", "downloads"}
    alice.wait_until_shared(1)
    refused = alice.run("library", "remove", "downloads")
    assert refused.returncode != 0 and "downloads are saved there" in refused.stderr
    # the config is kept as the libraries are set out now, and names none
    # of them as standing for the rest.
    written = json.loads(alice.config.read_text())
    assert [one["name"] for one in written["libraries"]] == ["my music", "downloads"]
    assert set(written) == {"libraries", "downloads"}


def test_the_library_from_before_keeps_its_link(servers):
    """a server from when it had one library has that library still, at its link."""
    files = {"music/a.bin": os.urandom(40_000), "docs/c.bin": os.urandom(35_000)}
    alice = servers("alice", files, libraries={"anything": {"folders": ["music"]}})
    link = alice.wait_for_links()["anything"]
    alice.stop()
    # what such a server left behind: one record, and a config with its
    # folders and the name it gave its library.
    state = alice.home / "state" / "mudita" / "serve"
    kept = json.loads((state / "libraries.json").read_text())["anything"]
    (state / "libraries.json").unlink()
    (state / "library.json").write_text(json.dumps(kept))
    shared = alice.home / "shared"
    folders = [str(shared / "music"), str(shared / "docs")]
    alice.config.write_text(json.dumps({"folders": folders, "name": "the shelf"}))
    alice.start()

    alice.wait_until_shared(2)
    assert alice.wait_for_links()["the shelf"] == link
    own = alice.call("status")["libraries"]
    shelf = next(one for one in own if one["name"] == "the shelf")
    # one library with every folder it had, listed as it was. downloads are
    # a library of their own now, which no search finds.
    assert (shelf["folders"], shelf["listed"]) == (folders, True)
    assert [(one["name"], one["listed"]) for one in own[1:]] == [("downloads", False)]
    # its catalog no longer shows each folder's own name: both are its top.
    bob = servers("bob", {})
    bob.wait_until_ready()
    catalog = bob.wait_for("the library", lambda: browsed(bob, link, 2))
    assert catalog["library"] == "the shelf"
    assert places(catalog) == [("", "a.bin"), ("", "c.bin")]
    # the next change writes the config as it is now, and the link stays.
    assert alice.run("library", "set", "the shelf", "--link-only").returncode == 0
    assert "folders" not in json.loads(alice.config.read_text())
    alice.stop()
    alice.start()
    assert alice.wait_for_links()["the shelf"] == link


def test_a_server_without_a_library_shares_and_downloads_by_path(servers, tmp_path):
    """what the one-file app asks of its server: files by path, one at a time."""
    source = tmp_path / "notes.bin"
    source.write_bytes(NOTES)
    alice, bob = servers("alice", {}, plain=True), servers("bob", {}, plain=True)
    alice.wait_until_ready()
    status = alice.call("status")
    assert (status["folders"], status["libraries"]) == ([], [])
    share = alice.call("share.add", path=str(source))["share"]

    def shared():
        held = alice.call("shares")["shares"]
        assert [entry["id"] for entry in held] == [share]
        assert held[0]["state"] != "error", held
        return held[0]["token"] and held[0]

    held = alice.wait_for("the share", shared)
    token = held["token"]
    assert held["link"] == link_for(token, "notes.bin")
    bob.wait_until_ready()

    # the name is in alice's slot, which may not have reached bob yet.
    def named():
        found = bob.call("lookup", link=token)
        return found if found["names"] else None

    found = bob.wait_for("the file's name", named)
    assert (found["name"], found["names"]) == ("notes.bin", ["notes.bin"])
    assert (found["size"], found["token"]) == (100_000, token)
    assert set(found) == {"token", "size", "root", "name", "names"}
    dest = tmp_path / "saved as.bin"
    download = bob.call("download", token=token, path=str(dest))["download"]

    def finished():
        held = bob.call("downloads")["downloads"]
        assert [entry["id"] for entry in held] == [download]
        assert held[0]["state"] != "error", held
        return held[0]["state"] == "done" and held[0]

    saved = bob.wait_for("the download", finished, timeout=FETCH_TIMEOUT_SECONDS)
    assert saved["saved"] == str(dest)
    assert dest.read_bytes() == NOTES

    # alice's share lists who downloaded it, as bob's own notices told her.
    # the last of them is sent a few times and then given up, so a share
    # that never says "finished" here is that known gap, not a new fault.
    def downloaders():
        listed = alice.call("shares")["shares"][0]["downloads"]
        return listed if listed and listed[0]["state"] == "finished" else None

    told = alice.wait_for("bob's finished notice", downloaders, timeout=NOTICE_SECONDS)
    assert len(told) == 1, told
    assert told[0]["done"] == told[0]["total"] == saved["total"]
    assert re.fullmatch("[0-9a-f]{16}", told[0]["id"])
    assert told[0]["started"] <= told[0]["seen"] <= time.time() + SEEN_SLACK_SECONDS
    # `uploads` lists the same download, with the file it is of: one shared
    # on its own is in no library.
    [upload] = alice.call("uploads")["uploads"]
    assert {key: upload[key] for key in told[0]} == told[0]
    assert (upload["name"], upload["library"]) == ("notes.bin", None)
    assert upload["size"] == len(NOTES)
    assert alice.call("status")["uploading"] == 0
    # a path that is taken is left alone.
    again = bob.call("download", token=token, path=str(dest))["download"]

    def refused():
        held = {entry["id"]: entry for entry in bob.call("downloads")["downloads"]}
        return held[again]["error"]

    assert "already exists" in bob.wait_for("the refusal", refused)
    assert dest.read_bytes() == NOTES
    alice.call("share.stop", id=share)
    assert alice.call("shares") == {"shares": []}
    # its slot is given up in the background, and its downloads with it.
    alice.wait_for("the share to go", lambda: not alice.call("uploads")["uploads"])
