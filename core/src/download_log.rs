//! what a peer says about its downloaders (DESIGN.md "download notices").
//! downloaders are anonymous, so their own notices are all a peer has to go
//! on. this turns notices and the ticks of a timer into log lines, and holds
//! no clock and no logger of its own: the wording is pinned by the
//! `downloadLog` vector, which the app's twin of this is tested against too,
//! so that every interface says the same thing in the same words.

use std::collections::BTreeMap;
use std::time::Duration;

/// how often progress is reported, by fetchers and in a peer's log.
pub const PROGRESS_INTERVAL: Duration = Duration::from_secs(10);
/// a download not heard from for this long has gone away.
pub const DOWNLOAD_GONE: Duration = Duration::from_secs(60);
/// how many downloads of one file that have finished or gone away are kept
/// to be listed. those under way are all kept.
pub const LISTED_MAX: usize = 64;
const PERCENT: u64 = 100;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Level {
    Debug,
    Info,
    Warn,
}

#[derive(Debug, PartialEq)]
pub struct Line {
    pub level: Level,
    pub message: String,
}

fn info(message: String) -> Vec<Line> {
    vec![Line {
        level: Level::Info,
        message,
    }]
}

/// how a download stands, as far as its notices say.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Standing {
    Downloading,
    Finished,
    /// not heard from for `DOWNLOAD_GONE`.
    Gone,
}

/// the file a notice is about: what tells it from every other file the
/// peer serves, what it is called, and how many pieces it has.
#[derive(Clone, Copy)]
pub struct Of<'a> {
    pub file: &'a str,
    pub name: &'a str,
    pub total: u64,
    /// how many bytes it has.
    pub size: u64,
}

struct Download {
    file: String,
    name: String,
    total: u64,
    size: u64,
    // when the first and the latest notice of it arrived, in seconds.
    first: u64,
    heard: u64,
    done: u64,
    // the count the log last gave. it starts at zero, so that a download
    // which has not got anywhere yet is not said to be at 0%.
    logged: u64,
    standing: Standing,
}

impl Download {
    fn fresh(now: u64, of: Of) -> Download {
        Download {
            file: of.file.to_owned(),
            name: of.name.to_owned(),
            total: of.total,
            size: of.size,
            first: now,
            heard: now,
            done: 0,
            logged: 0,
            standing: Standing::Downloading,
        }
    }

    fn percent(&self) -> u64 {
        self.done * PERCENT / self.total.max(1)
    }

    fn active(&self) -> bool {
        self.standing == Standing::Downloading
    }

    fn seen(&self, id: &str) -> Seen {
        Seen {
            id: id.to_owned(),
            done: self.done,
            total: self.total,
            standing: self.standing,
            started: self.first,
            seen: self.heard,
        }
    }
}

/// a download, and the file it is of.
#[derive(Debug, PartialEq, Clone)]
pub struct SeenFile {
    pub seen: Seen,
    pub name: String,
    pub size: u64,
}

/// a download as whoever asks after a file's downloaders is shown it.
#[derive(Debug, PartialEq, Clone)]
pub struct Seen {
    pub id: String,
    pub done: u64,
    pub total: u64,
    pub standing: Standing,
    /// when its first and its latest notice arrived, in seconds.
    pub started: u64,
    pub seen: u64,
}

/// the downloads a peer has heard of, by hex id: those under way, which
/// are what the log speaks of, and those that have finished or gone away,
/// kept to be listed for as long as their file is served. ordered, so that
/// one tick always reports them in the same order.
#[derive(Default)]
pub struct DownloadLog {
    downloads: BTreeMap<String, Download>,
}

impl DownloadLog {
    // the download a notice is about, heard from just now. one whose
    // "started" was lost begins at its first notice of any kind, and so
    // does one heard from again after it was given up for gone. a late
    // notice of one that has finished changes nothing.
    fn hear(&mut self, now: u64, id: &str, of: Of) -> &mut Download {
        let held = self.downloads.entry(id.to_owned());
        let download = held.or_insert_with(|| Download::fresh(now, of));
        match download.standing {
            Standing::Downloading => download.heard = now,
            Standing::Gone => *download = Download::fresh(now, of),
            Standing::Finished => {}
        }
        download
    }

