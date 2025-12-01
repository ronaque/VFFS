#![allow(warnings)]

mod utils;

use crate::utils::{system_time_from_time, time_from_system_time, time_now};
use clap::{Arg, ArgAction, Command};
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use fuser::{MountOption, ReplyEntry, FUSE_ROOT_ID};
use libc::c_int;
use log::{debug, LevelFilter};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::mem::size_of;
use std::time::{Duration, SystemTime};

const DIR_MODE: u8 = 0;
const FILE_MODE: u8 = 1;

const BLOCK_SIZE: u32 = 512;

const FMODE_EXEC: i32 = 0x20;

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

const MAX_NODE_NAME_LENGTH: usize = 255; // Max file name length in bytes

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
    size: u64,
}

impl VFFS {
    fn new(mount: &String) -> VFFS {
        let root = Inode::new(DIR_MODE, mount.clone(), FUSE_ROOT_ID);
        let mut inodes = HashMap::new();
        inodes.insert(FUSE_ROOT_ID, root);
        VFFS { inodes, size: 0 }
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

    /// Append a new inode to the filesystem.
    /// The method adds the inode to the internal inode map,
    /// adding its size to the total filesystem size.
    fn append_inode(&mut self, inode: Inode) {
        self.size += inode.size;
        self.inodes.insert(inode.id, inode);
    }

    /// Remove an inode from the filesystem by its ID.
    /// The method subtracts the inode size from the total filesystem size
    /// and removes the inode from the internal inode map.
    fn remove_inode(&mut self, inode_id: u64) {
        if let Some(inode) = self.inodes.remove(&inode_id) {
            self.size -= inode.size;
        }
    }

    /// Write data to a file inode.
    /// The method validates the size of the data to be written against
    /// the maximum file size and available memory,
    /// and writes the data to the file's data buffer.
    /// If successful, it updates the total filesystem size.
    fn write_file_data(&mut self, inode_id: u64, data: &[u8]) -> Result<(), c_int> {
        let data_len = data.len() as u64;

        let size_diff: i64;

        {
            let inode = match self.lookup_node_mut(inode_id) {
                Ok(inode) => inode,
                Err(err) => return Err(err),
            };

            match &mut inode.data {
                InodeData::File(virtual_file) => {
                    let old_size = inode.size;
                    virtual_file.write_date(data);

                    let final_size = old_size + data_len;

                    if final_size > get_max_file_size() {
                        return Err(libc::EFBIG);
                    }

                    size_diff = final_size as i64 - old_size as i64;

                    inode.size = final_size;
                    inode.update_changes();
                }
                _ => return Err(libc::EISDIR),
            }
        }

        let new_total_size = (self.size as i64 + size_diff) as u64;

        if new_total_size > get_max_memory() {
            return Err(libc::ENOMEM);
        }

        self.size = new_total_size;

        Ok(())
    }

    fn validate_and_return_node_name(name: &OsStr) -> Result<String, c_int> {
        let name_str = name.to_str().unwrap();
        if name_str.len() > MAX_NODE_NAME_LENGTH {
            Err(libc::ENAMETOOLONG)
        } else {
            Ok(name_str.to_string())
        }
    }

    fn tree(&self) {
        let root_id = 1;

        match self.inodes.get(&root_id) {
            Some(inode) => {
                let root_name = match &inode.data {
                    InodeData::Directory(dir) => &dir.name,
                    InodeData::File(f) => &f.name,
                };
                println!("{}", root_name);

                self.print_recursive(root_id, "".to_string());
            }
            None => println!("Erro: Nó raiz (ID 1) não encontrado."),
        }
    }

    /// Função auxiliar recursiva
    fn print_recursive(&self, inode_id: u64, prefix: String) {
        let inode = match self.inodes.get(&inode_id) {
            Some(i) => i,
            None => return,
        };

        if let InodeData::Directory(directory) = &inode.data {
            let mut children = directory.nodes.clone();
            children.sort_by(|a, b| a.1.cmp(&b.1));

            let count = children.len();

            for (i, (child_id, child_name, _file_type)) in children.iter().enumerate() {
                let is_last = i == count - 1;
                let connector = if is_last { "└── " } else { "├── " };

                println!("{}{}{}", prefix, connector, child_name);

                if let Some(child_inode) = self.inodes.get(child_id) {
                    if let InodeData::Directory(_) = child_inode.data {
                        let child_prefix = if is_last { "    " } else { "│   " };
                        let new_prefix = format!("{}{}", prefix, child_prefix);

                        self.print_recursive(*child_id, new_prefix);
                    }
                }
            }
        }
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
        let name_str = match VFFS::validate_and_return_node_name(name) {
            Ok(name) => name,
            Err(err) => {
                reply.error(err);
                return;
            }
        };

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

        // println!(
        //     "Created inode {:?} for create with parent: {parent} and name: {:?}",
        //     new_inode_id,
        //     name.to_str()
        // );
        println!("Created file. Filesystem:");
        self.tree();
        reply.created(&Duration::new(0, 0), &file_attr, 0, 0, 0);
    }

    /// Get the attributes of a file or directory by its inode number.
    /// The method retrieves the attributes of the specified inode from the VFFS.
    ///
    /// The `ino` parameter is the inode number of the file or directory.
    fn getattr(&mut self, _req: &Request<'_>, ino: u64, fh: Option<u64>, reply: ReplyAttr) {
        // println!("getattr() called with ino: {ino}, fh: {fh:?}");
        match self.lookup_node(ino) {
            Ok(inode) => {
                // println!("Found inode {:?} for getattr with ino: {ino} ", inode);
                reply.attr(&Duration::new(0, 0), &inode.into())
            }
            Err(err) => reply.error(err),
        }
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
                let file_entry = match directory.find_node_by_name(name_str) {
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

                // println!(
                //     "Found inode {:?} for lookup with parent: {parent} and name: {name_str}",
                //     file_inode
                // );
                reply.entry(&Duration::new(0, 0), &file_inode.into(), 0)
            }
            Err(err) => reply.error(err),
        }
    }

