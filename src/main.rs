mod utils;

use crate::utils::{system_time_from_time, time_from_system_time, time_now};
use clap::{Arg, ArgAction, Command};
use fuser::{FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyDirectory, ReplyWrite, Request, TimeOrNow};
use fuser::{MountOption, ReplyEntry, FUSE_ROOT_ID};
use libc::c_int;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::mem::size_of;
use std::time::{Duration, SystemTime};

const DIR_MODE: u8 = 0;
const FILE_MODE: u8 = 1;

const BLOCK_SIZE: u32 = 512;

static mut INODE_SERIAL_NUMER: u64 = 2;
fn get_next_serial_number() -> u64 {
    unsafe {
        let serial_number = INODE_SERIAL_NUMER;
        INODE_SERIAL_NUMER += 1;
        serial_number
    }
}

static mut MAX_MEMORY: u64 = 0; // Max memory of the program in MB

fn set_max_memory(mb: u64) {
    unsafe {
        MAX_MEMORY = mb * 1024 * 1024;
    }
}

fn get_max_memory() -> u64 {
    unsafe { MAX_MEMORY }
}

const MAX_FILE_NAME_LENGTH: usize = 255; // Max file name length in bytes

static mut MAX_FILE_SIZE: u64 = 0; // Max file size in MB

fn set_max_file_size(mb: u64) {
    unsafe {
        MAX_FILE_SIZE = mb * 1024 * 1024;
    }
}

fn get_max_file_size() -> u64 {
    unsafe { MAX_FILE_SIZE }
}

#[derive(Debug, Clone)]
pub enum InodeData {
    File(File),
    Directory(Directory),
}

impl From<InodeData> for FileType {
    fn from(kind: InodeData) -> Self {
        match kind {
            InodeData::File(_) => FileType::RegularFile,
            InodeData::Directory(_) => FileType::Directory,
        }
    }
}

struct VFFS {
    inodes: HashMap<u64, Inode>,
}

impl VFFS {
    fn new(mount: &String) -> VFFS {
        let root = Inode::new(DIR_MODE, mount.clone(), FUSE_ROOT_ID);
        let mut inodes = HashMap::new();
        inodes.insert(FUSE_ROOT_ID, root);
        VFFS { inodes }
    }

    fn lookup_node(&self, id: u64) -> Result<&Inode, c_int> {
        let inode = self.inodes.get(&id);
        match inode {
            Some(inode) => {
                // println!("Found inode: {:?}", inode);
                Ok(inode)
            }
            None => {
                println!("Inode not found for iid: {}", id);
                Err(libc::ENOENT)
            }
        }
    }

    fn lookup_node_mut(&mut self, iid: u64) -> Result<&mut Inode, c_int> {
        let inode = self.inodes.get_mut(&iid);
        match inode {
            Some(inode) => {
                // println!("Found inode: {:?}", inode);
                Ok(inode)
            }
            None => {
                println!("Inode not found for iid: {}", iid);
                Err(libc::ENOENT)
            }
        }
    }

    fn append_inode(&mut self, inode: Inode) {
        self.inodes.insert(inode.id, inode);
    }
}

impl Filesystem for VFFS {
    /// Create a new file in the specified parent directory.
    /// The creation of the file consists of allocating a new inode, adding it to the VFFS
    /// and updating the parent directory structure to include the new file.
    ///
    /// The `parent` parameter is the inode number of the parent directory,
    /// `name` is the name of the new file to be created
    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        // println!("create() called with parent: {parent}, name: {:?}, mode: {mode:o}, umask: {umask:o}, flags: {flags}",
        //     name.to_str().unwrap()
        // );
        let name_str = name.to_str().unwrap().to_string();

        // Check if parent is a directory
        // It must be in a local scope to avoid holding the borrow too long
        {
            let parent_inode = match self.lookup_node(parent) {
                Ok(inode) => inode,
                Err(err) => {
                    reply.error(err);
                    return;
                }
            };

            if !parent_inode.is_directory() {
                reply.error(libc::ENOTDIR);
                return;
            }
        }

        let new_inode = Inode {
            id: get_next_serial_number(),
            size: 0,
            updated_at: time_now(),
            accessed_at: time_now(),
            metadata_change_at: time_now(),
            data: InodeData::File(File::new(name_str.clone())),
            mode: (mode & !umask) as u16,
            hardlinks: 1,
            uid: _req.uid(),
            gid: _req.gid(),
            xattrs: BTreeMap::default(),
        };
        let new_inode_id = new_inode.id;
        let file_attr: FileAttr = (&new_inode).into();

