//! `mount` subcommand

use std::ffi::OsStr;

use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::iter::Skip;
use std::mem;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::vec::IntoIter;

use infinitree::object::Pool;
use tracing::debug;
use zerostash_files::directory::Dir;
use zerostash_files::store::index_file_non_async;

use std::io::Result;

use fuse_mt::*;
use infinitree::object::{AEADReader, PoolRef, Reader};
use infinitree::{ChunkPointer, Infinitree};
use nix::libc;
use zerostash_files::{restore, Entry, Files};

pub async fn mount(
    stash: Infinitree<Files>,
    options: &restore::Options,
    mountpoint: &str,
    threads: usize,
) -> anyhow::Result<()> {
    let stash = Arc::new(Mutex::new(stash));
    let (tx, finished) = mpsc::sync_channel(2);
    let destroy_tx = tx.clone();
    ctrlc::set_handler(move || tx.send(()).expect("Could not send signal on channel."))
        .expect("Error setting Ctrl-C handler");

    let stash_clone = Arc::clone(&stash);
    tokio::spawn(async move {
        auto_commit(stash_clone).await;
    });

    let filesystem = ZerostashFS::open(stash, options, destroy_tx, threads).unwrap();
    let fuse_args = [OsStr::new("-o"), OsStr::new("fsname=zerostash")];

    let fs = fuse_mt::FuseMT::new(filesystem, 1);

    // Mount the filesystem.
    let handle = spawn_mount(fs, mountpoint, &fuse_args[..])?;

    // Wait until we are done.
    finished.recv().expect("Could not receive from channel.");

    // Ensure the filesystem is unmounted.
    handle.join();

    Ok(())
}

async fn auto_commit(stash: Arc<Mutex<Infinitree<Files>>>) {
    let mut interval = tokio::time::interval(Duration::from_secs(180));

    interval.tick().await;
    loop {
        interval.tick().await;

        let mut stash_guard = stash.lock().unwrap();
        let _ = stash_guard.commit("Fuse commit");
        let _ = stash_guard.backend().sync();
        debug!("Commited Changes!");
    }
}

pub struct ZerostashFS {
    pub commit_timestamp: SystemTime,
    pub destroy_tx: mpsc::SyncSender<()>,
    pub stash: Arc<Mutex<Infinitree<Files>>>,
    pub chunks_cache: scc::HashMap<PathBuf, ChunkStackCache>,
    pub threads: usize,
}

impl ZerostashFS {
    pub fn open(
        stash: Arc<Mutex<Infinitree<Files>>>,
        _options: &restore::Options,
        destroy_tx: mpsc::SyncSender<()>,
        threads: usize,
    ) -> Result<Self> {
        stash.lock().unwrap().load_all().unwrap();

        let commit_timestamp = stash
            .lock()
            .unwrap()
            .commit_list()
            .last()
            .unwrap()
            .metadata
            .time;

        Ok(ZerostashFS {
            commit_timestamp,
            destroy_tx,
            stash,
            chunks_cache: scc::HashMap::new(),
            threads,
        })
    }
}

impl FilesystemMT for ZerostashFS {
    fn destroy(&self) {
        debug!("destroy and commit");

        let mut stash = self.stash.lock().unwrap();
        let _ = stash.commit("Fuse commit");
        let _ = stash.backend().sync();
        self.destroy_tx
            .send(())
            .expect("Could not send signal on channel.");
    }

    fn getattr(&self, _req: RequestInfo, path: &Path, _fh: Option<u64>) -> ResultEntry {
        debug!("gettattr = {:?}", path);

        if self
            .stash
            .lock()
            .unwrap()
            .index()
            .directories
            .contains(&path.to_path_buf())
        {
            Ok((TTL, DIR_ATTR))
        } else {
            let path_string = strip_path(path).to_str().unwrap();
            match self.stash.lock().unwrap().index().files.get(path_string) {
                Some(metadata) => {
                    let fuse = file_to_fuse(&metadata, self.commit_timestamp);
                    Ok((TTL, fuse))
                }
                None => Err(libc::ENOENT),
            }
        }
    }

    fn opendir(&self, _req: RequestInfo, _path: &Path, _flags: u32) -> ResultOpen {
        debug!("opendir");
        Ok((0, 0))
    }

    fn readdir(&self, _req: RequestInfo, path: &Path, _fh: u64) -> ResultReaddir {
        debug!("readdir: {:?}", path);

        let entries = self
            .stash
            .lock()
            .unwrap()
            .index()
            .directories
            .get(path)
            .unwrap_or_default();
        let transformed_entries = transform(entries.to_vec());

        Ok(transformed_entries)
    }

    fn open(&self, _req: RequestInfo, path: &Path, _flags: u32) -> ResultOpen {
        debug!("open: {:?}", path);
        Ok((0, 0))
    }

