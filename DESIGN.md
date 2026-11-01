# design

how mudita moves a file directly between peers over veilid.

## overview

mudita has no server and stores no file data in the dht. a peer keeps the
file on its own device and answers piece requests over a veilid private route
for as long as it stays running. the only thing in the dht is one small
rendezvous record that says what the file is and where the peers holding it
can be reached right now.

the record's address is derived from the file itself, so everyone who shares
the same file lands on the same record and the same link, and a downloader
pulls from all of them at once. a downloader joins the record too and serves
the pieces it has verified, for as long as it stays open.

there is one implementation of the protocol, in rust, and every client runs
it:

- `core/` - the protocol, sharing, downloading, the library, the logging:
  everything that is the same wherever it runs, built on veilid-core.
- `cli/` - `dana`, the command line, which is the core on a desktop or a
  server.
- `wasm/` - the core for a web page, where it runs a full veilid node in the
  browser.
- `lib/` - the flutter app (web, linux). it has no protocol code: it shows
  what the core reports and passes on what the user asks for, through the
  control methods ("the server and its remote control").

`testdata/protocol_vectors.json` pins the byte-level encodings, the merkle
tree shape and the log lines, and the core's tests check themselves against
it. `make test-e2e` then transfers real files between the cli, the server
and the web app over the live network.

## protocol (version 4)

version 1 had one seeder per record and a random record key. version 2 added
the share keys and the swarm. version 3 named the file in every request, so
one route can serve many files, and added the library requests. version 4
takes the file's name out of what a link is derived from, so the same bytes
are one swarm whatever anyone calls them. no two versions interoperate: a
reader rejects a manifest whose `version` is not its own.

### pieces

a file is split into pieces of `PIECE_BYTES` = 30720 bytes (30 KiB). the last
piece may be shorter. an empty file has exactly one empty piece, so every
file has a merkle root. `piece_count = max(1, ceil(size / PIECE_BYTES))`.

the piece size is chosen so that one piece plus its merkle proof fits in a
single veilid app_call reply, which is capped at 32768 bytes.

### merkle tree

`H` is the veilid VLD0 hash (blake3, 32 bytes).

- leaf: `H(0x00 || piece)`
- interior node: `H(0x01 || left || right)`
- a level with an odd number of nodes promotes its last node unchanged to the
  next level (it is not duplicated or rehashed).
- the root is the single node of the last level. a one-piece file's root is
  its leaf hash.

the distinct prefixes keep an interior node from being passed off as a leaf.

the proof for piece `i` is the list of sibling hashes from the leaf level up,
omitting any level where the node has no sibling (it was promoted). to
verify, walk up from the leaf hash with `width = piece_count`: at each level
where `(index ^ 1) < width`, consume the next proof hash and combine it (the
node is the right child when `index` is odd); then `index >>= 1` and
`width = ceil(width / 2)`. the proof is valid when every proof hash was
consumed and the result equals the root.

### share keys

everything that locates a share is derived from the file, so sharing the same
file again, from any device, gives the same record and the same link.

`identity = root || size`: the 32 byte merkle root and the file size as a
big-endian u64. the name is not part of it: a name is a label people give
the bytes, and two people with the same file under different names share one
record and one swarm.

- owner secret: `H(0x02 || identity)`, used as a VLD0 secret key (an ed25519
  seed). the owner public key follows from it as in rfc 8032.
- record encryption key: `H(0x03 || identity)`.

the prefixes continue the ones the merkle tree uses. veilid has no call that
turns a secret key into its public key, so the core computes it itself, with
`ed25519-dalek`.

what this costs: anyone who already has the file can find the record, see that the file is being shared, and reach its peers' routes.
they learn no ip address (R9) and nothing they could not already read.

### rendezvous record

a veilid dht record with the default (DFLT) schema and `1 + PEER_SLOTS` = 9
owner subkeys. its owner keypair and its encryption key are the share keys, so
the contents are readable only by someone holding the full record key, and
writable by anyone who has read the manifest.

- subkey 0, the manifest: utf-8 json.
  `{"version":4,"size":123,"pieceBytes":30720,"root":"<64 hex chars>"}`
  `root` is lowercase hex. a reader rejects a manifest whose `version` or
  `pieceBytes` it does not know, and one whose share keys do not give the
  record key it was read from. every field is covered by that check, so
  nothing in a manifest can be changed by someone who merely has the link.
  a peer writes the manifest only when the record holds none that passes.
- subkeys 1 to 8, the peer slots: one peer each.
  `flags (1 byte) || unix seconds (big-endian u64) || name length (big-endian u16) || name (utf-8) || route blob`
  bit 0 of `flags` is set when the peer holds every piece, and bit 1 when it
  is a library (see "libraries"). the time is when the peer last wrote the
  slot. the name is what that peer calls the file, at most `NAME_MAX` = 255
  bytes and possibly empty. a slot is vacant when it is unset, too short to
  hold its header and name, has no route, or is stale: its time is more than
  `SLOT_STALE` = 360 seconds from the reader's clock, in either direction.

#### names

a name is not verified by anything: each peer says what it calls the file,
and a reader treats every name as text from a stranger, keeping only its
last path component when saving. where a name comes from, in order of
trust:

1. the link the user was given, which may carry one (see below). the link
   is what they chose to trust, so its name is used as it is.
2. a library's catalog or a search result, which names each file.
3. the slots: every distinct name the peers there give, the most common
   first. this is what a bare token has to go on, and an app shows all of
   them for the user to pick from.

with none of these the file is called `download`.

