//! wire format shared with the flutter app (see DESIGN.md). everything here
//! is pure.

use crate::merkle::Hasher;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const PROTOCOL_VERSION: u32 = 4;
/// veilid caps app_call requests and replies at this size.
pub const MAX_MESSAGE_BYTES: usize = 32768;
pub const HASH_BYTES: usize = 32;
/// leaves room in a reply for the status byte and a 63 hash proof.
pub const PIECE_BYTES: u64 = 30 * 1024;

pub const MANIFEST_SUBKEY: u32 = 0;
/// peers that can serve one file at a time, one record subkey each.
pub const PEER_SLOTS: u16 = 8;
pub const FIRST_SLOT_SUBKEY: u32 = 1;
pub const RECORD_SUBKEYS: u16 = 1 + PEER_SLOTS;
/// a slot not rewritten for this long, by the reader's clock, is vacant.
pub const SLOT_STALE_SECONDS: u64 = 360;
pub const SLOT_HEARTBEAT: Duration = Duration::from_secs(120);
/// failures in a row before a peer is passed over for the others.
pub const PEER_STRIKES: u32 = 3;
/// how often a running download looks for peers that joined since.
pub const PEER_REFRESH: Duration = Duration::from_secs(60);
/// how long a piece is tried for, round after round, before the download
/// gives up. a round asks every known peer once; rounds are quick now that
/// they do not wait on the slots, so the patience is kept in time, not in
/// a count of them.
pub const PIECE_PATIENCE: Duration = Duration::from_secs(600);
/// how long one peer gets to answer a piece request. measured answers took
/// 0.25s at the median and 1.3s at worst, and veilid itself gives up after
/// 5s, so this moves on to the next peer well before that.
pub const PIECE_TIMEOUT: Duration = Duration::from_secs(3);
/// the slots are read again after a failed round no more often than this.
pub const SLOT_REREAD: Duration = Duration::from_secs(15);
/// the most peers a download fetches from at once.
pub const SWARM_MAX: usize = 32;

// these continue the merkle tree's domain separation prefixes.
const OWNER_PREFIX: u8 = 2;
const ENCRYPTION_PREFIX: u8 = 3;
const SLOT_COMPLETE: u8 = 1;
const SLOT_LIBRARY: u8 = 2;
// the flags and the time; the name's length follows, then the name.
const SLOT_HEADER_BYTES: usize = 9;
/// the longest name a slot carries, in utf-8 bytes.
pub const NAME_MAX: usize = 255;
/// what a file is saved as when nobody has given it a name.
pub const FALLBACK_NAME: &str = "download";
/// where the web app is hosted. share links open there.
pub const LINK_BASE: &str = "http://veilid.tech/mudita/";
/// what the fragment of a link to a library starts with.
pub const LIBRARY_PREFIX: &str = "library/";
// what parts a file's token from its name in a link.
const NAME_MARK: char = '/';
/// a library's own record has one subkey: the slot naming its route.
pub const LIBRARY_SUBKEYS: u16 = 1;
pub const LIBRARY_SLOT_SUBKEY: u32 = 0;

const REQUEST_PIECE: u8 = 1;
const REQUEST_NOTICE: u8 = 2;
const REQUEST_CATALOG: u8 = 3;
const REQUEST_SEARCH: u8 = 4;
const REQUEST_PEERS: u8 = 5;
const REQUEST_SWARM_PEERS: u8 = 6;
const CATALOG_REQUEST_BYTES: usize = 1 + 4;
/// the longest search query.
pub const QUERY_BYTES: usize = 256;
/// a catalog or search reply carries as many files as fit in this much json.
pub const PAGE_BYTES: usize = 30000;
/// the most routes one peers reply carries.
pub const PEERS_MAX: usize = 16;
/// the longest name a library may have, in utf-8 bytes.
pub const NAME_BYTES: usize = 64;
/// the most libraries one search asks.
pub const SEARCH_PEERS: usize = 32;
/// the root the lobby's share keys are derived from, with a size of zero.
/// no file hashes to it: the record is only where libraries first meet, and
/// the name in each of its slots is the library's own.
pub const LOBBY_ROOT: Hash = [0; HASH_BYTES];
const LENGTH_BYTES: usize = 2;
const PIECE_REQUEST_BYTES: usize = 1 + HASH_BYTES + 4;
pub const DOWNLOAD_ID_BYTES: usize = 8;
const NOTICE_BYTES: usize = 2 + DOWNLOAD_ID_BYTES + HASH_BYTES;
// the event byte of a progress notice, which also carries a piece count.
const NOTICE_PROGRESS: u8 = 3;
const PROGRESS_BYTES: usize = NOTICE_BYTES + 4;
const STATUS_OK: u8 = 0;
const STATUS_ERROR: u8 = 1;

pub type Hash = [u8; HASH_BYTES];
/// random per download; lets a seeder tell concurrent downloads apart.
pub type DownloadId = [u8; DOWNLOAD_ID_BYTES];

/// what a share's record says of its file. it has no name: every field is
/// one the record's key is derived from, so none can be changed by someone
/// who merely has the link.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    pub size: u64,
    pub piece_bytes: u64,
    /// lowercase hex merkle root over the pieces.
    pub root: String,
}

pub fn encode_manifest(manifest: &Manifest) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(manifest)?)
}