    fn read(
        &self,
        _req: RequestInfo,
        path: &Path,
        _fh: u64,
        offset: u64,
        size: u32,
        callback: impl FnOnce(ResultSlice<'_>) -> CallbackResult,
    ) -> CallbackResult {
        debug!("read: {:?} {:#x} @ {:#x}", path, size, offset);

        let real_path = strip_path(path);
        let path_string = real_path.to_str().unwrap();
        let metadata = self
            .stash
            .lock()
            .unwrap()
            .index()
            .files
            .get(path_string)
            .unwrap();
        let file_size = metadata.size as usize;
        let offset = offset as usize;

        if offset > file_size {
            return callback(Err(libc::EINVAL));
        }

        let size = size as usize;
        let sort_chunks = || {
            let mut chunks = metadata.chunks.clone();
            chunks.sort_by(|(a, _), (b, _)| a.cmp(b));
            chunks
        };
        let mut obj_reader = self.stash.lock().unwrap().storage_reader().unwrap();

        {
            let mut chunks = self
                .chunks_cache
                .entry(real_path.to_path_buf())
                .or_insert_with(|| ChunkStackCache::new(sort_chunks()));
            let chunks = chunks.get_mut();

            if chunks.last_read_offset == offset {
                let end = size.min(file_size - offset);
                if chunks.buf.len() < end {
                    loop {
                        if chunks.read_next(file_size, &mut obj_reader).is_err() {
                            return callback(Err(libc::EINVAL));
                        }

                        if chunks.buf.len() >= end {
                            break;
                        }
                    }
                }
                let ret_buf = chunks.split_buf(end);
                chunks.set_current_read(offset + end);
                return callback(Ok(&ret_buf));
            }
        }

        let mut chunks = ChunkStack::new(sort_chunks(), offset);

        loop {
            if chunks
                .read_next(file_size, offset, &mut obj_reader)
                .is_err()
            {
                return callback(Err(libc::EINVAL));
            }

            if chunks.is_full(size, file_size, offset) {
                let from = chunks.start.unwrap();
                let to = chunks.end.unwrap();
                return callback(Ok(&chunks.buf[from..to]));
            }
        }
    }

    fn write(
        &self,
        _req: RequestInfo,
        path: &Path,
        _fh: u64,
        offset: u64,
        data: Vec<u8>,
        _flags: u32,
    ) -> ResultWrite {
        debug!("write: {:?} {:#x} @ {:#x}", path, data.len(), offset);

        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();

        if let Err(e) = file.seek(SeekFrom::Start(offset)) {
            return Err(e.raw_os_error().unwrap());
        }

        let nwritten: u32 = match file.write(&data) {
            Ok(n) => n as u32,
            Err(e) => {
                return Err(e.raw_os_error().unwrap());
            }
        };

        if let Err(e) = file.seek(SeekFrom::Start(0)) {
            return Err(e.raw_os_error().unwrap());
        }

        let metadata = file.metadata().unwrap();
        let path = strip_path(path);
        let preserve = zerostash_files::PreserveMetadata::default();
        let entry = match zerostash_files::Entry::from_metadata(metadata, &path, &preserve) {
            Ok(e) => e,
            Err(_) => {
                return Err(libc::EINVAL);
            }
        };

        {
            let stash = self.stash.lock().unwrap();
            let hasher = stash.hasher().unwrap();
            let index = stash.index();
            let path_str = path.to_str().unwrap().to_owned();
            let balancer = Pool::new(
                NonZeroUsize::new(self.threads).unwrap(),
                stash.storage_writer().unwrap(),
            )
            .unwrap();
            index_file_non_async(file, entry, hasher, &balancer, &index, path_str);
        }

        Ok(nwritten)
    }

    fn truncate(&self, _req: RequestInfo, path: &Path, _fh: Option<u64>, size: u64) -> ResultEmpty {
        debug!("truncate: {:?} {:#x}", path, size);
        Ok(())
    }

    fn release(
        &self,
        _req: RequestInfo,
        path: &Path,
        _fh: u64,
        _flags: u32,
        _lock_owner: u64,
        _flush: bool,
    ) -> ResultEmpty {
        debug!("release {:?}", path);
        let real_path = strip_path(path);

        self.chunks_cache.remove(real_path);

        Ok(())
    }
}

type Chunks = Skip<IntoIter<(u64, Arc<ChunkPointer>)>>;

pub struct ChunksIter {
    pub chunks: std::iter::Peekable<Chunks>,
}

impl ChunksIter {
    fn new(chunks: Chunks) -> Self {
        let chunks = chunks.peekable();
        Self { chunks }
    }

    fn peek_next_offset(&mut self, file_size: usize) -> usize {
        let arc = (file_size as u64, Arc::new(ChunkPointer::default()));
        let (chunk_offset, _) = self.chunks.peek().unwrap_or(&arc);
        *chunk_offset as usize
    }

    fn get_next(&mut self) -> Option<(usize, Arc<ChunkPointer>)> {
        let (c_offset, pointer) = match self.chunks.next() {
            Some((o, p)) => (o as usize, p),
            None => return None,
        };
        Some((c_offset, pointer))
    }
}

#[derive(Debug)]
pub enum ChunkDataError {
    NullChunkPointer,
}

pub struct ChunkStackCache {
    pub chunks: ChunksIter,
    pub buf: Vec<u8>,
    pub last_read_offset: usize,
}

impl ChunkStackCache {
    fn new(chunks: Vec<(u64, Arc<ChunkPointer>)>) -> Self {
        let chunks = ChunksIter::new(chunks.into_iter().skip(0));
        Self {
            chunks,
            buf: Default::default(),
            last_read_offset: Default::default(),
        }
    }