reading the slots for names is not free: a slot nobody has written takes
veilid 10 seconds to give up on, and a written one usually answers in a
second but sometimes in five. so a lookup reads them side by side, waits up
to `NAMES_WAIT` = 10 seconds for the first name, gives the rest
`NAMES_GRACE` = 1.5 seconds more, and reports the names that answered. a
download reads them for a name only when neither the link nor the asker
gave one. a peer sharing one file under two names holds a slot for each,
which is what makes both names visible.

veilid always creates a record with a random encryption key, so a peer
creates the record under the owner keypair, closes it, and reopens it under
the derived record key. a peer first tries to open the record as a writer,
which succeeds when anyone has shared the file before, and creates it only
when that fails.

### peer slots

every peer that serves pieces holds one slot.

- claim: read all slots from the network, pick a vacant one at random, and
  write it. if the write loses to a newer value, read again and pick again.
  when no slot is vacant the swarm is full: the peer still serves whoever
  finds it through peer exchange (below), and tries for a slot again at its
  next heartbeat.
- heartbeat: every `SLOT_HEARTBEAT` = 120 seconds at most, and by chance
  up to a quarter sooner, drawn anew for each beat, read the slot back from
  the network. if it still holds this peer's route, rewrite it with the
  current time; if another peer took it, claim a slot again. the chance is
  there because a slot says when it was written: peers that beat in step
  can be told to be on one node (see "a node's libraries").
- route change: when a veilid RouteChange update lists the peer's route as
  dead, allocate a new one (stability `Reliable`) and rewrite the slot.
- leave: write the slot with no route.

two peers can claim one slot in the same moment. the dht keeps one of the
writes and the loser finds out at its next heartbeat, so it is unreachable
for at most one heartbeat. a peer that vanishes without leaving holds its
slot until it goes stale.

### share token and link

the share token is the record key string as veilid prints it (it embeds the
record encryption key). a share link is any url whose fragment is the
percent-encoded token and then, optionally, a slash and the percent-encoded
name: `<base url>#<token>/<name>`. a slash inside a name is encoded like
anything else, so the first slash ends the token. the fragment is never sent
to the web server hosting the app. every input that takes a link also
accepts a bare token, or `<token>/<name>`.

the web app is hosted at `http://veilid.tech/mudita/`, so that is the base url
of the links the cli and the native apps produce. the web app builds links
from the page it is running on, so a locally served build links to itself.

### piece requests

a fetcher imports a peer's route blob and sends an app_call to the route.

- request: 37 bytes, `0x01`, then the 32 byte merkle root of the file, then
  the piece index as a big-endian u32. the root says which file is meant,
  because one route may serve many. it is the root and not the whole
  identity: two files with one root have the same pieces under any name.
- reply, success: `0x00`, then the piece bytes, then the proof hashes
  (32 bytes each). the fetcher knows the piece length from the manifest, so
  whatever follows the piece is the proof.
- reply, failure: `0x01`, then a utf-8 error message. a peer that is still
  downloading answers this way for a piece it does not hold yet.

a fetcher verifies every piece against the manifest root before accepting it.

### fetching from the swarm

a fetcher reads every peer slot and imports the route of each occupied one,
skipping its own. piece `i` starts with peer `i mod n` of the `n` it knows,
which spreads the requests evenly, and on any failure moves to the next peer.
when every peer has failed for a piece, that is one failed round, and another
follows. a piece is given `PIECE_PATIENCE` = 600 seconds of rounds before the
download fails: the first retry is immediate and later ones wait up to 5
seconds.

a peer that fails `PEER_STRIKES` = 3 requests in a row goes to the end of
every round, so it is asked only once every other peer has failed; a success
clears its count. an error reply is not a strike: the peer is reachable and
simply lacks the piece. a peer whose slot comes back with a different route
starts clean.

every slot read imports every route again, even one already known: veilid
forgets an imported route once a request over it has timed out.

the slots are also read again every `PEER_REFRESH` = 60 seconds while a
download runs, so peers that join later are used.

### peer exchange

the slots are where a swarm is first found, not the whole of it. a peer of a
file also tells whoever asks which other peers of that file it knows:

- request: `0x06`, the 32 byte merkle root, then optionally the caller's
  own route blob.