    fn active(&self) -> usize {
        let active = |download: &&Download| download.active();
        self.downloads.values().filter(active).count()
    }

    // let go of the oldest ended downloads of a file past LISTED_MAX.
    fn prune(&mut self, file: &str) {
        let mut ended: Vec<(u64, String)> = self
            .downloads
            .iter()
            .filter(|(_, download)| download.file == file && !download.active())
            .map(|(id, download)| (download.heard, id.clone()))
            .collect();
        ended.sort();
        let over = ended.len().saturating_sub(LISTED_MAX);
        for (_, id) in ended.into_iter().take(over) {
            self.downloads.remove(&id);
        }
    }

    /// a fetcher says its download of a file has begun.
    pub fn started(&mut self, now: u64, id: &str, of: Of) -> Vec<Line> {
        self.hear(now, id, of);
        let active = self.active();
        info(format!("download {id} started ({active} active)"))
    }

    /// a fetcher says how many pieces it has verified. said in the log at
    /// the next tick, if it is news by then.
    pub fn progress(&mut self, now: u64, id: &str, of: Of, done: u64) -> Vec<Line> {
        let download = self.hear(now, id, of);
        if download.active() {
            download.done = done;
        }
        Vec::new()
    }

    /// a fetcher says its download is complete. `served` is how many pieces
    /// this peer has served, to anyone, so far.
    pub fn finished(&mut self, now: u64, id: &str, of: Of, served: u64) -> Vec<Line> {
        let known = self.downloads.get(id).filter(|download| download.active());
        let took = known.map(|download| format!(" in {}s", now - download.first));
        let took = took.unwrap_or_default();
        let download = self.hear(now, id, of);
        (download.done, download.heard) = (download.total, now);
        download.standing = Standing::Finished;
        self.prune(of.file);
        info(format!(
            "download {id} finished{took} ({served} pieces served in total)"
        ))
    }

    /// the progress timer has fired: say how far each download has got, if
    /// that has changed since it was last said, and give up those that
    /// have fallen silent for gone.
    pub fn tick(&mut self, now: u64) -> Vec<Line> {
        let (mut lines, mut gone) = (Vec::new(), Vec::new());
        for (id, download) in self.downloads.iter_mut().filter(|(_, held)| held.active()) {
            let (name, percent) = (&download.name, download.percent());
            if now - download.heard >= DOWNLOAD_GONE.as_secs() {
                lines.push(Line {
                    level: Level::Warn,
                    message: format!("download {id} of {name} went away at {percent}%"),
                });
                download.standing = Standing::Gone;
                gone.push(download.file.clone());
            } else if download.done != download.logged {
                let (done, total) = (download.done, download.total);
                lines.extend(info(format!(
                    "download {id} of {name} at {percent}% ({done}/{total} pieces)"
                )));
                download.logged = done;
            }
        }
        gone.iter().for_each(|file| self.prune(file));
        lines
    }

    /// the downloads of one file, oldest first: those under way, and the
    /// last LISTED_MAX that have finished or gone away.
    pub fn of_file(&self, file: &str) -> Vec<Seen> {
        let of_file = |(_, download): &(&String, &Download)| download.file == file;
        let held = self.downloads.iter().filter(of_file);
        let mut seen: Vec<Seen> = held.map(|(id, download)| download.seen(id)).collect();
        seen.sort_by_key(|download| download.started);
        seen
    }

    /// every download listed, of whichever file, each with what its file
    /// is called and how big it is: by id.
    pub fn all(&self) -> Vec<SeenFile> {
        let listed = |(id, download): (&String, &Download)| SeenFile {
            seen: download.seen(id),
            name: download.name.clone(),
            size: download.size,
        };
        self.downloads.iter().map(listed).collect()
    }