/// parse a manifest, rejecting versions and piece sizes this build cannot read.
pub fn decode_manifest(bytes: &[u8]) -> Result<Manifest> {
    let manifest: Manifest = serde_json::from_slice(bytes)?;
    if manifest.version != PROTOCOL_VERSION {
        return Err(format!("unsupported protocol version {}", manifest.version).into());
    }
    if manifest.piece_bytes != PIECE_BYTES {
        return Err(format!("unsupported piece size {}", manifest.piece_bytes).into());
    }
    Ok(manifest)
}

/// what locates a share, all derived from the file so that everyone sharing
/// it lands on one record.
#[derive(Debug, PartialEq)]
pub struct ShareKeys {
    pub owner_secret: Hash,
    pub owner_public: Hash,
    pub encryption_key: Hash,
}

/// the ed25519 public key of a 32 byte seed, as in rfc 8032.
pub fn public_key_of(secret: &Hash) -> Hash {
    ed25519_dalek::SigningKey::from_bytes(secret)
        .verifying_key()
        .to_bytes()
}

/// the keys of the file with this root and size. its name is no part of
/// them: the same bytes are one share whatever anyone calls them.
pub fn share_keys(hash: Hasher, root: &Hash, size: u64) -> ShareKeys {
    let derive = |prefix: u8| hash(&[&[prefix][..], root, &size.to_be_bytes()].concat());
    let owner_secret = derive(OWNER_PREFIX);
    ShareKeys {
        owner_public: public_key_of(&owner_secret),
        owner_secret,
        encryption_key: derive(ENCRYPTION_PREFIX),
    }
}

/// one peer's entry in the rendezvous record.
#[derive(Debug, PartialEq, Clone)]
pub struct Slot {
    /// the peer holds every piece.
    pub complete: bool,
    /// the peer is a library: it also answers catalog, search and peers
    /// requests, which this cli never sends.
    pub library: bool,
    /// unix seconds at which the peer last wrote the slot.
    pub time: u64,
    /// what the peer calls the file, or in a library's own record and in
    /// the lobby, what the library is called. a label from a stranger.
    pub name: String,
    pub route: Vec<u8>,
}

impl Slot {
    /// a clock far off in either direction must not pin a slot forever.
    pub fn is_stale(&self, now: u64) -> bool {
        now.abs_diff(self.time) > SLOT_STALE_SECONDS
    }
}

/// as much of a name as a slot carries: cut at a character, never in one.
pub fn slot_name(name: &str) -> &str {
    let mut end = name.len().min(NAME_MAX);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// an empty route marks the slot as given up.
pub fn encode_slot(slot: &Slot) -> Vec<u8> {
    let flag = |set: bool, bit: u8| if set { bit } else { 0 };
    let flags = flag(slot.complete, SLOT_COMPLETE) | flag(slot.library, SLOT_LIBRARY);
    let name = slot_name(&slot.name).as_bytes();
    let length = (name.len() as u16).to_be_bytes();
    let head = [&[flags][..], &slot.time.to_be_bytes(), &length].concat();
    [&head[..], name, &slot.route].concat()
}

/// the peer a slot value names, or None when it is too short to hold its
/// header and name, or carries no route.
pub fn decode_slot(value: &[u8]) -> Option<Slot> {
    let (header, rest) = value.split_at_checked(SLOT_HEADER_BYTES)?;
    let (length, rest) = rest.split_at_checked(LENGTH_BYTES)?;
    let length = usize::from(u16::from_be_bytes(length.try_into().ok()?));
    let (name, route) = rest.split_at_checked(length)?;
    if route.is_empty() {
        return None;
    }
    Some(Slot {
        complete: header[0] & SLOT_COMPLETE != 0,
        library: header[0] & SLOT_LIBRARY != 0,
        time: u64::from_be_bytes(header[1..].try_into().ok()?),
        name: String::from_utf8_lossy(name).into_owned(),
        route: route.to_vec(),
    })
}

/// the names a file goes by, as the peers in its slots give them: every
/// distinct one that is not empty, the most common first, and by name where
/// two are as common.
pub fn common_names<'a>(given: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for name in given.into_iter().filter(|name| !name.is_empty()) {
        match counted.iter_mut().find(|(held, _)| *held == name) {
            Some((_, count)) => *count += 1,
            None => counted.push((name, 1)),
        }
    }
    counted.sort_by(|(a, a_count), (b, b_count)| b_count.cmp(a_count).then(a.cmp(b)));
    counted
        .into_iter()
        .map(|(name, _)| name.to_owned())
        .collect()
}

/// what a file is called, of the names on offer in order of trust: the one
/// asked for, the one in the link, the most common in the slots. empty when
/// there is none.
pub fn chosen_name(asked: Option<&str>, link: &str, slots: &[String]) -> String {
    let slot = slots.first().map(String::as_str);
    let offered = [asked, Some(link), slot];
    let named = offered.into_iter().flatten().find(|name| !name.is_empty());
    named.unwrap_or_default().to_owned()
}

/// the name to save a file under. a name is text from a stranger, so only
/// its last path component is kept, and one with none is called `download`.
pub fn save_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    match last {
        "" | "." | ".." => FALLBACK_NAME.to_owned(),
        last => last.to_owned(),
    }
}

/// an empty file still has one (empty) piece so that it has a merkle root.
pub fn piece_count(size: u64) -> u64 {
    size.div_ceil(PIECE_BYTES).max(1)
}

/// byte range of a piece within a file of the given size.
pub fn piece_range(index: u64, size: u64) -> (u64, u64) {
    (index * PIECE_BYTES, size.min((index + 1) * PIECE_BYTES))
}