- reply, success: `0x00`, then up to `PEERS_MAX` = 16 routes of other peers
  of that file, most recently heard of first, each as a big-endian u16
  length and the blob (the format of a library's peers reply).
- reply, failure: `0x01` and a message, as for any request; a peer that
  does not know the request, or does not share the file, answers this way,
  and the asker takes it to mean "none".

a peer knows of the peers whose slots it has read, and of every caller that
gave its route, and forgets one it has not heard of for `SLOT_STALE`. a
caller gives its route only if it serves the file itself.

a fetcher asks each peer it reaches, once when it first reaches it and
again at every `PEER_REFRESH`, and adds what it is told to the peers it
fetches from. a peer that does not answer is asked again every 5 seconds
for 2 minutes: a route that was just published can be silent for its first
half minute, and that is exactly when a new peer is first asked. so one working slot is enough to find the rest, and a peer
that found the slots full still serves: it gives its route to the peers in
the slots, at each heartbeat, and they pass it on. such a peer is found
only through them, so it tries for a slot again at every heartbeat as
before.

### finding peers quickly

reading a slot is a network round trip, and one nobody has written is the
slowest kind, so a fetcher does not wait on all of them:

- it reads all the slots side by side and uses each peer the moment its
  slot comes back, so it starts asking for pieces as soon as it has one
  peer; others join the rotation as they are found, from slots or from peer
  exchange. the reads still outstanding when the download ends are dropped.
- a failed round does not wait on a new read of the slots. the piece is
  tried again at once, and the slots are read again in the background, no
  more often than every `SLOT_REREAD` = 15 seconds.
- a piece request is given `PIECE_TIMEOUT` = 3 seconds, after which the
  next peer is tried, and the route is imported again, since veilid forgets
  one after a timeout.
- `PARALLEL` = 16 piece requests run at once.

measured on the live network, which is where these numbers come from: a
written slot reads in about half a second and a never-written one in 10
seconds, every time, which is veilid's lookup timeout; asking the record
which subkeys are written (a network inspect) takes those same 10 seconds,
so it is no shortcut. a piece request is answered in 0.2 to 1.4 seconds and
one to a vanished peer fails after veilid's 5. a 5 MB download took 9 to 14
seconds of piece time at 8 parallel requests, 3.4 to 4.0 at 16, and 2.6 to
3.9 at 32 with each answer twice as slow. with all of this the time from
starting a fetch to its first piece fell from 10 seconds to about 1.

claiming a slot still waits on all 8 reads: a claim has to see that a slot
really is vacant.

these change no bytes on the wire: they are how the core's fetcher
behaves, wherever it runs.

### download notices

piece requests arrive over private routes and say nothing about who sent
them, so a seeder cannot tell on its own when a download begins or ends, or
tell two downloaders apart. a fetcher therefore announces both:

- request: 42 bytes, `0x02`, then an event byte (`0x01` started, `0x02`
  finished), then an 8 byte download id, then the merkle root of the file.
- reply: `0x00` alone.

the id is random, chosen per download, and identifies nothing else. a fetcher
sends "started" once it has the manifest and "finished" once every piece has
verified, each to every peer it knows at that moment. notices are a courtesy:
a fetcher tries each a few times, then gives up, accepts any reply, and never
waits on "started" before asking for pieces. a download that fails or is
abandoned sends no "finished".

while it downloads, a fetcher also reports how far along it is:

- request: 46 bytes, `0x02`, the event byte `0x03` (progress), the download
  id, the merkle root, then the number of pieces verified so far as a
  big-endian u32.
- reply: `0x00` alone.

it sends one to every peer it knows every `PROGRESS_INTERVAL` = 10 seconds,
whether or not the number has moved, once each and with no retry: the next
one is never far off. a peer cannot count this itself, because a fetcher
takes its pieces from many peers.

a peer uses notices only to report activity to its user, by logging them
(see "logging" below). every `PROGRESS_INTERVAL` it logs, for each download
whose count has changed since it last said so, how far along that download
is; a download that has not moved is not mentioned again. a progress notice
for a download whose "started" was lost starts it. a download not heard
from in any notice for `DOWNLOAD_GONE` = 60 seconds is taken to have gone
away: the peer says so once, with how far it got, and no longer counts it.
this is also what keeps "active" counts honest when a downloader vanishes.

the same bookkeeping is what the node's owner is shown: every peer keeps
the downloads it has heard of, those that finished or went away too (the
last 64 of each file, until the file is no longer served), and the control
methods `uploads`, `shares` and `status` read them (see "the server and
its remote control"). a node with libraries has a peer for each, so
`uploads` gathers them and says which library each file is in. none of it
is sent to anyone: on the network each library still answers for itself
alone.

so that every interface says the same thing in the same words, what a peer
logs about its downloaders is pinned like the wire format: the `downloadLog`
vector is a run of notices and timer ticks with the exact lines each must
produce, and the core is tested against it. it keeps this in one small pure
component (`download_log`) that every peer logs through, in the cli, the
server and the web app alike.

### serving while downloading

a fetcher is a peer from the moment it has the manifest: it claims a slot
with the "holds every piece" flag clear and answers requests for the pieces
it has verified. it can prove them, because the proof that came with each
piece is the same proof the next fetcher needs. it keeps the hashes in a
sparse copy of the tree, where pieces that share a parent share its hash.
once the last piece verifies it rewrites its slot with the flag set.

the app serves for as long as the download stays on screen: until the user
dismisses it or closes the app. the cli's `fetch` exits when the
file is saved, so it does not serve yet.

### libraries

a library is a peer that shares many files at once and can be browsed and
searched. libraries are kept by `dana serve` (see "the server and its
remote control"). bindu, the mini app, and dana's `send` and `fetch` share
one file at a time and answer the requests below with an error, which a
library takes to mean "not a library".

a library serves every file over one route and holds a slot in each file's
record, with bit 1 of the slot flags set. its files are therefore ordinary
shares: each has a link, and dana and the apps download them and join their
swarms without knowing a library is involved. (one server shares any number
of such libraries: see "a node's libraries".)

three more requests are answered over a library's route. a file is described
by what a library knows of it:
`{"name":"a.bin","size":123,"root":"<64 hex chars>","folder":"live/1999"}`.
the size and root are all it takes to derive the share keys, and so the
token, of a result; the name is what this library calls it. `folder` is
where the file is inside the library, `/`-separated, with no slash at
either end and empty at the top. the name of the folder on disk is no part
of it: a library is called what its owner chose.
it is for display only: a reader shows it, never writes to it, and takes a
missing one as the empty string.

- catalog. request: 5 bytes, `0x03` then an offset as a big-endian u32.
  reply: `0x00` then `{"total":<n>,"library":"<name>","files":[...]}`: the
  library's files in a fixed order (by folder, then name, then root),
  starting at the offset, as many as fit in `PAGE_BYTES` = 30000 bytes of
  json. `total` is the size of the whole catalog. `library` is the name its
  owner gave the library, at most `NAME_BYTES` = 64 bytes of utf-8 and
  possibly empty; a reader shows it as it would any text from a stranger,
  and takes a missing one as empty. nothing makes a name unique or true.
- search. request: `0x04` then a utf-8 query of 1 to `QUERY_BYTES` = 256
  bytes. reply: as for the catalog, holding the files whose name contains
  every whitespace-separated word of the query, ignoring case. `total`
  counts all matches; only the first page is sent.
- peers. request: `0x05`, then optionally the caller's own route blob.
  reply: `0x00` then up to `PEERS_MAX` = 16 routes of other libraries,
  each as a big-endian u16 length and the blob. mudita answers with the
  libraries in the lobby's slots as it last read them, in slot order, less
  its own route and the caller's, and remembers nothing of the caller: it
  tells nothing the lobby does not, so that two libraries on one node never
  hand out one private list. it sends no route of its own when it asks.

a library finds others in three ways: the slots flagged as libraries in the
records of the files it shares or downloads; the peers request; and the
lobby, a rendezvous record every library knows. the lobby's share keys are
derived as for a file with a root of 32 zero bytes and size 0, which no real
file has. it has no manifest; its slots are claimed and kept like any
others, by up to 8 libraries at a time, and read by all of them, again
every 60 seconds: a slot written moments ago is often not yet visible to
another node, so one read at startup misses libraries that are there. a
library that holds a lobby slot is listed; one that holds none is reached
by its link alone, and reads the lobby all the same.

the first request over a freshly imported route often times out, so a
library is asked 3 times before it is given up on and forgotten.

#### library record and link

a library's route changes, so a link to a whole library points at a record
the library keeps current instead: a veilid dht record with the DFLT schema
and one subkey, owned by a keypair the library makes once and keeps, like
the record key, between runs. unlike a file's record nothing about it is
derived: only the library can write it.

subkey 0 holds one peer slot, encoded as in a file's record, with the
"complete" and "library" flags set: the library's current route, and in the
name field the library's own name. (a library's slots in the lobby carry its
name the same way.) it is
rewritten at every heartbeat and route change, and with no route when the
library shuts down, so a reader finds a library that is gone to be vacant or
stale, and says it is offline.

the library token is that record's key string, and a library link is
`<base url>#library/<token>`, percent-encoded like a share link. every input
that takes a share link also takes a library link or `library/<token>`.

a reader opens the record, imports the route, and pages through the catalog
with the catalog request. from there each file is an ordinary share: its
token is derived from its size and root. bindu shows the catalog as
folders and files and downloads the one that is picked; it needs no library
of its own to read someone else's.

search is done by the asker, not relayed: it sends the query to the
libraries it knows and asks each for its peers, widening the set as it goes,
up to `SEARCH_PEERS` = 32 libraries per query. no library forwards a query
for another, so a query cannot be amplified through the network.

#### a node's libraries

one `dana serve` shares any number of libraries: each a name of its
owner's choosing over one or more folders, with every shared folder in
exactly one. the folder downloads are saved to is in one too, called
"downloads" until it is renamed. a library is listed, which is to be found
by searches, only where its owner says so: every new one, however it is
made, starts out reached by its link alone, and so do the downloads. the
one exception is the library a server from before already had (below),
which stays as findable as it was.

to the network each is a library like any other, and nothing says which of
them are on one node. there is no library that stands for them together:
no record, route, link or name of the node as a whole.

- each has a veilid private route of its own, and a library record of its
  own (a key and an owner keypair made apart from every other's), so a link
  of its own. renaming it keeps the record; removing it forgets the record,
  and a later library of the same name is a new one.
- a request is answered by the library whose route it arrived over (veilid
  says which), and by that one alone: its catalog and searches hold its own
  files, at their paths inside it; it serves pieces of its own files and
  answers for any other file, also one in another library of the node, in
  the words of a peer that has never had it (`not shared here`); peer
  exchange covers its own files' swarms. no reply names another route of
  the node.
- a file's slot in its record carries the route of the library the file is
  in, so the library flag there leads to that library's catalog. the same
  bytes in two libraries are two slots with two routes, as two unrelated
  libraries with one file would be.
- a listed library holds a lobby slot under its own name and route. an
  unlisted one holds none.
- what the server itself downloads is fetched and served on as the library
  downloads are saved in: that one's route is given in peer exchange and in
  the slot claimed while downloading.
- every peer keeps its own heartbeat, each beat a random share sooner than
  the last allows, so the slots of two libraries are not written in step.

a node without libraries (`send`, `serve --no-library`, a page in a
browser) has one peer and one route, which answers whatever arrives. a
server with libraries has such a peer too, for the single files a client
hands it (`share.add`): it is no library and names none.

what this does not hide:

- all of a node's routes come and go together: its libraries appear when
  the server starts, vanish when it stops, and replace their routes in the
  same moments of bad network. someone who watches several links over time
  can guess they are one node's.
- their routes are built by one node out of the relays it knows. this
  relies on veilid's private routes not saying which node is behind them,
  and on its dht writes going out over safety routes, as they do by
  default; neither was tested here.
- two libraries of a node answer the peers request from one reading of the
  lobby, so they change their answer in the same minute. the answer itself
  is what anyone reading the lobby then would give.
- a slot says when it was written by its writer's clock. a node whose
  clock is wrong by some seconds is wrong by the same seconds in every
  slot of every library, which tells them from the slots of nodes that
  keep time. a clock set from the network gives nothing away.
- the same file in two libraries is evidence of nothing, but a rare one in
  both, or files that only make sense together, is the owner's to judge.
- a library's name and file names are chosen by the owner and shown to
  everyone with its link.

a server from when a node had one library (a config with `folders`, and
maybe a `name`) has that library still: one library with all those folders,
called what it was called or else what its first folder is, listed as it
was, at the same record and so the same link. what the link shows has
changed shape: a file used to be under the name of the folder on disk it
was shared from, and is now at its path inside the library, so two folders'
files meet at the library's top. the download folder, which used to be part
of that library, is a library of its own now.

## app layers (`lib/`)

- `core/` - the app's one way to the network. `Core` is the interface: any
  control method by name, plus the things that involve the user's own
  files (choose a file and share it, share one dropped on the app, download
  to where the user says, hand a finished download over, discard one). it
  has two forms. on a desktop
  (`core_io.dart`, `server.dart`) the core is a `dana serve` process
  reached over its socket, and files are paths. in a browser
  (`core_web.dart`) it is the wasm module loaded into the page, files are
  browser File objects, and a download waits in the browser's storage
  until the user saves it. a file dropped on the page is seen only in a
  browser (`Core.drops` is null on a desktop, and the screen then does not
  invite a drop). the core's log lines are passed on
  through `package:logging` in both, so the app's log is the core's log.
- `ui/` - bindu's one page, and what both entrypoints share (the log,
  formatting, waiting for the core, and the look). the look is one set of
  measures, named in `app_shell.dart` (spacing, corners, heights, the row
  grid, colours), and a few pieces drawn to them in `parts.dart` (one
  card, one row, one button, one field); no widget carries a number of its
  own. the frame of either app's window is `AppFrame`: a bar along the top
  with the transfers button and the connection chip, the column of the
  sides under a switch, and the log in a bar along the bottom that shows
  its latest line and opens into a panel of every line.
  - transfers (`transfers_panel.dart`, in both apps) are off the page. the
    button in the top bar says how many are under way, downloads and
    other people's downloads from this node together, over a fill as wide
    as they are far along (pieces there over pieces in all; one whose
    size is not known yet is left out). it opens the panel, a drawer down
    the right side under the top bar with a shade over the rest, put away
    by its x, the shade, or the button. its head repeats the count with
    one bar for all; under it "downloading" has a row for each of the
    user's downloads and "sharing" one for each of other people's, those
    under way first, a section with nothing in it left out. a row is the
    name and, under it, a line about it. a download under way has its
    size, since when, a bar and "cancel"; one saved has when, "dismiss",
    and a small button to show it in its folder on a desktop or to save
    it again in a browser; one that failed has why in place of that line.
    other people's downloads have nothing to press: the library the file
    is in if any, who, and since when, or when it finished or they went
    away. those come from the `uploads` method; a core without it is no
    trouble, and bindu then still lists who is downloading its one file
    from the `downloads` of the `shares` reply. the panel only ever holds
    what was asked for: the app opens it when a download is started.
  - share: one file at a time. with nothing shared there is only a place
    to add a file (the file dialog, or a drop on the page in a browser).
    the file's card takes that place, with its link and a button to copy
    it, until sharing is stopped. a file dropped meanwhile is refused, as
    are several dropped together.
  - download: a row that takes a link or token, typed or pasted, and
    under it a card (`PendingCard`) with a row for each link given that
    has not been asked for: being looked up, found (its name and size,
    the names to choose from when it is known by several, "download", and
    an x), or leading nowhere, with why. nothing is downloaded until the
    user asks; "download" takes the row off the page and makes a transfer
    of it. a transfer stays in the panel, through downloading, saved or
    failed, until it is dismissed, and dismissing one discards its
    download in the core. a link the app was opened with, or one that
    arrives while it is open, brings this side forward. a link to a
    library opens it under the card, in the browser the desktop app has
    (`LibraryBrowser`, below), without what belongs to a list of
    libraries: the way starts at the library, there is no arrow back, and
    an x closes it. a file picked out of it is downloaded at once and
    shows among the transfers. another library link takes its place.

  `BinduModel` follows the core's state by asking again every second and
  holds the one share, the list of received files, and other people's
  downloads from the core.
- `e2e/` - the `window.muditaTest` hook, compiled in only for e2e builds.
- `desktop/` - mudita, the full desktop app, for linux
  (`lib/desktop/main.dart`). `LibraryModel` follows the server's state by
  asking again every two seconds, and passes on what the user asks for;
  every control method the window uses is named in `library_model.dart`.
  a file is downloaded once at a time, wherever it is asked for: the model
  notes each file asked for by its token, from the press on, and offers it
  again only when its download has failed or been taken off the list.
  its `ui/` is built from the same parts and measures as bindu's page, on a
  wider column, in the same frame: the bar along the top with the
  transfers button and the connection chip, a switch between four sides,
  and the log in the bar along the bottom. a download asked for on any
  side opens the panel of transfers.
  - search: a row with the field, then a card per file found, filling in
    as libraries answer: its size, how many libraries have it, what else
    it is called, a link to copy, and "download", or "show in folder" once
    a finished download has its name and size.
  - browse (in the code, discover): other people's libraries as a small
    file browser, one level at a time in one card, under a row that opens
    a library by its link from any level. the head of the card is a bar
    of its own holding the way to where one is: "libraries", the library,
    each folder gone into, every part but the last leading back to its
    level.
    - the top level (`LibraryList`) is the libraries the server has
      discovered, a row each: its name, when it was last seen (amber when
      not within six minutes, which is when a slot goes stale), and how
      many files it has when the server says. "refresh" in the head has
      the server look again; with none found the card says so, once the
      server has answered.
    - inside a library is `LibraryBrowser` (`lib/ui/`), the one view of
      a library in both apps: it is handed a way to read a page of the
      catalog and a button for each file, reads every page by itself
      (`LibraryListing`), and sets the files out by `fileTreeOf`, the tree
      the share side shows too: the folders of the level by name, each
      with how many files are in and under it, then its files by name
      with their sizes. the head starts with an arrow one level back
      (from the library's top, to the list) and ends with "copy link"
      when the server gives the library's link. it speaks of the listing
      only while pages are arriving or when one failed, with "refresh".
    - a row that opens something, a library or a folder, is pressed
      anywhere and says so with a small arrow after its name. a file's
      row starts with a button that says where the file stands
      (download, downloading, show in folder), as an icon with a tooltip.
    - the place is kept while the user is on another side; opening a
      library again starts at its top. one opened before the network is
      up waits and lists once it is.
  - download (in the code, downloads): the row of bindu's download side,
    and its card of what waits: a file link is looked up and downloaded
    only when the user asks, and a bare token offers the names the file is
    known by (`LinkLookups`, which is bindu's model with the download
    handed to the library). a library link given here opens on the browse
    side.
    the panel of transfers lists every download of the server's: saved
    ones with "show in folder" (`xdg-open` on its folder), failed ones
    with "try again" for one this window started, since the server does
    not say what a download was asked with. when a download started is
    known only for one this window started, and when it finished only
    for one seen finishing.
  - share (in the code, shared): the user's own libraries. a row names a new one and chooses
    its first folder; with no name the server calls it what the folder is.
    a library someone is downloading from says how many transfers it has,
    in the accent colour, after its caption.
    a card per library, one open at a time: its name, edited in place,
    "copy link", "stop sharing" after a confirmation (never for the
    library downloads go to), whether it is discoverable, its folders,
    and its files as the tree they are on disk, each with a link that has
    its name in it.

on linux bindu starts a server of its own (`serve --no-library`, on a
socket private to that run) and the desktop app uses the user's library
server, starting it if none is running. android and ios have no
core yet: they would need it as a native library.

mudita ("sympathetic joy") is the project and its full desktop app. bindu
("a drop") is the mini app, on every platform, and stays small: one file is
sent at a time, and files are received one by one. features that go beyond
one file are built in mudita. dana ("giving") is the command line and the
server both linux apps run on. the two linux apps are one flutter project
and one runner: the makefile gives each bundle its binary name and, through
`MUDITA_APP_ID`, its application id.

the rust side is a cargo workspace at the repo root.

- `core/` (crate `mudita-core`) is everything that is the same wherever it
  runs: `protocol`, `merkle`, `members` and `download_log` are pure;
  `node` owns the veilid node; `peer`, `seed` and `fetch` share and
  download; `library`, `library_record`, `neighbors` and `downloads` make a
  library; `control` is the remote control's methods; and `serve` holds the
  `Engine` that ties them together and answers a control request. it has no
  tokio, no files, no sockets and no clock of its own: timers and tasks come
  from veilid's helpers (`rt`), and the embedder supplies where bytes come
  from and go to (`Source`, `Sink`, `Sinks`), where small state is kept
  (`StateStore`), and what a library's folders hold (`Host`). that is what
  lets the same code run in a browser.
- `cli/` (the `dana` binary) is the native embedder: arguments, the log
  subscriber, the unix socket, signals, config and state files, and
  folders on `std::fs`. `send`, `fetch` and `serve` all run on the engine;
  the first two as a fresh node with throwaway storage.

## the server and its remote control

`dana serve` is a long-standing server (see "libraries") that
shares the folders named in its config file, as the libraries set out
there, and downloads on request. it keeps its veilid node and its leaf
hashes between runs, so a restart neither changes its identity on the
network nor reads every file again.

the config file sets out the libraries (see "a node's libraries"):
`{"downloads": "<folder>", "libraries": [{"name": "<chosen>", "folders":
["..."], "discoverable": true}]}`. `discoverable` (`listed` in a file from
before, and everywhere in the control methods) may be left out, and is then
false.
names are unique, not empty and without `/`,
and a folder is in one library. the download folder is in one of them:
where it is in none, a library "downloads" is made for it, unlisted. a
file written when a server had one library has `folders` and maybe a
`name` instead: they become one library (see "a node's libraries"), and
the file is written in the new shape when it is next changed. the records
of the libraries are kept in the state directory by name
(`libraries.json`; the `library.json` of a server from before stays where
it was, and the library it became takes its record over): a name changed
by editing the config file, not by `library.rename`, makes a new record
and so a new link.

everything else talks to it over a unix socket, `$XDG_RUNTIME_DIR/
mudita.sock` (under the state directory when that is unset; `--socket` or
`MUDITA_SOCKET` overrides it). the socket is bound before the node attaches,
so a client connects within a second of the server starting and sees
`"ready": false` until the network is usable. every cli command but `send`,
`fetch` and `serve` is a client of it, and so is the desktop app,
which is why they answer in the time of one request: the node is already
up.

the window starts a server when it finds none, with `serve --attached`: such
a server watches its stdin, which closes when the process that started it
exits for any reason, and then shuts down as on SIGINT, vacating its slots.
so the server goes when the window does, even a window that was killed. a
server the window found running, it leaves running.

a server that is stopping gives up its socket first and its slots after:
leaving the swarm takes about a second per shared file, and a window opened
in that time must not take the server that is leaving for one that is
running. the next server waits up to 30 seconds for the state directory,
which the one leaving holds until it is gone.

the control protocol is newline-delimited json; one connection carries many
requests. a request is `{"id":<int>,"method":"<name>","params":{...}}`, and
its answer `{"id":<same>,"result":{...}}` or `{"id":<same>,"error":
"<message>"}`. a file is `{"name","size","root","folder"}`, as in a catalog; where the
server hands one out (`browse`, search results) it adds `token`, the file's
share token, and search results add `names`, every name the libraries asked
gave that file, the most common first, with `name` the first of them.

| method | params | result |
| --- | --- | --- |
| `status` | | `ready`, `folders` (every shared folder but the download folder), `downloads` (the folder), `files`, `shared` (counts), `discovered` (how many of other people's libraries are known), `uploading` (how many downloads others are making from this node just now), `libraries`: the server's own, each `name`, `folders`, `listed`, `token` and `link` (its own, null until its record is published), `files`, `shared` |
| `libraries` | | `libraries`, as in `status` |
| `files` | `offset`, `limit` | `total`, `files`: each `path`, `name`, `library` (the one it is in), `size`, `state` (`hashing`, `publishing`, `shared`, `error`), `progress`, `served`, `token`, `link` (the share link with the name in it, null until shared), `error` |
| `library.add` | `name`, optionally `listed` (false unless said) | as `status`; a new library with no folders, reached by its link alone unless it is asked for listed |
| `library.rename` | `name`, `to` | as `status`; its link stays |
| `library.set` | `name`, `listed` | as `status`; takes or gives up its lobby slot |
| `library.remove` | `name` | as `status`; the one downloads are saved in cannot be removed |
| `folders.add`, `folders.remove` | `path`, optionally `library` (the name of one of the server's own) | as `status`; rewrites the config and rescans. without `library` a folder is added as a new library called what the folder is, not listed, and removed from whichever has it, which goes too if that leaves it empty. the download folder cannot be removed |
| `rescan` | | |
| `discover` | | `libraries`: other people's, each an `id`, 16 hex chars of the hash of its route, a `name`, null until the library has answered once, and `seen`, the unix seconds at which it last answered or its fresh slot was last read, null for one only heard of from another library; and where they are known, `files`, how many files its catalog has (once a page of it has been asked for), and `link`, its library link (only where it was reached by that link: the lobby gives a route, not a record) |
| `discover.refresh` | | reads the lobby again |
| `browse` | `library` (an id) or `link` (a library link), and `offset` | `total`, `library` (its name), `files`: one catalog page |
| `search.start` | `query` | `search`: an id |
| `search.poll` | `search` | `asked`, `done`, `results`: files with `sources`, and `libraries`, what the libraries that have the file are called (those that have a name) |
| `search.last` | | as `search.poll`, for the most recent search; how `dana download <number>` finds its file |
| `download` | `file` or `token` (a link or token), and optionally `name` (what to save it as, over any name in the link) and `path` (native: save exactly there) | `download`: an id |
| `lookup` | `link` | `token`, `size`, `root`, `name` (from the link, else the most common in the slots, else empty), `names` (every distinct name the peers in the slots give, most common first) |
| `shares` | | `shares`: each `id`, `name`, `size`, `state` (`hashing`, `publishing`, `shared`, `error`), `progress`, `served`, `token`, `link` (the share link with the name in it), `error`, and `downloads`: who is downloading the file from here or has been, oldest first, each `id` (the 16 hex characters the log calls it by), `done` and `total` (pieces), `state` (`downloading`, `finished`, or `gone` for one not heard from for `DOWNLOAD_GONE`), `started` and `seen` (unix seconds of its first and its latest notice). it is what the downloaders' own notices say and no more (see "download notices"): one whose notices were lost is missing or behind, one heard from again after `gone` starts over, and of those that have ended the last 64 are kept, until the share is stopped |
| `uploads` | | `uploads`: every download others are making from this node or have made, of files shared on their own and of every library's, those under way first and then those that have ended, the newest first in each. each is as an entry of a share's `downloads`, with `name` (the file, as this node shares it), `library` (the library it is in, or null for a file shared on its own) and `size` (bytes). for the node's owner only: nothing of it is sent to the network |
| `share.add` | `path` (native) | `share`: an id |
| `share.stop` | `id` | |
| `downloads` | | `downloads`: each `id`, `name`, `size`, `state` (`resolving`, `downloading`, `done`, `error`), `done`, `total`, `error`, and `saved` (where a finished one is: a path, or a name in the browser's storage) |
| `download.cancel` | `id` | |

the socket is private to the user (mode 0600) and that is its only access
control: whoever can open it can share any folder that user can read.

## logging

both halves report what they do through their platform's standard logging
library, with the same messages, so every line carries a time and a level.

| message | level (cli / app) | side |
| --- | --- | --- |
| `serving from slot <n>` | INFO | sharing |
| `swarm is full, not serving` | WARN / WARNING | sharing |
| `sharing <name>` | INFO | sharing |
| `download <id> started (<n> active)` | INFO | sharing |
| `download <id> finished in <s>s (<n> pieces served in total)` | INFO | sharing |
| `download <id> of <name> at <p>% (<done>/<total> pieces)` | INFO | sharing |
| `download <id> of <name> went away at <p>%` | WARN / WARNING | sharing |
| `route replaced` | WARN / WARNING | sharing |
| `reply failed: <error>` | WARN / WARNING | sharing |
| `fetching <name> (<size> bytes)` | INFO | downloading |
| `found <n> peers` | INFO | downloading |
| `piece <n> failed, retrying: <error>` | WARN / WARNING | downloading |
| `downloaded <name>` | INFO | downloading |
| `download failed: <error>` | WARN / WARNING (ERROR when `fetch` fails) | downloading |

every node also logs `connecting to veilid`, `hashing <name>` and
`<done>/<total> pieces (<percent>%)` at each 10 percent of a download, and
the cli logs `link: <url>` and `saved <path>`, all at INFO.

- **cli**: the `tracing` crate, with one `tracing-subscriber` fmt layer
  writing to stderr. a line is an rfc 3339 utc timestamp, the level, the
  module, and the message:
  `2026-10-03T21:29:43.512345Z  INFO mudita::peer: download 3f9a1c0e7b2d4a68 started (1 active)`.
  our own events show at INFO and above: the core's under `mudita::`, and
  what `cli/` itself logs under `dana`. veilid-core logs through
  `tracing` too, under targets named after its facilities (`net`, `rpc`,
  ...), and stays silent until `RUST_LOG` asks for it in the usual
  `target=level` syntax: `RUST_LOG=debug` for all of veilid,
  `RUST_LOG=mudita=debug` for mudita's own diagnostics,
  `RUST_LOG=net=debug,mudita=warn` to pick. `main.rs` installs the only
  global subscriber; veilid's `VeilidTracing` helper is not used because it
  would install a second one. stdout carries nothing but the share token,
  and a usage error is printed plainly because logging is not up yet.
- **app**: `package:logging`. the app logs no protocol events of its own:
  it passes on the core's. on a desktop each line the server writes is
  logged under `mudita.server` at the server's level; in a browser each
  core event is logged under its module (`mudita::peer` as
  `mudita.peer`), with DEBUG as FINE. `setUpLogging` (`app_shell.dart`)
  configures the root logger once and, in a debug build, echoes every
  record to the console. the log shows the records of the
  `mudita` loggers at INFO and above, newest first, as
  `14:29:43 INFO download 3f9a1c0e7b2d4a68 started (1 active)`, and keeps the
  last 200.

## starting a node

a node is of use once veilid calls it ready, which most are within ten
seconds. about one in seven started in a browser is attached as soon, with
peers, and is still not ready ten minutes later; detaching and attaching
again, and shutting the node down and starting it from the same storage,
change nothing, while a node with fresh storage starts as usual. the cause
was not found. so a node that is not ready after 40 seconds is shut down
and started again as a new node, with what veilid kept for it (its
identity, and what it remembered of the network) deleted, for five
attempts in all before the failure is reported. nothing of mudita's own is in that storage:
a library's link and name are in its own state file.

the app shows one wait for all of this. a link opened before the network is
there (the page was opened with it) is looked up once it is.

the icon is drawn in `icons/` and rendered to the sizes the web and linux
apps use by `scripts/render_icons.sh`; the results are checked in.

## where a download goes (R7)

a fetcher never holds the file. each piece is handed to a piece store as soon
as it verifies:

- the cli and the native apps write pieces at their offsets into
  `<destination>.part` and rename it once the last piece is in. the native
  app asks for the destination before downloading, because a temp directory
  is often a ram disk.
- the web app cannot write to the user's disk directly, so it groups pieces
  into chunks of 256 pieces (about 7.5 MiB) and keeps them in the browser's
  storage. saving builds one file out of references to those chunks and
  hands it to the browser as a download, so the file is never loaded whole.
  there are two stores, chosen when the core starts (`wasm/src/storage.rs`)
  and named in its first log line:
  - the origin private file system, with web locks marking which tab holds
    what. this is the long-term store, used whenever the page is a secure
    context (https, or localhost).
  - indexeddb, one database per download holding a blob per chunk. this is
    a WORKAROUND for plain http, where the file system and locks do not
    exist, and the hosted app is on plain http because a veilid node in a
    page cannot yet be used from https. when it can, the app moves to
    https and this store (`wasm/src/indexed.rs`) is removed.
  what a download kept is deleted when it is discarded or replaced. what a
  closed tab left behind is deleted when a page next starts the core; a
  database a live tab still holds open is spared.

the app's stores also read pieces back, which is what lets a download serve
them: from the file on native, and on web from memory while a chunk is still
filling and from a slice of its blob afterwards.

a piece is retried through about ten minutes of trouble before a download is
given up, because routes die and requests time out in bursts over a long
transfer.

## limitations

- a library writes one slot per file at every heartbeat, four at a time.
  that is fine for hundreds of files and not for many thousands; past that
  it would have to announce only the files someone asks for.
- the libraries of one node are strangers to each other only as far as "a
  node's libraries" says: they share a node, and so its uptime. a file
  that is in two of them takes two of its record's 8 slots. every library
  costs the node one more private route to keep alive.
- a library's link depends on the name it is kept under: renamed in the
  config file by hand, it gets a new record and a new link.
- nothing shows all of one node's libraries together to someone else: the
  owner gives out each link that is wanted.
- a library's name is whatever its owner typed: two can share one, and one
  can claim another's. only its link identifies it. search matches file
  names only.
- the lobby holds 8 libraries. others are found through them, or through
  the files they share, so a library nobody has met yet can go unseen.
- nothing a library says about its files is trusted or needs to be: a
  result is a name, size and root, and a download verifies against them.
  but nothing stops a library from listing files it will not serve.
- a record holds 8 peers. more than that are found through peer exchange.
- a fetcher does not know which pieces a downloading peer holds, so it asks
  and is told no, which costs a round trip each time.
- a share is found only through its record, so a file stays reachable for as
  long as one peer of it is running. the cli's `fetch` does not serve.
- everyone with a link can write its record. they cannot change what the
  link leads to, but they can fill the slots with routes that go nowhere.
- downloads cannot resume across restarts, and a download that fails throws
  away what it had. throughput is a few hundred kilobytes per second, so a
  multi-gigabyte file takes hours.
- the web app needs free browser storage for the whole file, and the browser
  then copies it into the download folder, so a download briefly needs twice
  its size on disk.
- bindu and `send` share one file at a time, besides the one being
  downloaded.
- the manifest is not rewritten periodically. the slots are, at every
  heartbeat, which has not been shown to keep the whole record alive in the
  dht through a very long share.
- saving a download is not implemented on android and ios.
