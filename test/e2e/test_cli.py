"""cli to cli transfers over the live veilid network."""

import os
import shutil

from conftest import (
    FINISHED_LOG,
    LOG_PREFIX,
    STARTED_LOG,
    CliSeeder,
    cli_fetch,
    link_for,
)

# enough pieces that some are still to be asked for when a second peer is
# found, a moment after the first.
SWARM_PAYLOAD_BYTES = 2_000_000
FULL_LOG = "swarm is full, not serving"
# a peer told of late hears the finish of a download and not its start.
HEARD_LOG = LOG_PREFIX + r"download [0-9a-f]{16} (started|finished)"


def test_sending_a_file_again_gives_the_same_token(cli_seeder, payload):
    again = CliSeeder(payload, tag="again")
    try:
        assert again.token == cli_seeder.token
    finally:
        again.stop()


def test_a_copy_of_the_file_gives_the_same_token(cli_seeder, payload, tmp_path):
    copy = tmp_path / "copy" / payload.name
    copy.parent.mkdir()
    shutil.copy(payload, copy)
    other = CliSeeder(copy)
    try:
        assert other.token == cli_seeder.token
    finally:
        other.stop()


def test_one_file_under_two_names_is_one_swarm(servers, tmp_path):
    """a token says what a file holds, and each of its senders what it is called."""
    data = os.urandom(SWARM_PAYLOAD_BYTES)
    first, second = tmp_path / "first name.bin", tmp_path / "second name.bin"
    first.write_bytes(data)
    second.write_bytes(data)
    senders = [CliSeeder(first), CliSeeder(second)]
    try:
        token = senders[0].token
        assert senders[1].token == token
        assert senders[0].link() == link_for(token, first.name)
        assert senders[1].link() == link_for(token, second.name)
        asker = servers("asker", {}, plain=True)
        asker.wait_until_ready()

        # a slot may not have reached the asker's corner of the dht yet.
        def both_names():
            found = asker.call("lookup", link=token)
            return found if len(found["names"]) == 2 else None

        found = asker.wait_for("both names", both_names)
        # as common as each other, so in name order, and the first is used.
        assert found["names"] == [first.name, second.name]
        assert (found["name"], found["size"]) == (first.name, SWARM_PAYLOAD_BYTES)
        # the link's own name is taken over whatever the senders say.
        named = asker.call("lookup", link=link_for(token, "mine.bin"))
        assert (named["name"], named["token"]) == ("mine.bin", token)
        dest = tmp_path / "received.bin"
        fetched = cli_fetch(token, dest)
        assert fetched.returncode == 0
        assert dest.read_bytes() == data
        assert "found 2 peers" in fetched.stderr
    finally:
        for sender in senders:
            sender.stop()


def test_a_link_with_a_name_saves_under_that_name(cli_seeder, payload, tmp_path):
    link = link_for(cli_seeder.token, "my notes #1.bin")
    assert cli_fetch(link, cwd=tmp_path).returncode == 0
    assert (tmp_path / "my notes #1.bin").read_bytes() == payload.read_bytes()


def test_a_bare_token_saves_under_the_name_its_sender_gives(
    cli_seeder, payload, tmp_path
):
    folder = tmp_path / "fetched"
    folder.mkdir()
    assert cli_fetch(cli_seeder.token, cwd=folder).returncode == 0
    assert (folder / payload.name).read_bytes() == payload.read_bytes()


def test_cli_fetches_from_whichever_sender_remains(cli_seeder, payload, tmp_path):
    other = CliSeeder(payload, tag="other")
    try:
        first = tmp_path / "first.bin"
        assert cli_fetch(cli_seeder.token, first).returncode == 0
        assert first.read_bytes() == payload.read_bytes()
        cli_seeder.stop()
        second = tmp_path / "second.bin"
        assert cli_fetch(cli_seeder.token, second).returncode == 0
        assert second.read_bytes() == payload.read_bytes()
    finally:
        other.stop()


def test_cli_fetches_when_the_first_of_three_senders_has_crashed(
    cli_seeder, payload, tmp_path
):
    """the sender that crashed still holds its slot; the others are used."""
    others = [CliSeeder(payload, tag=tag) for tag in ("second", "third")]
    try:
        cli_seeder.kill()
        dest = tmp_path / "received.bin"
        assert cli_fetch(cli_seeder.token, dest).returncode == 0
        assert dest.read_bytes() == payload.read_bytes()
    finally:
        for other in others:
            other.stop()


def test_a_sender_beyond_the_slots_is_found_through_its_peers(tmp_path):
    """with the slots full, a sender is told of by the one that holds a slot."""
    payload = tmp_path / "swarm.bin"
    payload.write_bytes(os.urandom(SWARM_PAYLOAD_BYTES))
    holder = CliSeeder(payload, tag="holder", slots=1)
    beyond = CliSeeder(payload, tag="beyond", slots=1)
    try:
        assert beyond.token == holder.token
        assert FULL_LOG in beyond.log_path.read_text()
        dest = tmp_path / "received.bin"
        fetched = cli_fetch(holder.token, dest)
        assert fetched.returncode == 0
        assert dest.read_bytes() == payload.read_bytes()
        assert "found 2 peers" in fetched.stderr
        # it heard of the download, so the fetcher reached it.
        beyond.wait_for_log(HEARD_LOG)
    finally:
        holder.stop()
        beyond.stop()


def test_cli_fetches_from_cli(cli_seeder, payload, tmp_path):
    dest = tmp_path / "received.bin"
    assert cli_fetch(cli_seeder.token, dest).returncode == 0
    assert dest.read_bytes() == payload.read_bytes()


def test_sender_logs_a_cli_download(cli_seeder, tmp_path):
    assert cli_fetch(cli_seeder.token, tmp_path / "received.bin").returncode == 0
    started = cli_seeder.wait_for_log(STARTED_LOG)
    finished = cli_seeder.wait_for_log(FINISHED_LOG)
    assert started.group(1) == finished.group(1)


def test_cli_accepts_a_share_link(cli_seeder, payload, tmp_path):
    dest = tmp_path / "received.bin"
    assert cli_fetch(link_for(cli_seeder.token), dest).returncode == 0
    assert dest.read_bytes() == payload.read_bytes()


def test_cli_refuses_to_overwrite(cli_seeder, tmp_path):
    dest = tmp_path / "existing.bin"
    dest.write_bytes(b"keep me")
    assert cli_fetch(cli_seeder.token, dest).returncode != 0
    assert dest.read_bytes() == b"keep me"