    /// Create a new directory in the specified parent directory.
    /// The creation of the directory consists of allocating a new inode,
    /// adding it to the VFFS, and updating the parent directory structure
    fn mkdir(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mut mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        // println!(
        //     "mkdir() called with {parent:?} {name:?} {mode:o}"
        // );

        let name_str = match VFFS::validate_and_return_node_name(name) {
            Ok(name) => name,
            Err(err) => {
                reply.error(err);
                return;
            }
        };

        // Check if parent is a directory
        {
            if let Ok(parent_inode) = self.lookup_node(parent) {
                if let InodeData::Directory(dir) = &parent_inode.data {
                    if dir.find_node_by_name(&name_str).is_some() {
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
            size: 0,
            updated_at: time_now(),
            accessed_at: time_now(),
            metadata_change_at: time_now(),
            data: InodeData::Directory(Directory::new(name_str.clone())),
            mode: (mode & !umask) as u16,
            hardlinks: 1,
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

        // println!(
        //     "Created inode {:?} for mkdir with parent: {parent} and name: {:?}",
        //     new_inode_id,
        //     name.to_str()
        // );
        println!("Created directory. Filesystem:");
        self.tree();

        reply.entry(&Duration::new(0, 0), &attr_reply, 0);
    }

    fn open(&mut self, req: &Request, inode: u64, flags: i32, reply: ReplyOpen) {
        // debug!("open() function called for {inode:?}");

        let (_, _, _) = match flags & libc::O_ACCMODE {
            libc::O_RDONLY => {
                if flags & libc::O_TRUNC != 0 {
                    reply.error(libc::EACCES);
                    return;
                }
                if flags & FMODE_EXEC != 0 {
                    (libc::X_OK, true, false)
                } else {
                    (libc::R_OK, true, false)
                }
            }
            libc::O_WRONLY => (libc::W_OK, false, true),
            libc::O_RDWR => (libc::R_OK | libc::W_OK, true, true),

            _ => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        let fh = inode;
        reply.opened(fh, 0);
    }

    fn read(
        &mut self,
        _req: &Request,
        inode: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        // debug!("read() called on {inode:?} offset={offset:?} size={size:?}");
        assert!(offset >= 0);

        match self.lookup_node(inode) {
            Ok(node) => match &node.data {
                InodeData::File(virtual_file) => {
                    let data_bytes = virtual_file.data.as_bytes();
                    let offset = offset as usize;

                    if offset >= data_bytes.len() {
                        reply.data(&[]);
                        return;
                    }

                    let available = data_bytes.len() - offset;
                    let to_read = std::cmp::min(size as usize, available);

                    reply.data(&data_bytes[offset..offset + to_read]);
                }
                InodeData::Directory(_) => {
                    reply.error(libc::EISDIR);
                }
            },
            Err(error_code) => {
                reply.error(error_code);
            }
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
                        for (id, name, filetype) in &directory.nodes {
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
                        reply.error(libc::ENOTDIR);
                    }
                }
            }
            Err(err) => reply.error(err),
        }
    }

    /// Rename a file or directory.
    /// This method moves a file or directory from one location to another,
    /// optionally renaming it in the process.
    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        new_parent: u64,
        new_name: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        let name_str = name.to_str().unwrap().to_string();
        let new_name_string = match VFFS::validate_and_return_node_name(new_name) {
            Ok(name) => name,
            Err(err) => {
                reply.error(err);
                return;
            }
        };

        // Find source node in the parent directory
        let source_inode_id = {
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

            match &parent_inode.data {
                InodeData::Directory(dir) => match dir.find_node_by_name(&name_str) {
                    Some((id, _, _)) => id,
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                },
                _ => {
                    reply.error(libc::ENOTDIR);
                    return;
                }
            }
        };

        // Check if target node exists in the new parent directory
        let target_inode_id_opt = {
            let new_parent_inode = match self.lookup_node(new_parent) {
                Ok(inode) => inode,
                Err(err) => {
                    reply.error(err);
                    return;
                }
            };

            if !new_parent_inode.is_directory() {
                reply.error(libc::ENOTDIR);
                return;
            }

            match &new_parent_inode.data {
                InodeData::Directory(dir) => {
                    dir.find_node_by_name(&new_name_string).map(|(id, _, _)| id)
                }
                _ => {
                    reply.error(libc::ENOTDIR);
                    return;
                }
            }
        };

        // Handle target node if it exists
        if let Some(target_id) = target_inode_id_opt {
            if target_id == source_inode_id {
                reply.ok();
                return;
            }

            let target_inode = self.lookup_node(target_id).unwrap();

            if let InodeData::Directory(dir) = &target_inode.data {
                if !dir.nodes.is_empty() {
                    reply.error(libc::ENOTEMPTY);
                    return;
                }
            }
        }

        // Remove source from old parent and add to new parent
        if let Some(target_id) = target_inode_id_opt {
            self.remove_inode(target_id);

            if let Ok(new_p_inode) = self.lookup_node_mut(new_parent) {
                if let InodeData::Directory(dir) = &mut new_p_inode.data {
                    dir.nodes.retain(|(id, _, _)| *id != target_id);
                }
            }
        }

        let file_type_cache;
        // Update parent directory
        {
            let parent_inode = self
                .lookup_node_mut(parent)
                .expect("Parent checked in Phase 1");
            parent_inode.update_changes();

            if let InodeData::Directory(dir) = &mut parent_inode.data {
                let (_, _, f_type) = dir.find_node_by_name(&name_str).unwrap();
                file_type_cache = f_type;

                dir.nodes.retain(|(id, _, _)| *id != source_inode_id);
            } else {
                reply.error(libc::EIO);
                return;
            }
        }

        // Add to new parent directory
        {
            let new_parent_inode = self
                .lookup_node_mut(new_parent)
                .expect("New Parent checked in Phase 1");
            new_parent_inode.update_changes();

            if let InodeData::Directory(dir) = &mut new_parent_inode.data {
                dir.add_node((source_inode_id, new_name_string.clone(), file_type_cache));
            }
        }

        // Update source inode name
        {
            let inode = self
                .lookup_node_mut(source_inode_id)
                .expect("Source checked in Phase 1");
            inode.metadata_change_at = time_now();

            match &mut inode.data {
                InodeData::File(f) => f.name = new_name_string,
                InodeData::Directory(d) => d.name = new_name_string,
            }
        }

        println!("Renamed file/dir. Filesystem:");
        self.tree();

        reply.ok();
    }

    /// Remove a directory from the filesystem.
    /// The method locates the inode corresponding to the directory to be removed,
    /// checks if it is empty, removes the entry from the parent directory structure,
    /// and deletes the inode from the VFFS.
    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = name.to_str().unwrap();
        // println!("rmdir() called with parent: {parent}, name: {name_str}");

        // Find the inode to be removed matching it as a directory
        let inode_id = {
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

            let directory = match &parent_inode.data {
                InodeData::Directory(dir) => dir,
                _ => {
                    reply.error(libc::ENOTDIR);
                    return;
                }
            };

            match directory.find_node_by_name(name_str) {
                Some((id, _, _)) => id,
                None => {
                    reply.error(libc::ENOENT);
                    return;
                }
            }
        };

        // Check if the directory is empty
        match self.lookup_node(inode_id) {
            Ok(inode) => {
                if let InodeData::Directory(dir) = &inode.data {
                    if !dir.nodes.is_empty() {
                        reply.error(libc::ENOTEMPTY);
                        return;
                    }
                } else {
                    reply.error(libc::ENOTDIR);
                    return;
                }
            }
            Err(err) => {
                reply.error(err);
                return;
            }
        }

        // Remove the entry from the parent directory
        match self.lookup_node_mut(parent) {
            Ok(parent_inode) => {
                if let InodeData::Directory(dir) = &mut parent_inode.data {
                    dir.nodes.retain(|(_, n, _)| n != name_str);
                }
                parent_inode.update_changes();
            }
            Err(err) => {
                reply.error(err);
                return;
            }
        }

        // Remove the inode from the filesystem
        self.remove_inode(inode_id);

        println!("Removed directory. Filesystem:");
        self.tree();
        reply.ok();
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

        // Update the inode attributes in a local scope
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

        // Retrieve the updated inode to send in the reply
        let inode = match self.lookup_node(ino) {
            Ok(inode) => inode,
            Err(err) => {
                reply.error(err);
                return;
            }
        };
        // println!("Updated inode for setattr: {:?}", inode);
        reply.attr(&Duration::new(0, 0), &inode.into());
    }

    /// Removes a file from the filesystem.
    /// The method locates the inode corresponding to the file to be removed,
    /// removes the entry from the parent directory structure,
    /// and deletes the inode from the VFFS.
    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = name.to_str().unwrap();
        // println!("unlink() called with parent: {parent}, name: {name_str}");

        // Find the inode to be unlinked
        let inode_id = {
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

            let directory = match &parent_inode.data {
                InodeData::Directory(dir) => dir,
                _ => {
                    reply.error(libc::ENOTDIR);
                    return;
                }
            };

            match directory.find_node_by_name(name_str) {
                Some((id, _, _)) => id,
                None => {
                    reply.error(libc::ENOENT);
                    return;
                }
            }
        };

        // Remove the entry from the parent directory
        match self.lookup_node_mut(parent) {
            Ok(parent_inode) => {
                if let InodeData::Directory(dir) = &mut parent_inode.data {
                    dir.nodes.retain(|(_, n, _)| n != name_str);
                }
                parent_inode.update_changes();
            }
            Err(err) => {
                reply.error(err);
                return;
            }
        }

        // Remove the inode from the filesystem
        self.remove_inode(inode_id);

        println!("Removed file. Filesystem:");
        self.tree();
        reply.ok();
    }