        // Add the new inode to the filesystem
        self.append_inode(new_inode);

        // Add the new file to the parent directory structure
        if let Ok(parent_inode) = self.lookup_node_mut(parent) {
            let inode_data = (new_inode_id, name_str, FileType::RegularFile);
            parent_inode.append_file_to_directory(inode_data);
        } else {
            reply.error(libc::ENOENT);
            return;
        }

        println!(
            "Created inode {:?} for create with parent: {parent} and name: {:?}",
            new_inode_id,
            name.to_str()
        );
        reply.created(&Duration::new(0, 0), &file_attr, 0, 0, 0);
    }

    /// Look up a directory entry by name and get its attributes.
    /// The method searches for any entry with the given name in the specified parent directory
    /// structure. If found, it retrieves the corresponding inode from the VFFS and returns its
    /// attributes.
    ///
    /// The `parent` parameter is the inode number of the parent directory,
    /// and `name` is the name of the entry to look up.
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = name.to_str().unwrap();
        // println!("lookup parent: {parent}, name: {name_str}. Looking for inode...");

        match self.lookup_node(parent) {
            Ok(inode) => {
                // Check if is Directory
                {
                    if !inode.is_directory() {
                        reply.error(libc::ENOTDIR);
                        return;
                    }
                }

                // Access its files
                let directory = match &inode.data {
                    InodeData::Directory(dir) => dir,
                    _ => {
                        reply.error(libc::ENOTDIR);
                        return;
                    }
                };

                // Find the file with the given name
                let file_entry = match directory.find_file_by_name(name_str) {
                    Some(file) => file,
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                };

                // Find the inode for the file
                let file_inode = match self.lookup_node(file_entry.0) {
                    Ok(inode) => inode,
                    Err(err) => {
                        reply.error(err);
                        return;
                    }
                };

                println!(
                    "Found inode {:?} for lookup with parent: {parent} and name: {name_str}",
                    file_inode
                );
                reply.entry(&Duration::new(0, 0), &file_inode.into(), 0)
            }
            Err(err) => reply.error(err),
        }
    }

    /// Get the attributes of a file or directory by its inode number.
    /// The method retrieves the attributes of the specified inode from the VFFS.
    ///
    /// The `ino` parameter is the inode number of the file or directory.
    fn getattr(&mut self, _req: &Request<'_>, ino: u64, fh: Option<u64>, reply: ReplyAttr) {
        // println!("getattr() called with ino: {ino}, fh: {fh:?}");
        match self.lookup_node(ino) {
            Ok(inode) => {
                println!("Found inode {:?} for getattr with ino: {ino} ", inode);
                reply.attr(&Duration::new(0, 0), &inode.into())
            }
            Err(err) => reply.error(err),
        }
    }

    /// Read the contents of a directory.
    /// The method retrieves the list of entries in the specified directory inode
    /// from the VFFS and adds them to the reply.
    ///
    /// The `ino` parameter is the inode number of the directory to read.
    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        // println!("readdir() called with ino: {ino}, fh: {fh}, offset: {offset}");
        match self.lookup_node(ino) {
            Ok(inode) => {
                // println!("Found inode for readdir: {:?}", inode);
                match &inode.data {
                    InodeData::Directory(directory) => {
                        let mut entry_offset: i64 = 0;
                        for (id, name, filetype) in &directory.files {
                            if entry_offset >= offset {
                                let buffer_full = reply.add(*id, entry_offset + 1, *filetype, name);

                                if buffer_full {
                                    break;
                                }
                            }
                            entry_offset += 1;
                        }
                        reply.ok();
                    }
                    _ => {
                        eprintln!("Error: trying to read a non-directory inode");
                        reply.error(libc::ENOTDIR);
                    }
                }
            }
            Err(err) => reply.error(err),
        }
    }

    /// Set the attributes of a file or directory.
    /// The method updates received attributes of the specified inode in the VFFS
    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        // println!(
        //     "setattr() called with ino: {ino}, mode: {:?}, uid: {:?}, gid: {:?}, size: {:?}, fh: {:?}, flags: {:?}",
        //     mode, uid, gid, size, fh, flags
        // );
        match self.lookup_node_mut(ino) {
            Ok(inode) => {
                if let Some(new_mode) = mode {
                    inode.mode = new_mode as u16;
                }
                if let Some(new_uid) = uid {
                    inode.uid = new_uid;
                }
                if let Some(new_gid) = gid {
                    inode.gid = new_gid;
                }
                if let Some(new_size) = size {
                    inode.size = new_size;
                }
                if let Some(access_time) = _atime {
                    match access_time {
                        TimeOrNow::SpecificTime(system_time) => {
                            inode.accessed_at = time_from_system_time(&system_time);
                        }
                        TimeOrNow::Now => {
                            inode.accessed_at = time_now();
                        }
                    }
                }
                inode.update_changes();
            }
            Err(err) => {
                reply.error(err);
                return;
            }
        }

        let inode = match self.lookup_node(ino) {
            Ok(inode) => inode,
            Err(err) => {
                reply.error(err);
                return;
            }
        };
        println!("Updated inode for setattr: {:?}", inode);
        reply.attr(&Duration::new(0, 0), &inode.into());
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        write_flags: u32,
        flags: i32,
        lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        // println!(
        //     "write() called with ino: {ino}, fh: {fh}, offset: {offset}, data size: {}, write_flags: {write_flags}, flags: {flags}, lock_owner: {:?}",
        //     data.len()
        // );
        match self.lookup_node_mut(ino) {
            Ok(inode) => {
                if inode.is_file() {
                    match &mut inode.data {
                        InodeData::File(file) => {
                            file.write_date(data);
                            inode.size = file.data.len() as u64;
                            inode.update_changes();

                            println!("Written data to inode for write: {:?}", inode);
                            reply.written(data.len() as u32);
                        }
                        _ => {
                            eprintln!("Error: trying to write to a non-file inode");
                            reply.error(libc::EISDIR);
                        }
                    }
                } else {
                    eprintln!("Error: trying to write to a non-file inode");
                    reply.error(libc::EISDIR);
                }
            }
            Err(err) => {
                reply.error(err);
            }
        }
    }

    fn mkdir(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mut mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let name_str = name.to_str().unwrap().to_string();

        // println!(
        //     "mkdir() called with {parent:?} {name:?} {mode:o}"
        // );
        {
            if let Ok(parent_inode) = self.lookup_node(parent) {
                if let InodeData::Directory(dir) = &parent_inode.data {
                    if dir.find_file_by_name(&name_str).is_some() {
                        reply.error(libc::EEXIST);
                        return;
                    }
                } else {
                    reply.error(libc::ENOTDIR);
                    return;
                }
            } else {
                reply.error(libc::ENOENT);
                return;
            }
        }

        // Update parent metadata
        match self.lookup_node_mut(parent) {
            Ok(parent_inode) => {
                parent_inode.update_changes();
            }
            Err(err) => {
                reply.error(err);
                return;
            }
        };

        let new_inode = Inode {
            id: get_next_serial_number(),
            size: (size_of::<Inode>() + size_of::<Directory>()) as u64,
            updated_at: time_now(),
            accessed_at: time_now(),
            metadata_change_at: time_now(),
            data: InodeData::Directory(Directory::new(name_str.clone())),
            mode: (mode & !umask) as u16,
            hardlinks: 2,
            uid: req.uid(),
            gid: req.gid(), 
            xattrs: BTreeMap::default(),
        };

        let new_inode_id = new_inode.id;
        let attr_reply = (&new_inode).into();

        self.append_inode(new_inode);

        // Link directory to parent
        match self.lookup_node_mut(parent) {
            Ok(parent_inode) => {
                let entry = (new_inode_id, name_str.clone(), FileType::Directory);
                parent_inode.append_file_to_directory(entry);
            }
            Err(err) => {
                reply.error(err);
                return;
            }
        }

        println!(
            "Created inode {:?} for mkdir with parent: {parent} and name: {:?}",
            new_inode_id,
            name.to_str()
        );

        reply.entry(&Duration::new(0, 0), &attr_reply, 0);
    }
}