    fn set_current_read(&mut self, val: usize) {
        self.last_read_offset = val;
    }

    fn split_buf(&mut self, end: usize) -> Vec<u8> {
        let mut ret_buf = self.buf.split_off(end);
        mem::swap(&mut self.buf, &mut ret_buf);
        ret_buf
    }

    #[inline(always)]
    fn read_next(
        &mut self,
        file_size: usize,
        objectreader: &mut PoolRef<AEADReader>,
    ) -> anyhow::Result<(), ChunkDataError> {
        let (c_offset, pointer) = match self.chunks.get_next() {
            Some(chunk) => chunk,
            None => return Err(ChunkDataError::NullChunkPointer),
        };
        let next_c_offset = self.chunks.peek_next_offset(file_size);

        let mut temp_buf = vec![0; next_c_offset - c_offset];
        objectreader.read_chunk(&pointer, &mut temp_buf).unwrap();
        self.buf.append(&mut temp_buf);

        Ok(())
    }
}

pub struct ChunkStack {
    pub chunks: ChunksIter,
    pub buf: Vec<u8>,
    pub start: Option<usize>,
    pub end: Option<usize>,
}

impl ChunkStack {
    fn new(chunks: Vec<(u64, Arc<ChunkPointer>)>, offset: usize) -> Self {
        let index = match chunks.binary_search_by(|a| a.0.cmp(&(offset as u64))) {
            Ok(v) => v,
            Err(v) => v - 1,
        };
        let chunks = ChunksIter::new(chunks.into_iter().skip(index));
        Self {
            chunks,
            buf: Default::default(),
            start: None,
            end: None,
        }
    }

    #[inline(always)]
    fn read_next(
        &mut self,
        file_size: usize,
        offset: usize,
        objectreader: &mut PoolRef<AEADReader>,
    ) -> anyhow::Result<(), ChunkDataError> {
        let (c_offset, pointer) = match self.chunks.get_next() {
            Some(chunk) => chunk,
            None => return Err(ChunkDataError::NullChunkPointer),
        };
        let next_c_offset = self.chunks.peek_next_offset(file_size);

        if self.start.is_none() {
            self.start = Some(offset - c_offset);
        }
        let mut temp_buf = vec![0; next_c_offset - c_offset];
        objectreader.read_chunk(&pointer, &mut temp_buf).unwrap();
        self.buf.append(&mut temp_buf);

        Ok(())
    }

    #[inline(always)]
    fn is_full(&mut self, size: usize, file_size: usize, offset: usize) -> bool {
        if let Some(from) = self.start {
            if self.buf[from..].len() >= size.min(file_size - offset) {
                self.end = Some(self.buf.len().min(from + size));
                return true;
            }
        }
        false
    }
}

const TTL: Duration = Duration::from_secs(1);

const DIR_ATTR: FileAttr = FileAttr {
    size: 0,
    blocks: 0,
    atime: SystemTime::UNIX_EPOCH,
    mtime: SystemTime::UNIX_EPOCH,
    ctime: SystemTime::UNIX_EPOCH,
    crtime: SystemTime::UNIX_EPOCH,
    kind: FileType::Directory,
    perm: 0o444,
    nlink: 1,
    uid: 1000,
    gid: 1000,
    rdev: 0,
    flags: 0,
};

fn transform(entries: Vec<Dir>) -> Vec<DirectoryEntry> {
    let mut vec = vec![];
    for entry in entries.iter() {
        let new_entry = DirectoryEntry {
            name: entry.path.file_name().unwrap().into(),
            kind: match entry.file_type {
                zerostash_files::FileType::Directory => fuse_mt::FileType::Directory,
                _ => fuse_mt::FileType::RegularFile,
            },
        };
        vec.push(new_entry);
    }
    vec
}

fn file_to_fuse(file: &Arc<Entry>, atime: SystemTime) -> FileAttr {
    let mtime = UNIX_EPOCH
        + Duration::from_secs(file.unix_secs as u64)
        + Duration::from_nanos(file.unix_nanos as u64);
    FileAttr {
        size: file.size,
        blocks: 1,
        atime,
        mtime,
        ctime: mtime,
        crtime: SystemTime::UNIX_EPOCH,
        kind: FileType::RegularFile,
        perm: 0o444,
        nlink: 1,
        gid: file
            .unix_gid
            .unwrap_or_else(|| nix::unistd::getgid().into()),
        uid: file
            .unix_uid
            .unwrap_or_else(|| nix::unistd::getuid().into()),
        rdev: 0,
        flags: 0,
    }
}

fn strip_path(path: &Path) -> &Path {
    path.strip_prefix("/").unwrap()
}
