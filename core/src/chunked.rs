//! a download kept in chunks: runs of pieces that are each written once,
//! whole, when their last piece is in. a browser's file system rewrites a
//! file on every write to it, so writing a piece at a time there would copy
//! the download again and again; a chunk is written in one go instead, and
//! its pieces wait in memory until then. pieces arrive nearly in order, so
//! few chunks are ever waiting.

use crate::protocol::{piece_count, piece_range, PIECE_BYTES};
use crate::store::{BoxFuture, Chunks, Sink};
use crate::Result;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// about 7.5 MiB a chunk: few enough files for a multi-gigabyte download,
/// small enough that the chunks being filled fit in memory.
pub const CHUNK_PIECES: u64 = 256;
const NOT_WRITTEN: &str = "that piece has not been written";

pub struct ChunkedSink<C> {
    chunks: C,
    size: u64,
    // the pieces of chunks not yet whole, by chunk and then by piece.
    waiting: Mutex<BTreeMap<u64, BTreeMap<u64, Vec<u8>>>>,
}

/// how many pieces chunk `chunk` of a file of `size` bytes has.
fn pieces_in(chunk: u64, size: u64) -> u64 {
    let left = piece_count(size).saturating_sub(chunk * CHUNK_PIECES);
    left.min(CHUNK_PIECES)
}

impl<C: Chunks> ChunkedSink<C> {
    /// a sink for a file of `size` bytes that keeps its chunks in `chunks`.
    pub fn new(chunks: C, size: u64) -> ChunkedSink<C> {
        ChunkedSink {
            chunks,
            size,
            waiting: Mutex::default(),
        }
    }

    // keep a piece, and take its chunk out, pieces joined in order, if that
    // made it whole.
    fn take_whole(&self, index: u64, piece: Vec<u8>) -> Option<(u64, Vec<u8>)> {
        let chunk = index / CHUNK_PIECES;
        let mut waiting = self.waiting.lock().unwrap();
        let pieces = waiting.entry(chunk).or_default();
        pieces.insert(index, piece);
        if (pieces.len() as u64) < pieces_in(chunk, self.size) {
            return None;
        }
        let pieces = waiting.remove(&chunk)?;
        Some((chunk, pieces.into_values().flatten().collect()))
    }

    fn waiting_piece(&self, index: u64) -> Option<Vec<u8>> {
        let waiting = self.waiting.lock().unwrap();
        waiting.get(&(index / CHUNK_PIECES))?.get(&index).cloned()
    }
}