#[derive(Debug)]
pub struct Inode {
    id: u64,
    size: u64,
    updated_at: (i64, u32),
    accessed_at: (i64, u32),
    metadata_change_at: (i64, u32),
    data: InodeData,
    // Permissions and special mode bits
    pub mode: u16,
    pub hardlinks: u32,
    pub uid: u32,
    pub gid: u32,
    pub xattrs: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Clone for Inode {
    fn clone(&self) -> Inode {
        Inode {
            id: self.id,
            size: self.size,
            updated_at: self.updated_at,
            accessed_at: self.accessed_at,
            metadata_change_at: self.metadata_change_at,
            data: match &self.data {
                InodeData::File(file) => InodeData::File(file.clone()),
                InodeData::Directory(directory) => InodeData::Directory(directory.clone()),
            },
            mode: self.mode,
            hardlinks: self.hardlinks,
            uid: self.uid,
            gid: self.gid,
            xattrs: self.xattrs.clone(),
        }
    }
}

impl From<Inode> for FileAttr {
    fn from(attrs: Inode) -> Self {
        FileAttr {
            ino: attrs.id,
            size: attrs.size,
            blocks: attrs.size.div_ceil(u64::from(BLOCK_SIZE)),
            atime: system_time_from_time(attrs.accessed_at.0, attrs.accessed_at.1),
            mtime: system_time_from_time(attrs.updated_at.0, attrs.updated_at.1),
            ctime: system_time_from_time(attrs.metadata_change_at.0, attrs.metadata_change_at.1),
            crtime: SystemTime::UNIX_EPOCH,
            kind: attrs.data.into(),
            perm: attrs.mode,
            nlink: attrs.hardlinks,
            uid: attrs.uid,
            gid: attrs.gid,
            rdev: 0,
            blksize: BLOCK_SIZE,
            flags: 0,
        }
    }
}

impl From<&Inode> for FileAttr {
    fn from(attrs: &Inode) -> Self {
        FileAttr {
            ino: attrs.id,
            size: attrs.size,
            blocks: attrs.size.div_ceil(u64::from(BLOCK_SIZE)),
            atime: system_time_from_time(attrs.accessed_at.0, attrs.accessed_at.1),
            mtime: system_time_from_time(attrs.updated_at.0, attrs.updated_at.1),
            ctime: system_time_from_time(attrs.metadata_change_at.0, attrs.metadata_change_at.1),
            crtime: SystemTime::UNIX_EPOCH,
            kind: attrs.data.clone().into(),
            perm: attrs.mode,
            nlink: attrs.hardlinks,
            uid: attrs.uid,
            gid: attrs.gid,
            rdev: 0,
            blksize: BLOCK_SIZE,
            flags: 0,
        }
    }
}

impl Inode {
    pub fn new(mode: u8, name: String, serial_number: u64) -> Inode {
        if mode == DIR_MODE {
            let size = (size_of::<Inode>() + size_of::<Directory>()) as u64;
            Inode {
                id: serial_number,
                size,
                updated_at: time_now(),
                accessed_at: time_now(),
                metadata_change_at: time_now(),
                data: InodeData::Directory(Directory::new(name)),
                mode: 0o777,
                hardlinks: 0,
                uid: 0,
                gid: 0,
                xattrs: BTreeMap::default(),
            }
        } else {
            let size = (size_of::<Inode>() + size_of::<File>()) as u64;
            Inode {
                id: serial_number,
                size,
                updated_at: time_now(),
                accessed_at: time_now(),
                metadata_change_at: time_now(),
                data: InodeData::File(File::new(name)),
                mode: 0o777,
                hardlinks: 0,
                uid: 0,
                gid: 0,
                xattrs: BTreeMap::default(),
            }
        }
    }