    /// a file is no longer served: its downloads are nobody's to list.
    pub fn forget(&mut self, file: &str) {
        self.downloads.retain(|_, download| download.file != file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectors;
    use serde_json::Value;

    fn expected(step: &Value) -> Vec<Line> {
        let line = |logged: &Value| Line {
            level: match logged["level"].as_str().unwrap() {
                "info" => Level::Info,
                _ => Level::Warn,
            },
            message: logged["message"].as_str().unwrap().to_owned(),
        };
        step["logs"].as_array().unwrap().iter().map(line).collect()
    }

    // a file that is told from the others by its name, as the vector's are.
    fn of(name: &str, total: u64) -> Of<'_> {
        Of {
            file: name,
            name,
            total,
            size: total * PIECE,
        }
    }

    // the vector does not say how big its files are: this much a piece.
    const PIECE: u64 = 1000;

    #[test]
    fn every_download_is_listed_with_its_files_name_and_size() {
        let mut log = DownloadLog::default();
        log.started(5, "bb", of("b.bin", 3));
        log.started(7, "aa", of("a.bin", 4));
        log.progress(9, "aa", of("a.bin", 4), 2);
        log.finished(12, "bb", of("b.bin", 3), 3);
        let listed: Vec<(String, String, u64, Standing, u64)> = log
            .all()
            .into_iter()
            .map(|file| {
                (
                    file.seen.id,
                    file.name,
                    file.size,
                    file.seen.standing,
                    file.seen.done,
                )
            })
            .collect();
        let expected = [
            (
                "aa".into(),
                "a.bin".into(),
                4 * PIECE,
                Standing::Downloading,
                2,
            ),
            (
                "bb".into(),
                "b.bin".into(),
                3 * PIECE,
                Standing::Finished,
                3,
            ),
        ];
        assert_eq!(listed, expected);
        // ended ones stay listed until their file is no longer served.
        log.forget("b.bin");
        assert_eq!(log.all().len(), 1);
        log.forget("a.bin");
        assert!(log.all().is_empty());
    }

    fn replay(log: &mut DownloadLog, step: &Value) -> Vec<Line> {
        let number = |field: &str| step[field].as_u64().unwrap();
        let text = |field: &str| step[field].as_str().unwrap();
        let now = number("at");
        let file = || of(text("name"), number("total"));
        match step["notice"].as_str() {
            None => log.tick(now),
            Some("started") => log.started(now, text("id"), file()),
            Some("progress") => log.progress(now, text("id"), file(), number("done")),
            Some(_) => log.finished(now, text("id"), file(), number("served")),
        }
    }

    fn seen(id: &str, done: u64, total: u64, standing: Standing, times: (u64, u64)) -> Seen {
        Seen {
            id: id.to_owned(),
            done,
            total,
            standing,
            started: times.0,
            seen: times.1,
        }
    }

    #[test]
    fn the_downloads_of_a_file_are_listed_from_what_the_log_heard() {
        let (first, second, late) = ("0102030405060708", "a1b2c3d4e5f60718", "ffffffffffffffff");
        let steps = vectors()["downloadLog"]["steps"].clone();
        let steps = steps.as_array().unwrap();
        let (mut log, mut played) = (DownloadLog::default(), 0);
        // what is listed for a file once the first `upto` steps have happened.
        let mut listed = |upto: usize, file: &str| {
            for step in &steps[played..upto] {
                replay(&mut log, step);
            }
            played = upto;
            log.of_file(file)
        };
        // under way, with the pieces last reported and when it was heard.
        let under_way = seen(first, 3, 10, Standing::Downloading, (0, 4));
        assert_eq!(listed(3, "a.bin"), [under_way]);
        // one whose start was lost is listed from its first notice, and
        // stays listed, whole, once it has finished.
        let finished = seen(second, 3, 3, Standing::Finished, (21, 31));
        assert_eq!(listed(8, "b.bin"), std::slice::from_ref(&finished));
        // one that fell silent is listed as gone, where it had got to.
        let gone = seen(first, 3, 10, Standing::Gone, (0, 14));
        assert_eq!(listed(10, "a.bin"), [gone]);
        // heard from again it is under way anew; one that only ever said it
        // had finished is listed as finished. oldest first.
        let again = seen(first, 0, 10, Standing::Downloading, (75, 75));
        let only_end = seen(late, 10, 10, Standing::Finished, (76, 76));
        assert_eq!(listed(12, "a.bin"), [again, only_end]);
        assert_eq!(log.of_file("b.bin"), [finished]);
        assert_eq!(log.of_file("c.bin"), []);
        // a file no longer served has no downloads to list.
        log.forget("a.bin");
        assert_eq!(log.of_file("a.bin"), []);
        assert_eq!(log.of_file("b.bin").len(), 1);
    }

    #[test]
    fn only_the_last_ended_downloads_of_a_file_are_kept() {
        let mut log = DownloadLog::default();
        let file = of("a.bin", 4);
        log.started(0, "live", file);
        for n in 0..LISTED_MAX as u64 + 5 {
            log.finished(n, &format!("{n:04}"), file, 0);
            log.finished(n, &format!("other{n:04}"), of("b.bin", 1), 0);
        }
        let listed = log.of_file("a.bin");
        assert_eq!(listed.len(), LISTED_MAX + 1);
        assert_eq!(listed[0].id, "live");
        assert_eq!(listed[1].id, "0005");
        assert_eq!(log.of_file("b.bin").len(), LISTED_MAX);
    }

    #[test]
    fn the_log_matches_the_vector_step_by_step() {
        let case = &vectors()["downloadLog"];
        assert_eq!(case["progressSeconds"], PROGRESS_INTERVAL.as_secs());
        assert_eq!(case["goneSeconds"], DOWNLOAD_GONE.as_secs());
        let mut log = DownloadLog::default();
        for (index, step) in case["steps"].as_array().unwrap().iter().enumerate() {
            assert_eq!(replay(&mut log, step), expected(step), "step {index}");
        }
    }

    #[test]
    fn a_download_that_has_not_moved_is_not_mentioned_again() {
        let mut log = DownloadLog::default();
        log.started(0, "aa", of("a.bin", 4));
        assert_eq!(log.tick(10), []);
        log.progress(11, "aa", of("a.bin", 4), 2);
        assert_eq!(log.tick(20).len(), 1);
        log.progress(21, "aa", of("a.bin", 4), 2);
        assert_eq!(log.tick(30), []);
    }

    #[test]
    fn a_download_gone_away_no_longer_counts_as_active() {
        let mut log = DownloadLog::default();
        log.started(0, "aa", of("a.bin", 4));
        let both = log.started(1, "bb", of("a.bin", 4));
        assert!(both[0].message.ends_with("(2 active)"));
        log.progress(50, "bb", of("a.bin", 4), 1);
        let gone = log.tick(60);
        assert_eq!(gone[0].level, Level::Warn);
        assert_eq!(gone[0].message, "download aa of a.bin went away at 0%");
        // it is said once: a later tick has nothing more to say of it.
        assert_eq!(log.tick(61), []);
        let again = log.started(61, "cc", of("a.bin", 4));
        assert!(again[0].message.ends_with("(2 active)"));
    }

    #[test]
    fn progress_before_a_start_counts_from_when_it_was_first_heard() {
        let mut log = DownloadLog::default();
        assert_eq!(log.progress(5, "aa", of("a.bin", 4), 1), []);
        let started = log.started(6, "aa", of("a.bin", 4));
        assert!(started[0].message.ends_with("(1 active)"));
        let finished = log.finished(25, "aa", of("a.bin", 4), 4);
        assert_eq!(
            finished[0].message,
            "download aa finished in 20s (4 pieces served in total)"
        );
        assert_eq!(log.tick(100), []);
    }
}