    /// Write data to a file.
    /// This is done by locating the inode in the VFFS, matching it as a file,
    /// and writing the received data to the file's data buffer.
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

        match self.write_file_data(ino, data) {
            Ok(_) => {
                // println!("Wrote {} bytes to inode {}", data.len(), ino);
                reply.written(data.len() as u32);
            }
            Err(err) => {
                reply.error(err);
            }
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

    pub fn update_changes(&mut self) {
        let now = time_now();
        self.updated_at = now;
        self.metadata_change_at = now;
    }

    pub fn update_acess_time(&mut self) {
        let now = time_now();
        self.accessed_at = now;
    }

    pub fn append_file_to_directory(&mut self, file: (u64, String, FileType)) {
        match &mut self.data {
            InodeData::Directory(directory) => {
                directory.add_node(file);
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
    nodes: Vec<(u64, String, FileType)>,
}

impl Directory {
    pub fn new(name: String) -> Directory {
        Directory {
            name,
            nodes: Vec::new(),
        }
    }

    pub fn clone(&self) -> Directory {
        Directory {
            name: self.name.clone(),
            nodes: self.nodes.clone(),
        }
    }

    pub fn add_node(&mut self, inode: (u64, String, FileType)) {
        self.nodes.push(inode);
    }

    pub fn find_node_by_name(&self, name: &str) -> Option<(u64, String, FileType)> {
        for file in &self.nodes {
            if file.1 == name {
                return Some(file.clone());
            }
        }
        None
    }
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