    // pub fn remove_inode(&mut self, rem_inode: Inode) {
    //     if self.is_directory() {
    //         self.size -= rem_inode.size;
    //         match &mut self.data {
    //             InodeData::Directory(directory) => {
    //                 let mut index = 0;
    //                 for (i, child_inode) in directory.files.iter().enumerate() {
    //                     if rem_inode.id == child_inode.id {
    //                         match &rem_inode.data {
    //                             InodeData::Directory(directory) => {
    //                                 directory.clone().recursive_remove();
    //                             }
    //                             _ => {}
    //                         }
    //                         index = i;
    //                         break;
    //                     }
    //                 }
    //                 directory.files.remove(index);
    //             }
    //             _ => eprintln!("Error: trying to remove a file from a non-directory inode"),
    //         }
    //     }
    // }

    pub fn get_name(&self) -> &String {
        match &self.data {
            InodeData::File(file) => &file.name,
            InodeData::Directory(directory) => &directory.name,
        }
    }

    pub fn get_size(&self) -> u64 {
        self.size
    }

    pub fn is_file(&self) -> bool {
        match self.data {
            InodeData::File(_) => true,
            _ => false,
        }
    }

    pub fn is_directory(&self) -> bool {
        match self.data {
            InodeData::Directory(_) => true,
            _ => false,
        }
    }

