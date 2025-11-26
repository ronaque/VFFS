mod utils;

use crate::utils::{system_time_from_time, time_now};
use clap::{Arg, ArgAction, Command};
use fuser::{FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyDirectory, Request};
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

struct MyFS {
    inodes: HashMap<u64, Inode>,
}

impl MyFS {
    fn new(mount: &String) -> MyFS {
        let root = Inode::new(DIR_MODE, mount.clone(), FUSE_ROOT_ID);
        let mut inodes = HashMap::new();
        inodes.insert(FUSE_ROOT_ID, root);
        MyFS { inodes }
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

impl Filesystem for MyFS {
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
        let name_str = name.to_str().unwrap().to_string();

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

        let new_inode = Inode::new(FILE_MODE, name_str.clone(), get_next_serial_number());
        let new_inode_id = new_inode.id;
        let file_attr: FileAttr = (&new_inode).into();

        self.append_inode(new_inode);

        if let Ok(parent_inode) = self.lookup_node_mut(parent) {
            let inode_data = (new_inode_id, name_str, FileType::RegularFile);
            parent_inode.append_file_to_directory(inode_data);
        } else {
            reply.error(libc::ENOENT);
            return;
        }

        reply.created(&Duration::new(0, 0), &file_attr, 0, 0, 0);
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = name.to_str().unwrap();
        println!("lookup parent: {parent}, name: {name_str}. Looking for inode...");

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

                reply.entry(&Duration::new(0, 0), &file_inode.into(), 0)
            }
            Err(err) => reply.error(err),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, fh: Option<u64>, reply: ReplyAttr) {
        // println!("getattr() called with ino: {ino}, fh: {fh:?}");
        match self.lookup_node(ino) {
            Ok(inode) => {
                // println!("Found inode for getattr: {:?}", inode);
                reply.attr(&Duration::new(0, 0), &inode.into())
            }
            Err(err) => reply.error(err),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        println!("readdir() called with ino: {ino}, fh: {fh}, offset: {offset}");
        match self.lookup_node(ino) {
            Ok(inode) => {
                println!("Found inode for readdir: {:?}", inode);
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
                .default_value("")
                .help("Act as a client, and mount FUSE at given path"),
        )
        .arg(
            Arg::new("v")
                .short('v')
                .action(ArgAction::Count)
                .help("Sets the level of verbosity"),
        )
        .get_matches();

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

    fuser::mount2(MyFS::new(&mountpoint), mountpoint, &options).unwrap();
}