impl<C: Chunks> Sink for ChunkedSink<C> {
    fn write(&self, index: u64, piece: Vec<u8>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            match self.take_whole(index, piece) {
                Some((chunk, bytes)) => self.chunks.write(chunk, bytes).await,
                None => Ok(()),
            }
        })
    }

    fn read(&self, index: u64) -> BoxFuture<'_, Result<Vec<u8>>> {
        Box::pin(async move {
            if let Some(piece) = self.waiting_piece(index) {
                return Ok(piece);
            }
            let (start, end) = piece_range(index, self.size);
            let chunk_start = index / CHUNK_PIECES * CHUNK_PIECES * PIECE_BYTES;
            let (start, end) = (start - chunk_start, end - chunk_start);
            let piece = self.chunks.read(index / CHUNK_PIECES, start, end).await?;
            if piece.len() as u64 != end - start {
                return Err(NOT_WRITTEN.into());
            }
            Ok(piece)
        })
    }

    fn finish(&self) -> BoxFuture<'_, Result<String>> {
        Box::pin(async move {
            if !self.waiting.lock().unwrap().is_empty() {
                return Err("the download is incomplete".into());
            }
            self.chunks.finish().await
        })
    }

    fn discard(&self) -> BoxFuture<'_, ()> {
        self.waiting.lock().unwrap().clear();
        self.chunks.discard()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::sync::Arc;

    // a chunk's bytes, and how often it was written.
    type Written = BTreeMap<u64, (Vec<u8>, u32)>;

    /// chunks kept in memory.
    #[derive(Default, Clone)]
    struct Memory(Arc<Mutex<Written>>);

    impl Chunks for Memory {
        fn write(&self, chunk: u64, bytes: Vec<u8>) -> BoxFuture<'_, Result<()>> {
            let mut chunks = self.0.lock().unwrap();
            let writes = chunks.get(&chunk).map_or(0, |held| held.1);
            chunks.insert(chunk, (bytes, writes + 1));
            Box::pin(async { Ok(()) })
        }

        fn read(&self, chunk: u64, start: u64, end: u64) -> BoxFuture<'_, Result<Vec<u8>>> {
            let chunks = self.0.lock().unwrap();
            let held = chunks.get(&chunk).map(|held| held.0.clone());
            Box::pin(async move {
                let held = held.ok_or(NOT_WRITTEN)?;
                Ok(held[start as usize..(end as usize).min(held.len())].to_vec())
            })
        }

        fn finish(&self) -> BoxFuture<'_, Result<String>> {
            Box::pin(async { Ok("memory".to_owned()) })
        }

        fn discard(&self) -> BoxFuture<'_, ()> {
            self.0.lock().unwrap().clear();
            Box::pin(async {})
        }
    }

    fn piece(index: u64, size: u64) -> Vec<u8> {
        let (start, end) = piece_range(index, size);
        vec![index as u8; (end - start) as usize]
    }

    fn now<T>(future: impl std::future::Future<Output = T>) -> T {
        future.now_or_never().expect("memory chunks never wait")
    }

    // one whole chunk, one piece into a second, and a short last piece.
    const SIZE: u64 = (CHUNK_PIECES + 1) * PIECE_BYTES + 5;

    #[test]
    fn a_chunk_is_written_once_when_its_last_piece_is_in() {
        let memory = Memory::default();
        let sink = ChunkedSink::new(memory.clone(), SIZE);
        // out of order, as pieces arrive.
        for index in (0..CHUNK_PIECES).rev() {
            assert!(memory.0.lock().unwrap().is_empty());
            now(sink.write(index, piece(index, SIZE))).unwrap();
        }
        let chunks = memory.0.lock().unwrap();
        let (bytes, writes) = &chunks[&0];
        assert_eq!(
            (bytes.len() as u64, *writes),
            (CHUNK_PIECES * PIECE_BYTES, 1)
        );
        assert_eq!(bytes[PIECE_BYTES as usize], 1);
    }

    #[test]
    fn a_piece_is_read_back_before_and_after_its_chunk_is_written() {
        let sink = ChunkedSink::new(Memory::default(), SIZE);
        now(sink.write(3, piece(3, SIZE))).unwrap();
        assert_eq!(now(sink.read(3)).unwrap(), piece(3, SIZE));
        assert!(now(sink.read(4)).is_err());
        for index in 0..piece_count(SIZE) {
            now(sink.write(index, piece(index, SIZE))).unwrap();
        }
        let last = piece_count(SIZE) - 1;
        for index in [0, 3, CHUNK_PIECES - 1, CHUNK_PIECES, last] {
            assert_eq!(now(sink.read(index)).unwrap(), piece(index, SIZE));
        }
        assert_eq!(now(sink.read(last)).unwrap().len(), 5);
    }

    #[test]
    fn the_short_last_chunk_is_whole_with_the_pieces_it_has() {
        let memory = Memory::default();
        let sink = ChunkedSink::new(memory.clone(), SIZE);
        assert_eq!(pieces_in(0, SIZE), CHUNK_PIECES);
        assert_eq!(pieces_in(1, SIZE), 2);
        assert_eq!(pieces_in(0, 0), 1);
        for index in CHUNK_PIECES..piece_count(SIZE) {
            now(sink.write(index, piece(index, SIZE))).unwrap();
        }
        let written = memory.0.lock().unwrap()[&1].0.len() as u64;
        assert_eq!(written, PIECE_BYTES + 5);
    }

    #[test]
    fn a_download_with_pieces_missing_does_not_finish() {
        let sink = ChunkedSink::new(Memory::default(), SIZE);
        now(sink.write(0, piece(0, SIZE))).unwrap();
        assert!(now(sink.finish()).is_err());
        let empty = ChunkedSink::new(Memory::default(), 0);
        now(empty.write(0, Vec::new())).unwrap();
        assert_eq!(now(empty.finish()).unwrap(), "memory");
    }

    #[test]
    fn a_discarded_download_keeps_nothing() {
        let memory = Memory::default();
        let sink = ChunkedSink::new(memory.clone(), SIZE);
        for index in 0..=CHUNK_PIECES {
            now(sink.write(index, piece(index, SIZE))).unwrap();
        }
        now(sink.discard());
        assert!(memory.0.lock().unwrap().is_empty());
        assert!(now(sink.read(CHUNK_PIECES)).is_err());
    }
}