    // pub fn add_inode(&mut self, inode: Inode) {
    //     if self.is_directory() {
    //         self.size += inode.size;
    //         match &mut self.data {
    //             InodeData::Directory(directory) => directory.add_inode(inode),
    //             _ => eprintln!("Error: trying to add a file to a non-directory inode"),
    //         }
    //     } else {
    //         // todo: handle error
    //         eprintln!("Error: trying to add a file to a non-directory inode");
    //     }
    // }

    // pub fn get_inode_by_name(&self, name: &str) -> Option<Inode> {
    //     if self.is_directory() {
    //         match &self.data {
    //             InodeData::Directory(directory) => {
    //                 for inode in &directory.files {
    //                     if inode.get_name() == name {
    //                         return Some(inode.clone());
    //                     }
    //                 }
    //                 None
    //             }
    //             _ => None,
    //         }
    //     } else {
    //         None
    //     }
    // }

    pub fn update_changes(&mut self) {
        let now = time_now();
        self.updated_at = now;
        self.metadata_change_at = now;
    }

    pub fn append_file_to_directory(&mut self, file: (u64, String, FileType)) {
        match &mut self.data {
            InodeData::Directory(directory) => {
                directory.add_inode(file);
            }
            _ => {
                eprintln!("Error: trying to append a file to a non-directory inode");
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct File {
    name: String,
    data: String,
}

impl File {
    pub fn new(name: String) -> File {
        File {
            name,
            data: String::new(),
        }
    }

    pub fn new_with_data(name: String, data: String) -> File {
        File { name, data }
    }

    pub fn write_date(&mut self, data: &[u8]) {
        let data_str = String::from_utf8_lossy(data).to_string();
        self.data.push_str(&data_str);
    }

    pub fn clone(&self) -> File {
        File {
            name: self.name.clone(),
            data: self.data.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Directory {
    name: String,
    files: Vec<(u64, String, FileType)>,
}

impl Directory {
    pub fn new(name: String) -> Directory {
        Directory {
            name,
            files: Vec::new(),
        }
    }

    pub fn clone(&self) -> Directory {
        Directory {
            name: self.name.clone(),
            files: self.files.clone(),
        }
    }

    pub fn add_inode(&mut self, inode: (u64, String, FileType)) {
        self.files.push(inode);
    }

    pub fn find_file_by_name(&self, name: &str) -> Option<(u64, String, FileType)> {
        for file in &self.files {
            if file.1 == name {
                return Some(file.clone());
            }
        }
        None
    }

    // pub fn recursive_remove(&mut self) {
    //     let mut index = 0;
    //     for inode in self.files.clone() {
    //         match &inode.data {
    //             InodeData::Directory(child_directory) => {
    //                 child_directory.clone().recursive_remove();
    //                 self.files.remove(index);
    //             }
    //             InodeData::File(_) => {
    //                 self.files.remove(index);
    //             }
    //         }
    //         index += 1;
    //     }
    // }
}

fn main() {
    let matches = Command::new("VFFS")
        .arg(
            Arg::new("mount-point")
                .long("mount-point")
                .value_name("MOUNT_POINT")
                .default_value("/tmp/vffs")
                .help("Act as a client, and mount FUSE at given path"),
        )
        .arg(
            Arg::new("v")
                .short('v')
                .action(ArgAction::Count)
                .help("Sets the level of verbosity"),
        )
        .arg(
            Arg::new("mem")
                .short('m')
                .long("memory-limit")
                .help("Sets the maximum memory usage in MB")
                .required(true),
        )
        .arg(
            Arg::new("file-size")
                .short('s')
                .long("max-file-size")
                .help("Sets the maximum file size in MB")
                .default_value("1"),
        )
        .get_matches();

    let mem_limit: u64 = matches
        .get_one::<String>("mem")
        .unwrap()
        .parse()
        .expect("Memory limit must be a number");
    set_max_memory(mem_limit);

    let file_size_limit: u64 = matches
        .get_one::<String>("file-size")
        .unwrap()
        .parse()
        .expect("File size limit must be a number");
    set_max_file_size(file_size_limit);

    let verbosity = matches.get_count("v");
    let log_level = match verbosity {
        0 => LevelFilter::Error,
        1 => LevelFilter::Warn,
        2 => LevelFilter::Info,
        3 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };
    env_logger::builder()
        .format_timestamp_nanos()
        .filter_level(log_level)
        .init();

    let mountpoint: String = matches
        .get_one::<String>("mount-point")
        .unwrap()
        .to_string();

    let options = vec![MountOption::FSName("VFFS".to_string())];

    fuser::mount2(VFFS::new(&mountpoint), mountpoint, &options).unwrap();
}
