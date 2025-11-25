mod utils;

use clap::{Arg, ArgAction, Command};
use fuser::MountOption;
use fuser::{Filesystem, KernelConfig, Request};
use log::LevelFilter;
use std::collections::HashMap;
use std::mem::size_of;
use std::os::raw::c_int;

pub const DIR_MODE: u8 = 0;
pub const FILE_MODE: u8 = 1;

const ROOT_INODE: u64 = 0;
static mut INODE_SERIAL_NUMER: u64 = 0;

#[derive(Debug, Clone)]
pub enum InodeData {
    File(File),
    Directory(Directory),
}

struct MyFS {
    inodes: HashMap<u64, Inode>,
}

impl MyFS {
    fn new(mount: &String) -> MyFS {
        let root = Inode::new(DIR_MODE, mount.clone());
        let mut inodes = HashMap::new();
        inodes.insert(ROOT_INODE, root);
        MyFS { inodes }
    }
}

impl Filesystem for MyFS {
}

#[derive(Debug, Clone)]
pub struct Inode {
    mode: u8,                  // file or directory
    size: u64,                 // in bytes
    permissions: (bool, bool), // (read, write)
    created_at: Option<u64>,
    updated_at: Option<u64>,
    accessed_at: Option<u64>,
    serial_number: u64,
    data: InodeData,
}

impl Inode {
    pub fn new(mode: u8, name: String) -> Inode {
        let serial_number: u64 = unsafe { INODE_SERIAL_NUMER };
        unsafe {
            INODE_SERIAL_NUMER += 1;
        }
        if mode == DIR_MODE {
            let size = (size_of::<Inode>() + size_of::<Directory>()) as u64;
            Inode {
                mode,
                size,
                permissions: (true, true),
                created_at: Some(utils::now_date()),
                updated_at: Some(utils::now_date()),
                accessed_at: Some(utils::now_date()),
                serial_number,
                data: InodeData::Directory(Directory::new(name)),
            }
        } else {
            let size = (size_of::<Inode>() + size_of::<File>()) as u64;
            Inode {
                mode,
                size,
                permissions: (true, true),
                created_at: Some(utils::now_date()),
                updated_at: Some(utils::now_date()),
                accessed_at: Some(utils::now_date()),
                serial_number,
                data: InodeData::File(File::new(name)),
            }
        }
    }

    pub fn new_file_with_data(name: String, data: String) -> Inode {
        let size = (size_of::<Inode>() + size_of::<File>() + data.len()) as u64;
        let serial_number: u64 = unsafe { INODE_SERIAL_NUMER };
        unsafe {
            INODE_SERIAL_NUMER += 1;
        }
        Inode {
            mode: FILE_MODE,
            size,
            permissions: (true, true),
            created_at: Some(utils::now_date()),
            updated_at: Some(utils::now_date()),
            accessed_at: Some(utils::now_date()),
            serial_number,
            data: InodeData::File(File::new_with_data(name, data)),
        }
    }

    pub fn remove_inode(&mut self, rem_inode: Inode) {
        if self.is_directory() {
            self.size -= rem_inode.size;
            match &mut self.data {
                InodeData::Directory(directory) => {
                    let mut index = 0;
                    for (i, child_inode) in directory.files.iter().enumerate() {
                        if rem_inode.serial_number == child_inode.serial_number {
                            match &rem_inode.data {
                                InodeData::Directory(directory) => {
                                    directory.clone().recursive_remove();
                                }
                                _ => {}
                            }
                            index = i;
                            break;
                        }
                    }
                    directory.files.remove(index);
                }
                _ => eprintln!("Error: trying to remove a file from a non-directory inode"),
            }
        }
    }

    fn clone(&self) -> Inode {
        Inode {
            mode: self.mode,
            size: self.size,
            permissions: self.permissions,
            created_at: self.created_at,
            updated_at: self.updated_at,
            accessed_at: self.accessed_at,
            serial_number: self.serial_number,
            data: match &self.data {
                InodeData::File(file) => InodeData::File(file.clone()),
                InodeData::Directory(directory) => InodeData::Directory(directory.clone()),
            },
        }
    }

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
        self.mode == FILE_MODE
    }

    pub fn is_directory(&self) -> bool {
        self.mode == DIR_MODE
    }

    pub fn add_inode(&mut self, inode: Inode) {
        if self.is_directory() {
            self.size += inode.size;
            match &mut self.data {
                InodeData::Directory(directory) => directory.add_inode(inode),
                _ => eprintln!("Error: trying to add a file to a non-directory inode"),
            }
        } else {
            // todo: handle error
            eprintln!("Error: trying to add a file to a non-directory inode");
        }
    }

    pub fn get_inode_by_name(&self, name: &str) -> Option<Inode> {
        if self.is_directory() {
            match &self.data {
                InodeData::Directory(directory) => {
                    for inode in &directory.files {
                        if inode.get_name() == name {
                            return Some(inode.clone());
                        }
                    }
                    None
                }
                _ => None,
            }
        } else {
            None
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
    files: Vec<Inode>,
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

    pub fn add_inode(&mut self, inode: Inode) {
        self.files.push(inode);
    }

    pub fn recursive_remove(&mut self) {
        let mut index = 0;
        for inode in self.files.clone() {
            match &inode.data {
                InodeData::Directory(child_directory) => {
                    child_directory.clone().recursive_remove();
                    self.files.remove(index);
                }
                InodeData::File(_) => {
                    self.files.remove(index);
                }
                _ => {}
            }
            index += 1;
        }
    }
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

    let mut options = vec![MountOption::FSName("VFFS".to_string())];

    fuser::mount2(MyFS::new(&mountpoint), mountpoint, &options).unwrap();
}
