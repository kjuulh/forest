use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use forest_grpc_interface::DownloadBinaryResponse;
use futures::{Stream, StreamExt, TryStreamExt};
use tokio::sync::Semaphore;

use super::{StreamedBinary, grpc_err};

pub(crate) const RANGE_SIZE: u64 = 8 * 1024 * 1024;

pub(crate) const RANGES_PER_BINARY: usize = 4;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Tuning {
    pub range_size: u64,
    pub ranges_per_binary: usize,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            range_size: RANGE_SIZE,
            ranges_per_binary: RANGES_PER_BINARY,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RangeRequest {
    pub offset: u64,
    pub length: u64,
    pub expected_sha256: Option<String>,
}

pub(crate) async fn download_to_file<O, Fut, S>(
    open: O,
    streams: &Arc<Semaphore>,
    tuning: Tuning,
    dest_dir: &Path,
    expected_sha256: Option<&str>,
    expected_size: Option<u64>,
    progress_label: Option<&str>,
) -> anyhow::Result<StreamedBinary>
where
    O: Fn(RangeRequest) -> Fut,
    Fut: Future<Output = anyhow::Result<S>>,
    S: Stream<Item = Result<DownloadBinaryResponse, tonic::Status>> + Unpin,
{
    tokio::fs::create_dir_all(dest_dir)
        .await
        .with_context(|| format!("creating download dir {}", dest_dir.display()))?;
    let rand: u64 = rand::random();
    let incoming = Incoming(Some(dest_dir.join(format!(".incoming.{rand:016x}"))));
    let temp_path = incoming.path().to_path_buf();
    let file = tokio::fs::File::create(&temp_path)
        .await
        .with_context(|| format!("creating tempfile {}", temp_path.display()))?;
    let file = Arc::new(file.into_std().await);

    let mut progress = Progress::new(progress_label, expected_size);
    let permit = Arc::clone(streams).acquire_owned().await?;
    let mut first = open(RangeRequest {
        offset: 0,
        length: tuning.range_size,
        expected_sha256: expected_sha256.map(|s| bare_hex(s).to_string()),
    })
    .await?;
    let head = first.next().await.transpose().map_err(grpc_err)?;

    let pinned = match head {
        None => None,
        Some(head) if head.sha256.is_empty() => {
            drain(first, Some(head), Span::whole(), &file, &progress).await?;
            drop(permit);
            None
        }
        Some(head) => {
            let pinned = Pinned {
                sha256: bare_hex(&head.sha256).to_ascii_lowercase(),
                size: head.total_size,
            };
            progress.learn_total(pinned.size);
            let f = Arc::clone(&file);
            let size = pinned.size;
            tokio::task::spawn_blocking(move || f.set_len(size))
                .await?
                .with_context(|| format!("preallocating {}", temp_path.display()))?;

            let mut spans = Span::split(&pinned, tuning.range_size).into_iter();
            let head_span = spans.next().context("empty range plan")?;
            let jobs = std::iter::once(Job::Head {
                stream: first,
                head,
                span: head_span,
                permit,
            })
            .chain(spans.map(Job::Range));
            futures::stream::iter(jobs)
                .map(|job| {
                    let open = &open;
                    let file = &file;
                    let progress = &progress;
                    async move {
                        match job {
                            Job::Head {
                                stream,
                                head,
                                span,
                                permit,
                            } => {
                                let _permit = permit;
                                drain(stream, Some(head), span, file, progress).await
                            }
                            Job::Range(span) => {
                                let _permit = Arc::clone(streams).acquire_owned().await?;
                                let stream = open(RangeRequest {
                                    offset: span.start,
                                    length: span.end - span.start,
                                    expected_sha256: Some(span.pinned_sha().to_string()),
                                })
                                .await?;
                                drain(stream, None, span, file, progress).await
                            }
                        }
                    }
                })
                .buffer_unordered(tuning.ranges_per_binary.max(1))
                .try_collect::<()>()
                .await?;
            Some(pinned)
        }
    };
    progress.clear();

    let (sha256_hex, size_bytes) =
        tokio::task::spawn_blocking(move || seal(&file, &temp_path)).await??;
    if let Some(pinned) = pinned {
        anyhow::ensure!(
            size_bytes == pinned.size && sha256_hex == pinned.sha256,
            "assembled binary does not match the registry's: expected sha256 {} ({} bytes), \
             got {sha256_hex} ({size_bytes} bytes)",
            pinned.sha256,
            pinned.size,
        );
    }

    Ok(StreamedBinary {
        temp_path: incoming.keep(),
        sha256_hex,
        size_bytes,
    })
}

fn seal(file: &std::fs::File, path: &Path) -> anyhow::Result<(String, u64)> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    file.sync_all()
        .with_context(|| format!("fsyncing {}", path.display()))?;
    let mut reader =
        std::fs::File::open(path).with_context(|| format!("reopening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("hashing {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

enum Job<S> {
    Head {
        stream: S,
        head: DownloadBinaryResponse,
        span: Span,
        permit: tokio::sync::OwnedSemaphorePermit,
    },
    Range(Span),
}

#[derive(Clone)]
struct Pinned {
    sha256: String,
    size: u64,
}

struct Span {
    start: u64,
    end: u64,
    pinned: Option<Pinned>,
}

impl Span {
    fn whole() -> Self {
        Self {
            start: 0,
            end: u64::MAX,
            pinned: None,
        }
    }

    fn split(pinned: &Pinned, range_size: u64) -> Vec<Self> {
        let step = range_size.max(1);
        let mut spans = Vec::new();
        let mut start = 0u64;
        loop {
            let end = start.saturating_add(step).min(pinned.size);
            spans.push(Self {
                start,
                end,
                pinned: Some(pinned.clone()),
            });
            if end >= pinned.size {
                return spans;
            }
            start = end;
        }
    }

    fn pinned_sha(&self) -> &str {
        self.pinned.as_ref().map_or("", |p| p.sha256.as_str())
    }
}

async fn drain<S>(
    mut stream: S,
    head: Option<DownloadBinaryResponse>,
    span: Span,
    file: &Arc<std::fs::File>,
    progress: &Progress,
) -> anyhow::Result<()>
where
    S: Stream<Item = Result<DownloadBinaryResponse, tonic::Status>> + Unpin,
{
    let mut cursor = span.start;
    let mut message = head;
    let mut first = true;
    loop {
        let msg = match message.take() {
            Some(m) => m,
            None => match stream.next().await.transpose().map_err(grpc_err)? {
                Some(m) => m,
                None => break,
            },
        };

        if let Some(pinned) = &span.pinned {
            if first && span.start > 0 && msg.sha256.is_empty() {
                anyhow::bail!(
                    "registry ignored the requested range at offset {}",
                    span.start
                );
            }
            if !msg.sha256.is_empty()
                && (!bare_hex(&msg.sha256).eq_ignore_ascii_case(&pinned.sha256)
                    || msg.total_size != pinned.size)
            {
                anyhow::bail!(
                    "registry served a different binary mid-download: sha256 {} ({} bytes), \
                     expected {} ({} bytes)",
                    msg.sha256,
                    msg.total_size,
                    pinned.sha256,
                    pinned.size,
                );
            }
            if msg.offset != cursor {
                anyhow::bail!(
                    "registry sent bytes at offset {} where {cursor} was next",
                    msg.offset
                );
            }
        }
        first = false;

        let len = msg.chunk.len() as u64;
        if len == 0 {
            continue;
        }
        if cursor.checked_add(len).is_none_or(|end| end > span.end) {
            anyhow::bail!(
                "registry sent more than the requested range {}..{}",
                span.start,
                span.end
            );
        }
        let f = Arc::clone(file);
        let at = cursor;
        tokio::task::spawn_blocking(move || write_at(&f, &msg.chunk, at))
            .await?
            .with_context(|| format!("writing {len} bytes at offset {at}"))?;
        cursor += len;
        progress.inc(len);
    }

    if span.pinned.is_some() && cursor != span.end {
        anyhow::bail!(
            "registry ended range {}..{} after {} bytes",
            span.start,
            span.end,
            cursor - span.start
        );
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &std::fs::File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

#[cfg(windows)]
fn write_at(file: &std::fs::File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        let n = std::os::windows::fs::FileExt::seek_write(file, buf, offset)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

fn bare_hex(sha: &str) -> &str {
    sha.strip_prefix("sha256:").unwrap_or(sha)
}

struct Incoming(Option<PathBuf>);

impl Incoming {
    fn path(&self) -> &Path {
        self.0.as_deref().expect("path is present until kept")
    }

    fn keep(mut self) -> PathBuf {
        self.0.take().expect("path is present until kept")
    }
}

impl Drop for Incoming {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            std::fs::remove_file(path).ok();
        }
    }
}

struct Progress {
    label: Option<String>,
    bar: Option<crate::ui::Bar>,
}

impl Progress {
    fn new(label: Option<&str>, total: Option<u64>) -> Self {
        let bar = match (label, total) {
            (Some(l), Some(t)) if t > 0 => Some(crate::ui::bytes_bar(l, t)),
            _ => None,
        };
        Self {
            label: label.map(str::to_owned),
            bar,
        }
    }

    fn learn_total(&mut self, total: u64) {
        if self.bar.is_none()
            && total > 0
            && let Some(label) = &self.label
        {
            self.bar = Some(crate::ui::bytes_bar(label, total));
        }
    }

    fn inc(&self, delta: u64) {
        if let Some(bar) = &self.bar {
            bar.inc(delta);
        }
    }

    fn clear(&mut self) {
        if let Some(bar) = self.bar.take() {
            bar.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::stream::BoxStream;
    use sha2::{Digest, Sha256};
    use tokio::sync::Notify;

    use super::*;

    const RANGE: u64 = 1000;
    const STREAMS_IN_FLIGHT: usize = 8;
    const CHUNK: usize = 300;

    fn tuning(ranges_per_binary: usize) -> Tuning {
        Tuning {
            range_size: RANGE,
            ranges_per_binary,
        }
    }

    fn bytes(len: usize) -> Vec<u8> {
        (0..len as u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8)
            .collect()
    }

    fn sha(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Behaviour {
        Honest,
        IgnoresRanges,
        FlipsByteAt(u64),
        TruncatesRangeAt(u64),
        RepeatsChunkInRangeAt(u64),
        OverrunsRangeAt(u64),
        RepublishedBeforeRangeAt(u64),
        FailsRangeAt(u64),
        StallsRangeAt(u64),
    }

    struct Fake {
        data: Vec<u8>,
        behaviour: Behaviour,
        in_flight: AtomicUsize,
        peak: AtomicUsize,
        requests: Mutex<Vec<RangeRequest>>,
        finished: Mutex<Vec<u64>>,
        waits: HashMap<u64, u64>,
        done: HashMap<u64, Arc<Notify>>,
    }

    impl Fake {
        fn new(data: Vec<u8>, behaviour: Behaviour) -> Arc<Self> {
            Arc::new(Self {
                data,
                behaviour,
                in_flight: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                finished: Mutex::new(Vec::new()),
                waits: HashMap::new(),
                done: HashMap::new(),
            })
        }

        fn gated(data: Vec<u8>, waits: HashMap<u64, u64>) -> Arc<Self> {
            let done = (0..data.len() as u64)
                .step_by(RANGE as usize)
                .map(|o| (o, Arc::new(Notify::new())))
                .collect();
            Arc::new(Self {
                waits,
                done,
                ..Arc::into_inner(Self::new(data, Behaviour::Honest)).unwrap()
            })
        }

        fn reversed(data: Vec<u8>) -> Arc<Self> {
            let last = data.len() as u64 / RANGE * RANGE;
            let waits = (0..last)
                .step_by(RANGE as usize)
                .map(|o| (o, o + RANGE))
                .collect();
            Self::gated(data, waits)
        }

        fn sha(&self) -> String {
            sha(&self.data)
        }

        fn open(
            self: &Arc<Self>,
            req: RangeRequest,
        ) -> impl Future<
            Output = anyhow::Result<
                BoxStream<'static, Result<DownloadBinaryResponse, tonic::Status>>,
            >,
        > + use<> {
            let fake = Arc::clone(self);
            async move {
                fake.requests.lock().unwrap().push(req.clone());
                let pinned_sha = match fake.behaviour {
                    Behaviour::RepublishedBeforeRangeAt(o) if req.offset >= o => "f".repeat(64),
                    _ => fake.sha(),
                };
                if let Some(want) = &req.expected_sha256
                    && *want != pinned_sha
                {
                    anyhow::bail!("binary changed");
                }
                let n = fake.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                fake.peak.fetch_max(n, Ordering::SeqCst);

                let size = fake.data.len() as u64;
                let (start, end) = if fake.behaviour == Behaviour::IgnoresRanges {
                    (0, size)
                } else {
                    (req.offset, (req.offset + req.length).min(size))
                };
                let mut messages = Vec::new();
                let mut at = start;
                while at < end {
                    let next = (at + CHUNK as u64).min(end);
                    let mut chunk = fake.data[at as usize..next as usize].to_vec();
                    let mut offset = at;
                    match fake.behaviour {
                        Behaviour::FlipsByteAt(o) if (at..next).contains(&o) => {
                            chunk[(o - at) as usize] ^= 0xff;
                        }
                        Behaviour::RepeatsChunkInRangeAt(o) if req.offset == o && at > start => {
                            offset = at - 1;
                        }
                        Behaviour::OverrunsRangeAt(o) if req.offset == o && next == end => {
                            chunk.push(0);
                        }
                        _ => {}
                    }
                    let mut msg = DownloadBinaryResponse {
                        chunk,
                        offset,
                        total_size: 0,
                        sha256: String::new(),
                    };
                    if at == start && fake.behaviour != Behaviour::IgnoresRanges {
                        msg.total_size = size;
                        msg.sha256 = pinned_sha.clone();
                    }
                    if fake.behaviour == Behaviour::IgnoresRanges {
                        msg.offset = 0;
                    }
                    messages.push(Ok(msg));
                    at = next;
                    if matches!(fake.behaviour, Behaviour::TruncatesRangeAt(o) if req.offset == o) {
                        break;
                    }
                }
                if matches!(fake.behaviour, Behaviour::FailsRangeAt(o) if req.offset == o) {
                    messages.truncate(1);
                    messages.push(Err(tonic::Status::unavailable("connection reset")));
                }

                let offset = req.offset;
                let stall = matches!(fake.behaviour, Behaviour::StallsRangeAt(o) if o == offset);
                let wait_for = fake
                    .waits
                    .get(&offset)
                    .and_then(|w| fake.done.get(w).cloned());
                let release = fake.done.get(&offset).cloned();
                let guard = InFlight(Arc::clone(&fake), offset, release);
                let stream =
                    futures::stream::iter(messages.into_iter().enumerate()).then(move |(i, m)| {
                        let wait_for = wait_for.clone();
                        let _keep = &guard;
                        async move {
                            if i == 1 {
                                if let Some(gate) = wait_for {
                                    gate.notified().await;
                                }
                                if stall {
                                    std::future::pending::<()>().await;
                                }
                            }
                            m
                        }
                    });
                Ok(stream.boxed())
            }
        }
    }

    struct InFlight(Arc<Fake>, u64, Option<Arc<Notify>>);

    impl Drop for InFlight {
        fn drop(&mut self) {
            self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.0.finished.lock().unwrap().push(self.1);
            if let Some(n) = &self.2 {
                n.notify_one();
            }
        }
    }

    async fn run(
        fake: &Arc<Fake>,
        streams: &Arc<Semaphore>,
        tuning: Tuning,
        dir: &Path,
        expected: Option<&str>,
    ) -> anyhow::Result<StreamedBinary> {
        download_to_file(|r| fake.open(r), streams, tuning, dir, expected, None, None).await
    }

    fn leftovers(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|d| d.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn ranges_completing_last_first_still_assemble_the_exact_binary() {
        let data = bytes(4 * RANGE as usize - 123);
        let fake = Fake::reversed(data.clone());
        let dir = tempfile::tempdir().unwrap();
        let streams = Arc::new(Semaphore::new(STREAMS_IN_FLIGHT));

        let got = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            run(&fake, &streams, tuning(4), dir.path(), None),
        )
        .await
        .expect("ranges must overlap; serially the reversed gates never open")
        .unwrap();

        assert_eq!(std::fs::read(&got.temp_path).unwrap(), data);
        assert_eq!(got.sha256_hex, sha(&data));
        assert_eq!(got.size_bytes, data.len() as u64);
        assert_eq!(*fake.finished.lock().unwrap(), vec![3000, 2000, 1000, 0]);
        assert_eq!(fake.peak.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn the_first_ranges_slot_is_reused_once_it_finishes() {
        let data = bytes(3 * RANGE as usize);
        let fake = Fake::gated(data.clone(), HashMap::from([(1000, 2000)]));
        let dir = tempfile::tempdir().unwrap();
        let streams = Arc::new(Semaphore::new(STREAMS_IN_FLIGHT));

        let got = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            run(&fake, &streams, tuning(2), dir.path(), None),
        )
        .await
        .expect("range 2000 must start in the slot range 0 frees")
        .unwrap();

        assert_eq!(std::fs::read(&got.temp_path).unwrap(), data);
        assert_eq!(*fake.finished.lock().unwrap(), vec![0, 2000, 1000]);
    }

    #[tokio::test]
    async fn later_ranges_are_pinned_to_the_digest_the_first_reply_named() {
        let data = bytes(3 * RANGE as usize);
        let fake = Fake::new(data.clone(), Behaviour::Honest);
        let dir = tempfile::tempdir().unwrap();
        let streams = Arc::new(Semaphore::new(STREAMS_IN_FLIGHT));

        run(&fake, &streams, tuning(4), dir.path(), None)
            .await
            .unwrap();

        let mut requests = fake.requests.lock().unwrap().clone();
        requests.sort_by_key(|r| r.offset);
        assert_eq!(
            requests,
            vec![
                RangeRequest {
                    offset: 0,
                    length: RANGE,
                    expected_sha256: None
                },
                RangeRequest {
                    offset: 1000,
                    length: RANGE,
                    expected_sha256: Some(fake.sha())
                },
                RangeRequest {
                    offset: 2000,
                    length: RANGE,
                    expected_sha256: Some(fake.sha())
                },
            ]
        );
    }

    #[tokio::test]
    async fn the_callers_expected_digest_rides_on_the_first_request() {
        let data = bytes(2 * RANGE as usize);
        let fake = Fake::new(data, Behaviour::Honest);
        let dir = tempfile::tempdir().unwrap();
        let streams = Arc::new(Semaphore::new(STREAMS_IN_FLIGHT));
        let expected = format!("sha256:{}", fake.sha());

        run(&fake, &streams, tuning(4), dir.path(), Some(&expected))
            .await
            .unwrap();
        let first = fake.requests.lock().unwrap()[0].clone();
        assert_eq!(first.expected_sha256, Some(fake.sha()));

        let wrong = "0".repeat(64);
        let err = run(&fake, &streams, tuning(4), dir.path(), Some(&wrong))
            .await
            .expect_err("a registry holding another binary must refuse");
        assert!(err.to_string().contains("binary changed"), "{err:#}");
        assert!(
            leftovers(dir.path()).len() == 1,
            "only the first, kept download remains"
        );
    }

    #[tokio::test]
    async fn a_server_without_ranges_is_read_as_one_whole_stream() {
        let data = bytes(5 * RANGE as usize + 7);
        let fake = Fake::new(data.clone(), Behaviour::IgnoresRanges);
        let dir = tempfile::tempdir().unwrap();
        let streams = Arc::new(Semaphore::new(STREAMS_IN_FLIGHT));

        let got = run(&fake, &streams, tuning(4), dir.path(), None)
            .await
            .unwrap();

        assert_eq!(std::fs::read(&got.temp_path).unwrap(), data);
        assert_eq!(got.sha256_hex, sha(&data));
        assert_eq!(fake.requests.lock().unwrap().len(), 1, "no second request");
    }

    #[tokio::test]
    async fn a_binary_smaller_than_one_range_is_one_request() {
        let data = bytes(RANGE as usize / 2);
        let fake = Fake::new(data.clone(), Behaviour::Honest);
        let dir = tempfile::tempdir().unwrap();
        let streams = Arc::new(Semaphore::new(STREAMS_IN_FLIGHT));

        let got = run(&fake, &streams, tuning(4), dir.path(), None)
            .await
            .unwrap();

        assert_eq!(std::fs::read(&got.temp_path).unwrap(), data);
        assert_eq!(fake.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_empty_binary_downloads_as_an_empty_file() {
        let fake = Fake::new(Vec::new(), Behaviour::Honest);
        let dir = tempfile::tempdir().unwrap();
        let streams = Arc::new(Semaphore::new(STREAMS_IN_FLIGHT));

        let got = run(&fake, &streams, tuning(4), dir.path(), None)
            .await
            .unwrap();

        assert_eq!(got.size_bytes, 0);
        assert_eq!(got.sha256_hex, sha(&[]));
    }

    #[tokio::test]
    async fn bad_bytes_from_the_registry_never_leave_a_file_behind() {
        let size = 4 * RANGE as usize - 50;
        let cases = [
            (Behaviour::FlipsByteAt(2500), "does not match"),
            (Behaviour::TruncatesRangeAt(1000), "ended range 1000..2000"),
            (Behaviour::RepeatsChunkInRangeAt(2000), "offset"),
            (
                Behaviour::OverrunsRangeAt(1000),
                "more than the requested range",
            ),
            (Behaviour::RepublishedBeforeRangeAt(2000), "binary changed"),
            (Behaviour::FailsRangeAt(3000), "connection reset"),
        ];
        for (behaviour, message) in cases {
            let fake = Fake::new(bytes(size), behaviour);
            let dir = tempfile::tempdir().unwrap();
            let streams = Arc::new(Semaphore::new(STREAMS_IN_FLIGHT));

            let err = run(&fake, &streams, tuning(4), dir.path(), None)
                .await
                .expect_err("corrupt or partial download must fail");

            assert!(format!("{err:#}").contains(message), "{message}: {err:#}");
            assert!(
                leftovers(dir.path()).is_empty(),
                "{message}: tempfile left behind"
            );
            assert_eq!(
                fake.in_flight.load(Ordering::SeqCst),
                0,
                "{message}: stream leaked"
            );
        }
    }

    #[tokio::test]
    async fn cancelling_a_download_removes_its_tempfile_and_closes_its_streams() {
        let fake = Fake::new(bytes(4 * RANGE as usize), Behaviour::StallsRangeAt(2000));
        let dir = tempfile::tempdir().unwrap();
        let streams = Arc::new(Semaphore::new(STREAMS_IN_FLIGHT));

        let download = run(&fake, &streams, tuning(4), dir.path(), None);
        let outcome = tokio::time::timeout(std::time::Duration::from_millis(200), download).await;

        assert!(
            outcome.is_err(),
            "the stalled range keeps the download pending"
        );
        assert!(leftovers(dir.path()).is_empty());
        assert_eq!(fake.in_flight.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn ranges_in_flight_are_bounded_per_binary_and_across_binaries() {
        let data = bytes(8 * RANGE as usize);
        let dir = tempfile::tempdir().unwrap();

        let fake = Fake::new(data.clone(), Behaviour::Honest);
        let plenty = Arc::new(Semaphore::new(STREAMS_IN_FLIGHT));
        run(&fake, &plenty, tuning(3), dir.path(), None)
            .await
            .unwrap();
        assert!(fake.peak.load(Ordering::SeqCst) <= 3);

        let shared = Arc::new(Semaphore::new(2));
        let a = Fake::new(data.clone(), Behaviour::Honest);
        let b = Fake::new(data.clone(), Behaviour::Honest);
        let (ra, rb) = tokio::join!(
            run(&a, &shared, tuning(4), dir.path(), None),
            run(&b, &shared, tuning(4), dir.path(), None),
        );
        let (ra, rb) = (ra.unwrap(), rb.unwrap());
        assert!(
            a.peak.load(Ordering::SeqCst) + b.peak.load(Ordering::SeqCst) <= 4,
            "each binary's peak is within the shared budget"
        );
        assert_eq!(std::fs::read(ra.temp_path).unwrap(), data);
        assert_eq!(std::fs::read(rb.temp_path).unwrap(), data);
        assert_eq!(shared.available_permits(), 2);
    }
}
