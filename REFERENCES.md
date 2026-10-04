# references

other projects worth reading before changing the protocol or the roadmap.

## stigmerge

https://github.com/cmars/stigmerge (mirror: https://codeberg.org/cmars/stigmerge)

a rust command line that sends and receives files over veilid, by a
different author. MPL-2.0. read at release 0.6.3 (2026-07-19, commit
8f26b50), built on veilid-core 0.5.6. it solves the same problem as mudita
and also has a swarm, built differently (see below). it has no app: its readme lists a native ui as a plan, leaning toward
tauri over flutter.

layout: `stigmerge-fileindex` (indexing and hashing a payload),
`stigmerge-peer` (the protocol and swarm), `stigmerge` (the cli).

### how it works

- `stigmerge seed <file>` prints a dht key; `stigmerge fetch <dht key>`
  downloads. every fetcher seeds what it has under its own dht key while it
  is still downloading.
- wire format is cap'n proto (`stigmerge-peer/proto/stigmerge.capnp`).
- a share is one dht record per peer. subkey 0 is a header: the payload's
  sha-256 and length, the peer's private route blob, the piece size, and the
  keys of two more records. the following subkeys hold the index: a sha-256
  per piece plus the file names.
- pieces are 1 MiB, split into 32 blocks of 32768 bytes. a fetcher asks for
  one block per app_call (`BlockRequest{piece, block}`) and gets the raw
  block back. a piece is verified against its digest once all 32 blocks are
  in.
- swarm: each peer publishes a "have map" record (a bitmap of its pieces)
  and a "peer map" record (the share keys of peers it knows). a peer
  announces itself to the peers it fetches from with an app_message
  (`AdvertisePeerRequest`). because the peer map lives in the dht, a new
  fetcher can follow a seeder's last-known neighbours even after that seeder
  has gone offline.
- fetching is coordinated by a piece lease manager
  (`stigmerge-peer/src/piece_leases/DESIGN.md`): a piece is leased to one
  peer at a time, leases expire, and peers are ranked by how many pieces
  they delivered or failed.

### how it differs from mudita

| | mudita | stigmerge |
|---|---|---|
| encoding | hand-rolled bytes and json | cap'n proto |
| hash | blake3 | sha-256 |
| unit fetched | 30720 byte piece with its merkle proof | 32768 byte block, no proof |
| verified | every piece on arrival | every 32 blocks (1 MiB) |
| what the dht holds | one small manifest with a merkle root | the whole list of piece digests |
| size ceiling | none from the protocol | about 32 GB, from the index size |
| swarm | built: 8 peer slots in one shared record, peer exchange | built: have maps, peer maps, gossip |
| multi-file | no | in the wire format, not finished |
| clients | flutter app (web, linux) and a cli | cli only |

the two are not wire compatible in any part.

### ideas worth borrowing

- per-peer share records plus a peer map in the dht, as the design for
  swarm discovery: an alternative to mudita's shared-writer rendezvous
  record, which needs no tracker either.
- have maps as bitmaps in the dht, so a fetcher can pick peers that hold
  what it still needs without asking each one.
- piece leases and peer ranking, to choose better among the peers a fetch
  draws on (mudita rotates through them and sets aside those that fail).
- a state directory, so an interrupted fetch resumes (mudita cannot resume).
- its plans: magnet links carrying the digests and some bootstrap peers,
  multi-file shares.

### what adopting its protocol would take

making mudita a stigmerge peer is plausible but is a rewrite of the core's
protocol (`core/`), which the cli, the server and the web app all run:

- cap'n proto encoding and decoding in rust for the handful of structs
  used, built for wasm as well as native.
- sha-256 in place of blake3, in rust for both builds alike.
- reading the index out of dht subkeys, and to seed, publishing a header,
  index, have map and peer map and keeping them current.
- giving up the merkle root: files over about 32 GB stop working, and a bad
  block is only noticed after the rest of its megabyte has been fetched.

not verified: whether stigmerge peers and a veilid node running in a browser
interoperate in practice, how its throughput compares with mudita's, and
whether its record keys carry an encryption key the way mudita's tokens do.
none of it has been run here; this is from reading the source.