/// a fetcher's announcement that a download began or ended.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Event {
    Started = 1,
    Finished = 2,
}

/// what one peer asks of another. the first two name the file by its merkle
/// root, because one route may serve many files; the rest are answered by
/// libraries only (DESIGN.md "libraries").
#[derive(Debug, PartialEq)]
pub enum Request {
    Piece(Hash, u32),
    Notice(Event, DownloadId, Hash),
    /// a download's count of pieces verified so far.
    Progress(DownloadId, Hash, u32),
    /// the catalog, from this offset on.
    Catalog(u32),
    /// the files whose names match this query.
    Search(String),
    /// the libraries known. carries the caller's own route, or nothing.
    Peers(Vec<u8>),
    /// the other peers of the file with this root. carries the caller's
    /// own route if it serves the file itself, or nothing.
    SwarmPeers(Hash, Vec<u8>),
}

pub fn encode_piece_request(root: &Hash, index: u32) -> Vec<u8> {
    [&[REQUEST_PIECE][..], root, &index.to_be_bytes()].concat()
}

pub fn encode_notice(event: Event, id: DownloadId, root: &Hash) -> Vec<u8> {
    [&[REQUEST_NOTICE, event as u8][..], &id, root].concat()
}

/// a fetcher's report of how many pieces of the file it has verified.
pub fn encode_progress_notice(id: DownloadId, root: &Hash, done: u32) -> Vec<u8> {
    let head = [REQUEST_NOTICE, NOTICE_PROGRESS];
    [&head[..], &id, root, &done.to_be_bytes()].concat()
}

pub fn encode_catalog_request(offset: u32) -> Vec<u8> {
    [&[REQUEST_CATALOG][..], &offset.to_be_bytes()].concat()
}

/// a query of 1 to QUERY_BYTES bytes; anything else is not sent.
pub fn encode_search_request(query: &str) -> Result<Vec<u8>> {
    if query.is_empty() || query.len() > QUERY_BYTES {
        return Err(format!("a query is 1 to {QUERY_BYTES} bytes").into());
    }
    Ok([&[REQUEST_SEARCH][..], query.as_bytes()].concat())
}

pub fn encode_peers_request(own_route: &[u8]) -> Vec<u8> {
    [&[REQUEST_PEERS][..], own_route].concat()
}

/// ask a peer of the file with this root which other peers of it it knows.
/// `own_route` is empty unless the caller serves the file too.
pub fn encode_swarm_peers_request(root: &Hash, own_route: &[u8]) -> Vec<u8> {
    [&[REQUEST_SWARM_PEERS][..], root, own_route].concat()
}

pub fn decode_request(request: &[u8]) -> Result<Request> {
    match request {
        [REQUEST_CATALOG, offset @ ..] if request.len() == CATALOG_REQUEST_BYTES => {
            Ok(Request::Catalog(u32::from_be_bytes(offset.try_into()?)))
        }
        [REQUEST_SEARCH, query @ ..] if !query.is_empty() && query.len() <= QUERY_BYTES => {
            Ok(Request::Search(std::str::from_utf8(query)?.to_owned()))
        }
        [REQUEST_PEERS, route @ ..] => Ok(Request::Peers(route.to_vec())),
        [REQUEST_SWARM_PEERS, body @ ..] if body.len() >= HASH_BYTES => {
            let (root, route) = body.split_at(HASH_BYTES);
            Ok(Request::SwarmPeers(root.try_into()?, route.to_vec()))
        }
        [REQUEST_PIECE, body @ ..] if request.len() == PIECE_REQUEST_BYTES => {
            let (root, index) = body.split_at(HASH_BYTES);
            Ok(Request::Piece(
                root.try_into()?,
                u32::from_be_bytes(index.try_into()?),
            ))
        }
        [REQUEST_NOTICE, NOTICE_PROGRESS, body @ ..] if request.len() == PROGRESS_BYTES => {
            let (id, rest) = body.split_at(DOWNLOAD_ID_BYTES);
            let (root, done) = rest.split_at(HASH_BYTES);
            let done = u32::from_be_bytes(done.try_into()?);
            Ok(Request::Progress(id.try_into()?, root.try_into()?, done))
        }
        [REQUEST_NOTICE, event, body @ ..] if request.len() == NOTICE_BYTES => {
            let event = match event {
                1 => Event::Started,
                2 => Event::Finished,
                _ => return Err("unknown notice".into()),
            };
            let (id, root) = body.split_at(DOWNLOAD_ID_BYTES);
            Ok(Request::Notice(event, id.try_into()?, root.try_into()?))
        }
        _ => Err("malformed request".into()),
    }
}

/// the whole reply to a notice.
pub fn encode_ok_reply() -> Vec<u8> {
    vec![STATUS_OK]
}

pub fn encode_piece_reply(piece: &[u8], proof: &[Hash]) -> Vec<u8> {
    [&[STATUS_OK][..], piece, &proof.concat()].concat()
}

pub fn encode_error_reply(message: &str) -> Vec<u8> {
    let mut reply = [&[STATUS_ERROR][..], message.as_bytes()].concat();
    reply.truncate(MAX_MESSAGE_BYTES);
    reply
}

/// a file as a library lists it: its size and root, which are all it takes
/// to derive its share keys and so its token, what the library calls it,
/// and where the library keeps it.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    /// lowercase hex merkle root.
    pub root: String,
    /// the file's directory, '/'-separated, starting with the name of the
    /// shared folder it is under. for display only: it comes from another
    /// library, so it is never used as a path to write to.
    #[serde(default)]
    pub folder: String,
}

