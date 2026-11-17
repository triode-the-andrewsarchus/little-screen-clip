//! Typed FUSE protocol boundary around one retained SQLite/read-handle owner.
use super::*;
use fuser::{
    Config, Errno, FileHandle, Filesystem, FopenFlags, Generation, LockOwner, OpenFlags, ReplyAttr,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyXattr, Request,
};
use std::sync::{Mutex, MutexGuard};

struct Server(Mutex<CatalogFs>);
impl Server {
    fn lock(&self) -> Result<MutexGuard<'_, CatalogFs>> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::from_raw_os_error(libc::EIO).into())
    }
}
pub(super) fn mount(fs: CatalogFs, path: &Path, id: &str) -> Result<()> {
    fuser::mount(Server(Mutex::new(fs)), path, &config(id))?;
    Ok(())
}
#[cfg(test)]
pub(super) fn spawn(fs: CatalogFs, path: &Path, id: &str) -> Result<fuser::BackgroundSession> {
    Ok(fuser::spawn_mount(
        Server(Mutex::new(fs)),
        path,
        &config(id),
    )?)
}
fn config(id: &str) -> Config {
    let mut config = Config::default();
    config.mount_options = vec![
        fuser::MountOption::RO,
        fuser::MountOption::NoExec,
        fuser::MountOption::NoDev,
        fuser::MountOption::NoSuid,
        fuser::MountOption::Subtype(little_clip_core::catalog_metadata::CATALOG_FS_SUBTYPE.into()),
        fuser::MountOption::FSName(format!("little-clip-catalog:{id}")),
        fuser::MountOption::CUSTOM("max_read=1048576".into()),
    ];
    // Keep the existing single-owner admission and memory budget. The mutex
    // satisfies the library's Sync contract without sharing the SQLite owner.
    config.n_threads = Some(1);
    config
}
fn errno(error: crate::Error, fallback: Errno) -> Errno {
    match error {
        crate::Error::Io(error) => error
            .raw_os_error()
            .map(Errno::from_i32)
            .unwrap_or(fallback),
        _ => fallback,
    }
}
fn xattr_reply(value: &[u8], size: u32, reply: ReplyXattr) {
    if size == 0 {
        reply.size(value.len() as u32);
    } else if size < value.len() as u32 {
        reply.error(Errno::ERANGE);
    } else {
        reply.data(value);
    }
}
impl Filesystem for Server {
    fn getxattr(&self, _req: &Request, ino: INodeNo, name: &OsStr, size: u32, reply: ReplyXattr) {
        let result = (|| -> Result<Option<Vec<u8>>> {
            let fs = self.lock()?;
            if name == OsStr::new(little_clip_core::catalog_metadata::CATALOG_ENTRY_XATTR) {
                fs.saved_entry(ino.0)
            } else if name == OsStr::new(little_clip_core::catalog_metadata::CATALOG_DETAILS_XATTR)
            {
                fs.details(ino.0)
            } else if name == OsStr::new(little_clip_core::catalog_metadata::CATALOG_STATUS_XATTR) {
                Ok(fs.coverage(ino.0)?.map(|status| status.token().to_vec()))
            } else {
                Ok(None)
            }
        })();
        match result {
            Ok(Some(value)) => xattr_reply(&value, size, reply),
            Ok(None) => reply.error(Errno::ENODATA),
            Err(error) => reply.error(errno(error, Errno::ENOENT)),
        }
    }
    fn listxattr(&self, _req: &Request, ino: INodeNo, size: u32, reply: ReplyXattr) {
        let result = (|| -> Result<Vec<u8>> {
            let fs = self.lock()?;
            let mut names = Vec::new();
            if fs.coverage(ino.0)?.is_some() {
                names.extend_from_slice(
                    little_clip_core::catalog_metadata::CATALOG_STATUS_XATTR.as_bytes(),
                );
                names.push(0);
            }
            if fs.details(ino.0)?.is_some() {
                names.extend_from_slice(
                    little_clip_core::catalog_metadata::CATALOG_DETAILS_XATTR.as_bytes(),
                );
                names.push(0);
            }
            if ino != INodeNo(1) {
                fs.row(ino.0)?
                    .ok_or_else(|| std::io::Error::from_raw_os_error(libc::ENOENT))?;
                names.extend_from_slice(
                    little_clip_core::catalog_metadata::CATALOG_ENTRY_XATTR.as_bytes(),
                );
                names.push(0);
            }
            Ok(names)
        })();
        match result {
            Ok(names) => xattr_reply(&names, size, reply),
            Err(error) => reply.error(errno(error, Errno::ENOENT)),
        }
    }
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let result = (|| -> Result<FileAttr> {
            let fs = self.lock()?;
            let path = fs.parent_path(parent.0)?.join(name);
            if !little_clip_core::drives::valid_relative(&path) {
                return Err("Invalid catalog path".into());
            }
            match fs.lookup_path(&path)? {
                Some((_, entry)) if entry.kind == "unavailable" => {
                    Err(std::io::Error::from_raw_os_error(libc::EIO).into())
                }
                Some((ino, entry)) => fs.attr(ino, Some(&entry)),
                None => Err(
                    std::io::Error::from_raw_os_error(if fs.listing_complete(parent.0)? {
                        libc::ENOENT
                    } else {
                        libc::EIO
                    })
                    .into(),
                ),
            }
        })();
        match result {
            Ok(attr) => reply.entry(&Duration::from_secs(1), &attr, Generation(0)),
            Err(error) => reply.error(errno(error, Errno::EACCES)),
        }
    }
    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let result = (|| -> Result<FileAttr> {
            let fs = self.lock()?;
            if ino == INodeNo(1) {
                return fs.attr(1, None);
            }
            match fs.row(ino.0)? {
                Some(entry) if entry.kind == "unavailable" => {
                    Err(std::io::Error::from_raw_os_error(libc::EIO).into())
                }
                Some(entry) => fs.attr(ino.0, Some(&entry)),
                None => Err(std::io::Error::from_raw_os_error(libc::ENOENT).into()),
            }
        })();
        match result {
            Ok(attr) => reply.attr(&Duration::from_secs(1), &attr),
            Err(error) => reply.error(errno(error, Errno::ENOENT)),
        }
    }
    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        let result = (|| -> Result<PathBuf> {
            let fs = self.lock()?;
            let entry = fs.row(ino.0)?.ok_or("Unknown catalog link")?;
            let link = entry.link.ok_or("Not a symlink")?;
            let mut target = entry.path.parent().unwrap_or(Path::new("")).to_owned();
            for component in link.components() {
                match component {
                    std::path::Component::Normal(name) => target.push(name),
                    std::path::Component::CurDir => (),
                    std::path::Component::ParentDir => {
                        if !target.pop() {
                            return Err("Link escapes catalog".into());
                        }
                    }
                    _ => return Err("Absolute links remain metadata-only".into()),
                }
            }
            if !target.as_os_str().is_empty() && fs.lookup_path(&target)?.is_none() {
                return Err("Link target is not catalogued".into());
            }
            Ok(link)
        })();
        match result {
            Ok(link) => reply.data(link.as_os_str().as_bytes()),
            Err(error) => reply.error(errno(error, Errno::ENODATA)),
        }
    }
    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let result = self.lock().and_then(|fs| fs.parent_path(ino.0));
        match result {
            Ok(_) => reply.opened(FileHandle(0), FopenFlags::empty()),
            Err(error) => reply.error(errno(error, Errno::EACCES)),
        }
    }
    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let result = (|| -> Result<()> {
            let fs = self.lock()?;
            fs.parent_path(ino.0)?;
            if offset < 1 && reply.add(ino, 1, FileType::Directory, ".") {
                return Ok(());
            }
            if offset < 2 && reply.add(INodeNo(1), 2, FileType::Directory, "..") {
                return Ok(());
            }
            let offset = i64::try_from(offset)
                .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
            for (id, name, entry) in fs.directory_page(ino.0, offset)? {
                if reply.add(INodeNo(id), id, file_type(&entry.kind), name) {
                    break;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(errno(error, Errno::EACCES)),
        }
    }
    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        if flags.0 & libc::O_ACCMODE != libc::O_RDONLY {
            reply.error(Errno::EROFS);
            return;
        }
        let result = (|| -> Result<u64> {
            let mut fs = self.lock()?;
            #[cfg(test)]
            fs.payload_opens
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if fs.opened.len() >= 128 {
                return Err(std::io::Error::from_raw_os_error(libc::EMFILE).into());
            }
            let entry = fs
                .row(ino.0)?
                .ok_or_else(|| std::io::Error::from_raw_os_error(libc::ENOENT))?;
            if entry.kind != "file" {
                return Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP).into());
            }
            let open = fs.resolve(&entry)?;
            fs.next_handle = fs.next_handle.wrapping_add(1).max(1);
            let handle = fs.next_handle;
            fs.opened.insert(handle, open);
            Ok(handle)
        })();
        match result {
            Ok(handle) => reply.opened(FileHandle(handle), FopenFlags::FOPEN_DIRECT_IO),
            Err(error) => {
                let fallback = if error.to_string().contains("stale") {
                    Errno::ESTALE
                } else {
                    Errno::from_i32(libc::ENOMEDIUM)
                };
                reply.error(errno(error, fallback));
            }
        }
    }
    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let result = (|| -> Result<Vec<u8>> {
            if size > 1024 * 1024 {
                return Err(std::io::Error::from_raw_os_error(libc::EINVAL).into());
            }
            let fs = self.lock()?;
            let open = fs.opened.get(&fh.0).ok_or("Unknown file handle")?;
            open.root.validate()?;
            open.source.check_file(&open.file)?;
            let mut bytes = vec![0; size as usize];
            let count = open.file.read_at(&mut bytes, offset)?;
            open.source.check_file(&open.file)?;
            bytes.truncate(count);
            Ok(bytes)
        })();
        match result {
            Ok(bytes) => reply.data(&bytes),
            Err(error) => reply.error(errno(error, Errno::ESTALE)),
        }
    }
    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        match self.lock() {
            Ok(mut fs) => {
                fs.opened.remove(&fh.0);
                reply.ok();
            }
            Err(error) => reply.error(errno(error, Errno::EIO)),
        }
    }
}