/// some of the files a library has, and how many there are in all.
#[derive(Deserialize, Debug, PartialEq)]
pub struct FilesPage {
    pub total: usize,
    /// what the library's owner calls it; empty when it has no name.
    #[serde(default)]
    pub library: String,
    pub files: Vec<FileEntry>,
}

/// a library's name as it may be kept and sent: control characters are
/// dropped, and one longer than NAME_BYTES is refused, not cut short.
pub fn clean_name(name: &str) -> Result<String> {
    let cleaned: String = name.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim().to_owned();
    if cleaned.len() > NAME_BYTES {
        return Err(format!("a library name is at most {NAME_BYTES} bytes").into());
    }
    Ok(cleaned)
}

/// the reply holding as many of `files` from `offset` on as fit one page,
/// from the library called `library`. `files` is everything that matched,
/// so its length is the total.
pub fn encode_files_reply(files: &[FileEntry], offset: usize, library: &str) -> Result<Vec<u8>> {
    let mut page = Vec::new();
    let mut bytes = 0;
    for file in files.iter().skip(offset) {
        let json = serde_json::to_string(file)?;
        bytes += json.len() + 1;
        if bytes > PAGE_BYTES {
            break;
        }
        page.push(json);
    }
    let body = format!(
        r#"{{"total":{},"library":{},"files":[{}]}}"#,
        files.len(),
        serde_json::to_string(library)?,
        page.join(",")
    );
    Ok([&[STATUS_OK][..], body.as_bytes()].concat())
}

// what follows the status byte of a reply that is not an error. a peer that
// is no library answers a library request with an error.
fn ok_body(reply: &[u8]) -> Result<&[u8]> {
    match reply.split_first() {
        Some((&STATUS_OK, body)) => Ok(body),
        Some((_, body)) => Err(format!("peer error: {}", String::from_utf8_lossy(body)).into()),
        None => Err("empty reply".into()),
    }
}

pub fn decode_files_reply(reply: &[u8]) -> Result<FilesPage> {
    Ok(serde_json::from_slice(ok_body(reply)?)?)
}

/// the reply naming up to PEERS_MAX of `routes`, each behind its length.
pub fn encode_peers_reply(routes: &[Vec<u8>]) -> Vec<u8> {
    let mut reply = vec![STATUS_OK];
    for route in routes.iter().take(PEERS_MAX) {
        reply.extend((route.len() as u16).to_be_bytes());
        reply.extend(route);
    }
    reply
}

pub fn decode_peers_reply(reply: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut rest = ok_body(reply)?;
    let mut routes = Vec::new();
    while !rest.is_empty() {
        let (length, tail) = rest.split_at_checked(LENGTH_BYTES).ok_or("cut short")?;
        let length = usize::from(u16::from_be_bytes(length.try_into()?));
        let (route, tail) = tail.split_at_checked(length).ok_or("cut short")?;
        routes.push(route.to_vec());
        rest = tail;
    }
    Ok(routes)
}

/// whether a file name holds every word of the query, ignoring case. a
/// query with no words matches nothing.
pub fn matches_query(name: &str, query: &str) -> bool {
    let (name, query) = (name.to_lowercase(), query.to_lowercase());
    let mut words = query.split_whitespace().peekable();
    words.peek().is_some() && words.all(|word| name.contains(word))
}

/// whether a peer answered and declined, as one that lacks the piece does.
pub fn is_error_reply(reply: &[u8]) -> bool {
    reply.first() == Some(&STATUS_ERROR)
}

/// split a reply into the piece (whose length the fetcher knows from the
/// manifest) and the proof hashes that follow it.
pub fn decode_piece_reply(reply: &[u8], piece_len: usize) -> Result<(&[u8], Vec<Hash>)> {
    let (status, body) = reply.split_first().ok_or("empty reply")?;
    if *status != STATUS_OK {
        return Err(format!("seeder error: {}", String::from_utf8_lossy(body)).into());
    }
    if body.len() < piece_len || !(body.len() - piece_len).is_multiple_of(HASH_BYTES) {
        return Err("malformed piece reply".into());
    }
    let (piece, proof) = body.split_at(piece_len);
    let proof = proof
        .chunks_exact(HASH_BYTES)
        .map(|h| h.try_into().unwrap())
        .collect();
    Ok((piece, proof))
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn hash_from_hex(hex: &str) -> Result<Hash> {
    if hex.len() != HASH_BYTES * 2 || !hex.is_ascii() {
        return Err("invalid hash".into());
    }
    let mut hash = [0; HASH_BYTES];
    for (byte, pair) in hash.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
    }
    Ok(hash)
}

const fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

fn percent_encoded(token: &str) -> String {
    token
        .bytes()
        .map(|byte| match byte {
            byte if is_unreserved(byte) => char::from(byte).to_string(),
            byte => format!("%{byte:02X}"),
        })
        .collect()
}

fn without_fragment(base: &str) -> &str {
    base.split('#').next().unwrap_or(base)
}

/// a share link carries the token in the url fragment, which browsers never
/// send to the web server hosting the app, and after a slash the name the
/// sharer gives the file. with no name it is the token alone.
pub fn link_for(base: &str, token: &str, name: &str) -> String {
    let (base, token) = (without_fragment(base), percent_encoded(token));
    match name {
        "" => format!("{base}#{token}"),
        name => format!("{base}#{token}{NAME_MARK}{}", percent_encoded(name)),
    }
}

/// a link to a whole library: its token in the fragment, behind the prefix
/// that tells it from a link to one file.
pub fn library_link_for(base: &str, token: &str) -> String {
    let token = percent_encoded(token);
    format!("{}#{LIBRARY_PREFIX}{token}", without_fragment(base))
}

/// what a link, or the token out of one, leads to.
#[derive(Debug, PartialEq)]
pub enum Link {
    /// the share token of one file, and the name the link gives it, which
    /// is empty when it gives none.
    File(String, String),
    /// the token of a library's record.
    Library(String),
}

/// tell a link to a file from a link to a library, and take its token out,
/// and for a file its name. either may also be given bare: `<token>`,
/// `<token>/<name>` or `library/<token>`. the first slash ends a file's
/// token: one inside a name is percent-encoded like anything else.
pub fn parse_link(input: &str) -> Link {
    let fragment = input.trim().rsplit('#').next().unwrap_or_default();
    if let Some(library) = fragment.strip_prefix(LIBRARY_PREFIX) {
        return Link::Library(percent_decoded(library));
    }
    let (token, name) = fragment.split_once(NAME_MARK).unwrap_or((fragment, ""));
    Link::File(percent_decoded(token), percent_decoded(name))
}

// a stray '%' is taken as it is rather than refused.
fn percent_decoded(text: &str) -> String {
    let text = text.as_bytes();
    let mut decoded = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let escaped = text
            .get(at + 1..at + 3)
            .and_then(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok());
        match (text[at], escaped) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                at += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{toy_hash, vectors};

    fn hex(value: &serde_json::Value) -> Vec<u8> {
        let text = value.as_str().unwrap();
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    // the file the request vectors are about.
    fn vector_root() -> Hash {
        hex(&vectors()["requestRoot"]).try_into().unwrap()
    }

    #[test]
    fn requests_match_vectors() {
        let root = vector_root();
        for case in vectors()["requests"].as_array().unwrap() {
            let index = case["index"].as_u64().unwrap() as u32;
            assert_eq!(encode_piece_request(&root, index), hex(&case["hex"]));
            let decoded = decode_request(&hex(&case["hex"])).unwrap();
            assert_eq!(decoded, Request::Piece(root, index));
        }
        assert!(decode_request(&[9, 0, 0, 0, 0]).is_err());
        assert!(decode_request(&[1, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn library_requests_match_vectors() {
        let all = vectors();
        let catalog = &all["catalogRequest"];
        let offset = catalog["offset"].as_u64().unwrap() as u32;
        assert_eq!(encode_catalog_request(offset), hex(&catalog["hex"]));
        let decoded = decode_request(&hex(&catalog["hex"])).unwrap();
        assert_eq!(decoded, Request::Catalog(offset));

        let search = &all["searchRequest"];
        let query = search["query"].as_str().unwrap();
        assert_eq!(encode_search_request(query).unwrap(), hex(&search["hex"]));
        let decoded = decode_request(&hex(&search["hex"])).unwrap();
        assert_eq!(decoded, Request::Search(query.to_owned()));

        for case in all["peersRequests"].as_array().unwrap() {
            let route = hex(&case["route"]);
            assert_eq!(encode_peers_request(&route), hex(&case["hex"]));
            let decoded = decode_request(&hex(&case["hex"])).unwrap();
            assert_eq!(decoded, Request::Peers(route));
        }
    }

    #[test]
    fn swarm_peers_requests_match_vectors() {
        let all = vectors();
        let root: Hash = hex(&all["requestRoot"]).try_into().unwrap();
        for case in all["swarmPeersRequests"].as_array().unwrap() {
            let route = hex(&case["route"]);
            let request = encode_swarm_peers_request(&root, &route);
            assert_eq!(request, hex(&case["hex"]));
            let decoded = decode_request(&request).unwrap();
            assert_eq!(decoded, Request::SwarmPeers(root, route));
        }
        // too short to name a file.
        assert!(decode_request(&[&[6][..], &[0; HASH_BYTES - 1]].concat()).is_err());
    }

    #[test]
    fn malformed_library_requests_are_rejected() {
        assert!(decode_request(&[3, 0, 0, 0]).is_err());
        assert!(decode_request(&[4]).is_err());
        assert!(decode_request(&[4, 0xff]).is_err());
        assert!(decode_request(&[&[4][..], &[b'a'; QUERY_BYTES + 1]].concat()).is_err());
        assert!(decode_request(&[6]).is_err());
        assert!(encode_search_request("").is_err());
        assert!(encode_search_request(&"a".repeat(QUERY_BYTES + 1)).is_err());
    }

    fn file(name: &str, size: u64) -> FileEntry {
        FileEntry {
            name: name.to_owned(),
            size,
            root: to_hex(&[0; HASH_BYTES]),
            folder: String::new(),
        }
    }

    #[test]
    fn files_replies_match_vectors() {
        let case = &vectors()["filesReply"];
        let json = case["json"].as_str().unwrap();
        let page = decode_files_reply(&[&[0][..], json.as_bytes()].concat()).unwrap();
        assert_eq!(page.total as u64, case["total"].as_u64().unwrap());
        assert_eq!(serde_json::to_value(&page.files).unwrap(), case["files"]);
        assert_eq!(page.library, case["library"].as_str().unwrap());
        // the encoder's total is the length of what it is given.
        let reply = encode_files_reply(&page.files, 0, &page.library).unwrap();
        assert_eq!(&reply[1..], json.replacen('7', "1", 1).as_bytes());
        // a reply that names no library is from one with no name.
        let unnamed = decode_files_reply(b"\x00{\"total\":0,\"files\":[]}").unwrap();
        assert_eq!(unnamed.library, "");
    }

    #[test]
    fn a_library_name_is_cleaned_and_never_cut_short() {
        assert_eq!(clean_name("  the\tshelf\n ").unwrap(), "theshelf");
        assert_eq!(clean_name("the shelf").unwrap(), "the shelf");
        assert_eq!(clean_name("").unwrap(), "");
        assert_eq!(
            clean_name(&"a".repeat(NAME_BYTES)).unwrap().len(),
            NAME_BYTES
        );
        assert!(clean_name(&"a".repeat(NAME_BYTES + 1)).is_err());
        // bytes, not characters.
        assert!(clean_name(&"\u{e9}".repeat(NAME_BYTES / 2 + 1)).is_err());
        let quoted = encode_files_reply(&[], 0, "a \"b\"").unwrap();
        assert_eq!(decode_files_reply(&quoted).unwrap().library, "a \"b\"");
    }

    #[test]
    fn a_catalog_too_big_for_one_reply_is_paged() {
        let files: Vec<FileEntry> = (0..1000)
            .map(|n| file(&format!("file-{n}.bin"), n))
            .collect();
        let name = "n".repeat(NAME_BYTES);
        let page = |offset: usize| encode_files_reply(&files, offset, &name).unwrap();
        assert!(page(0).len() <= MAX_MESSAGE_BYTES);
        let first = decode_files_reply(&page(0)).unwrap();
        assert_eq!(first.total, 1000);
        assert!(first.files.len() > 1 && first.files.len() < 1000);
        let next = decode_files_reply(&page(first.files.len()));
        assert_eq!(next.unwrap().files[0], files[first.files.len()]);
        let past = decode_files_reply(&page(5000)).unwrap();
        assert!(past.files.is_empty());
    }

    #[test]
    fn an_error_reply_is_no_files_reply() {
        let refused = decode_files_reply(&encode_error_reply("malformed request"));
        assert!(refused
            .unwrap_err()
            .to_string()
            .contains("malformed request"));
        assert!(decode_files_reply(&[0, b'[', b']']).is_err());
        assert!(decode_peers_reply(&encode_error_reply("no")).is_err());
    }

    #[test]
    fn peers_replies_match_vectors() {
        let case = &vectors()["peersReply"];
        let routes: Vec<Vec<u8>> = case["routes"].as_array().unwrap().iter().map(hex).collect();
        assert_eq!(encode_peers_reply(&routes), hex(&case["hex"]));
        assert_eq!(decode_peers_reply(&hex(&case["hex"])).unwrap(), routes);
        assert!(decode_peers_reply(&[0, 0, 4, 0xaa]).is_err());
        let many = vec![routes[0].clone(); 40];
        let capped = decode_peers_reply(&encode_peers_reply(&many)).unwrap();
        assert_eq!(capped.len(), PEERS_MAX);
    }

    #[test]
    fn a_query_matches_names_holding_all_of_its_words() {
        assert!(matches_query("Linux Mint 22.iso", "mint linux"));
        assert!(matches_query("Linux Mint 22.iso", "  ISO "));
        assert!(!matches_query("Linux Mint 22.iso", "mint debian"));
        assert!(!matches_query("Linux Mint 22.iso", "   "));
    }

    #[test]
    fn notices_match_vectors() {
        let root = vector_root();
        for case in vectors()["notices"].as_array().unwrap() {
            let event = match case["event"].as_str().unwrap() {
                "started" => Event::Started,
                _ => Event::Finished,
            };
            let id: DownloadId = hex(&case["id"]).try_into().unwrap();
            let encoded = hex(&case["hex"]);
            assert_eq!(encode_notice(event, id, &root), encoded);
            assert_eq!(
                decode_request(&encoded).unwrap(),
                Request::Notice(event, id, root)
            );
            let unknown = [&[REQUEST_NOTICE, 9][..], &encoded[2..]].concat();
            assert!(decode_request(&unknown).is_err());
        }
        assert_eq!(encode_ok_reply(), hex(&vectors()["okReply"]));
        assert!(decode_request(&[2, 1, 1, 2, 3]).is_err());
    }

    #[test]
    fn progress_notices_match_vectors() {
        let (root, case) = (vector_root(), &vectors()["progressNotice"]);
        let id: DownloadId = hex(&case["id"]).try_into().unwrap();
        let done = case["done"].as_u64().unwrap() as u32;
        let encoded = hex(&case["hex"]);
        assert_eq!(encode_progress_notice(id, &root, done), encoded);
        assert_eq!(
            decode_request(&encoded).unwrap(),
            Request::Progress(id, root, done)
        );
        // a started notice is not one for want of a count, nor the reverse.
        assert!(decode_request(&encoded[..encoded.len() - 4]).is_err());
        let started = [&encode_notice(Event::Started, id, &root)[..], &[0; 4]].concat();
        assert!(decode_request(&started).is_err());
    }

    #[test]
    fn replies_match_vectors() {
        let case = &vectors()["pieceReply"];
        let piece = hex(&case["piece"]);
        let proof: Vec<Hash> = case["proof"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| hex(h).try_into().unwrap())
            .collect();
        let reply = hex(&case["hex"]);
        assert_eq!(encode_piece_reply(&piece, &proof), reply);
        assert_eq!(
            decode_piece_reply(&reply, piece.len()).unwrap(),
            (&piece[..], proof)
        );
        assert!(decode_piece_reply(&reply, piece.len() + 1).is_err());

        let error = &vectors()["errorReply"];
        let message = error["message"].as_str().unwrap();
        assert_eq!(encode_error_reply(message), hex(&error["hex"]));
        let failure = decode_piece_reply(&hex(&error["hex"]), 4).unwrap_err();
        assert!(failure.to_string().contains(message));
    }

    #[test]
    fn a_full_piece_with_the_deepest_proof_fits_one_message() {
        let reply = encode_piece_reply(&vec![3; PIECE_BYTES as usize], &[[7; HASH_BYTES]; 63]);
        assert!(reply.len() <= MAX_MESSAGE_BYTES);
    }

    #[test]
    fn manifests_match_vectors() {
        let case = &vectors()["manifest"];
        let manifest = decode_manifest(case["json"].as_str().unwrap().as_bytes()).unwrap();
        assert_eq!(serde_json::to_value(&manifest).unwrap(), case["fields"]);
        assert_eq!(
            decode_manifest(&encode_manifest(&manifest).unwrap()).unwrap(),
            manifest
        );
        let newer = Manifest {
            version: 99,
            ..manifest
        };
        assert!(decode_manifest(&encode_manifest(&newer).unwrap()).is_err());
    }

    #[test]
    fn a_manifest_holds_nothing_but_what_its_key_is_derived_from() {
        let case = &vectors()["manifest"];
        let json = case["json"].as_str().unwrap();
        let manifest = decode_manifest(json.as_bytes()).unwrap();
        assert_eq!(encode_manifest(&manifest).unwrap(), json.as_bytes());
        // a name someone wrote into one is not read out of it.
        let named = json.replacen('{', r#"{"name":"evil.exe","#, 1);
        assert_eq!(decode_manifest(named.as_bytes()).unwrap(), manifest);
        let other_size = json.replace("30720", "1024");
        assert!(decode_manifest(other_size.as_bytes()).is_err());
    }

    #[test]
    fn public_keys_match_vectors() {
        for case in vectors()["ed25519"].as_array().unwrap() {
            let secret: Hash = hex(&case["secret"]).try_into().unwrap();
            assert_eq!(public_key_of(&secret).to_vec(), hex(&case["public"]));
        }
    }

    #[test]
    fn share_keys_match_vectors() {
        let case = &vectors()["shareKeys"];
        let root: Hash = hex(&case["root"]).try_into().unwrap();
        let size = case["size"].as_u64().unwrap();
        let keys = share_keys(toy_hash, &root, size);
        assert_eq!(keys.owner_secret.to_vec(), hex(&case["ownerSecret"]));
        assert_eq!(keys.owner_public.to_vec(), hex(&case["ownerPublic"]));
        assert_eq!(keys.encryption_key.to_vec(), hex(&case["encryptionKey"]));
        assert_ne!(share_keys(toy_hash, &root, size + 1), keys);
        assert_ne!(share_keys(toy_hash, &[0; HASH_BYTES], size), keys);
    }

    #[test]
    fn slots_match_vectors() {
        assert_eq!(vectors()["peerSlots"], PEER_SLOTS);
        assert_eq!(vectors()["slotStaleSeconds"], SLOT_STALE_SECONDS);
        for case in vectors()["slots"].as_array().unwrap() {
            let slot = Slot {
                complete: case["complete"].as_bool().unwrap(),
                library: case["library"].as_bool().unwrap(),
                time: case["time"].as_u64().unwrap(),
                name: case["name"].as_str().unwrap().to_owned(),
                route: hex(&case["route"]),
            };
            let encoded = hex(&case["hex"]);
            assert_eq!(encode_slot(&slot), encoded);
            let decoded = decode_slot(&encoded);
            assert_eq!(decoded, (!slot.route.is_empty()).then_some(slot));
        }
        assert_eq!(decode_slot(&[]), None);
    }

    fn slot_named(name: &str) -> Slot {
        Slot {
            complete: true,
            library: false,
            time: 1_000_000,
            name: name.to_owned(),
            route: vec![1],
        }
    }

    #[test]
    fn a_slot_cut_short_names_nobody() {
        let whole = encode_slot(&slot_named("a.bin"));
        assert!(decode_slot(&whole).is_some());
        // without its route, inside its name, inside its length, its header.
        for length in [whole.len() - 1, 12, 10, 5] {
            assert_eq!(decode_slot(&whole[..length]), None, "{length} bytes");
        }
        // a name length that runs past the end.
        let mut lying = whole.clone();
        lying[10] = 200;
        assert_eq!(decode_slot(&lying), None);
    }

    #[test]
    fn a_long_name_is_cut_at_a_character_when_a_slot_is_written() {
        assert_eq!(slot_name("a.bin"), "a.bin");
        assert_eq!(slot_name(&"a".repeat(NAME_MAX + 9)).len(), NAME_MAX);
        // two bytes each: 255 would split the last one.
        let accented = "\u{e9}".repeat(NAME_MAX);
        assert_eq!(slot_name(&accented).len(), NAME_MAX - 1);
        let decoded = decode_slot(&encode_slot(&slot_named(&accented))).unwrap();
        assert_eq!(decoded.name, "\u{e9}".repeat(NAME_MAX / 2));
        assert_eq!(decoded.route, [1]);
    }

    #[test]
    fn names_are_listed_most_common_first_then_by_name() {
        let given = ["b.bin", "a.bin", "", "c.bin", "b.bin", "a.bin", "b.bin"];
        assert_eq!(common_names(given), ["b.bin", "a.bin", "c.bin"]);
        assert_eq!(common_names(["z", "a"]), ["a", "z"]);
        assert_eq!(common_names(["", ""]), [""; 0]);
    }

    #[test]
    fn a_name_is_taken_from_the_most_trusted_source_that_has_one() {
        let slots = vec!["common.bin".to_owned(), "rare.bin".to_owned()];
        assert_eq!(chosen_name(Some("asked"), "linked", &slots), "asked");
        assert_eq!(chosen_name(None, "linked", &slots), "linked");
        assert_eq!(chosen_name(Some(""), "", &slots), "common.bin");
        assert_eq!(chosen_name(None, "", &[]), "");
    }

    #[test]
    fn a_file_is_saved_under_the_last_part_of_its_name() {
        assert_eq!(save_name("a.bin"), "a.bin");
        assert_eq!(save_name("../../etc/passwd"), "passwd");
        assert_eq!(save_name(r"c:\windows\evil.exe"), "evil.exe");
        for nameless in ["", "dir/", "..", ".", "a/.."] {
            assert_eq!(save_name(nameless), FALLBACK_NAME, "{nameless:?}");
        }
    }

    #[test]
    fn a_slot_goes_stale_in_either_direction() {
        let slot = slot_named("");
        assert!(!slot.is_stale(slot.time));
        assert!(!slot.is_stale(slot.time + SLOT_STALE_SECONDS));
        assert!(slot.is_stale(slot.time + SLOT_STALE_SECONDS + 1));
        assert!(!slot.is_stale(slot.time - SLOT_STALE_SECONDS));
        assert!(slot.is_stale(slot.time - SLOT_STALE_SECONDS - 1));
    }

    #[test]
    fn only_an_error_reply_is_an_error_reply() {
        assert!(is_error_reply(&encode_error_reply("no")));
        assert!(!is_error_reply(&encode_ok_reply()));
        assert!(!is_error_reply(&[]));
    }

    #[test]
    fn piece_layout_matches_vectors() {
        for case in vectors()["pieceCounts"].as_array().unwrap() {
            assert_eq!(
                piece_count(case["size"].as_u64().unwrap()),
                case["count"].as_u64().unwrap()
            );
        }
        assert_eq!(
            piece_range(1, PIECE_BYTES + 1),
            (PIECE_BYTES, PIECE_BYTES + 1)
        );
        assert_eq!(piece_range(0, 0), (0, 0));
    }

    // the token and the name a vector case expects of a link to a file.
    fn file_link(case: &serde_json::Value) -> Link {
        let text = |field: &str| case[field].as_str().unwrap().to_owned();
        Link::File(text("token"), text("name"))
    }

    #[test]
    fn links_match_vectors() {
        for case in vectors()["links"].as_array().unwrap() {
            assert_eq!(parse_link(case["input"].as_str().unwrap()), file_link(case));
        }
        let case = &vectors()["linkFor"];
        let text = |field: &str| case[field].as_str().unwrap();
        let link = link_for(text("base"), text("token"), text("name"));
        assert_eq!(link, text("link"));
        assert_eq!(parse_link(&link), file_link(case));
        // with no name to give, a link is the token alone.
        let bare = link_for(text("base"), text("token"), "");
        assert_eq!(bare, text("bare"));
        assert_eq!(
            parse_link(&bare),
            Link::File(text("token").to_owned(), String::new())
        );
    }

    #[test]
    fn library_links_match_vectors() {
        let token_of = |value: &serde_json::Value| value.as_str().unwrap().to_owned();
        for case in vectors()["libraryLinks"].as_array().unwrap() {
            assert_eq!(
                parse_link(case["input"].as_str().unwrap()),
                Link::Library(token_of(&case["token"]))
            );
        }
        let case = &vectors()["libraryLinkFor"];
        let link = library_link_for(
            case["base"].as_str().unwrap(),
            case["token"].as_str().unwrap(),
        );
        assert_eq!(link, case["link"]);
        assert_eq!(parse_link(&link), Link::Library(token_of(&case["token"])));
    }

    #[test]
    fn a_link_to_a_file_is_not_taken_for_a_library() {
        for case in vectors()["links"].as_array().unwrap() {
            let parsed = parse_link(case["input"].as_str().unwrap());
            assert!(matches!(parsed, Link::File(..)), "{parsed:?}");
        }
        // a file may even be called what a library link starts with.
        let named = link_for("https://x/", "VLD0:a:b", "library/x");
        assert_eq!(
            parse_link(&named),
            Link::File("VLD0:a:b".into(), "library/x".into())
        );
    }

    #[test]
    fn a_file_without_a_folder_is_in_none() {
        let json = r#"{"name":"a","size":1,"root":"00"}"#;
        let file: FileEntry = serde_json::from_str(json).unwrap();
        assert_eq!(file.folder, "");
    }

    #[test]
    fn hashes_round_trip_through_hex() {
        let hash = [0xab; HASH_BYTES];
        assert_eq!(hash_from_hex(&to_hex(&hash)).unwrap(), hash);
        assert!(hash_from_hex("abcd").is_err());
    }
}
