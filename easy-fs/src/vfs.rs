use super::{
    block_cache_sync_all, get_block_cache, BlockDevice, DirEntry, DiskInode, DiskInodeType,
    EasyFileSystem, DIRENT_SZ, BLOCK_SZ
};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};
/// Virtual filesystem layer over easy-fs
pub struct Inode {
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
}

impl Inode {
    /// Create a vfs inode
    pub fn new(
        block_id: u32,
        block_offset: usize,
        fs: Arc<Mutex<EasyFileSystem>>,
        block_device: Arc<dyn BlockDevice>,
    ) -> Self {
        Self {
            block_id: block_id as usize,
            block_offset,
            fs,
            block_device,
        }
    }
    /// Call a function over a disk inode to read it
    fn read_disk_inode<V>(&self, f: impl FnOnce(&DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .read(self.block_offset, f)
    }
    /// Call a function over a disk inode to modify it
    fn modify_disk_inode<V>(&self, f: impl FnOnce(&mut DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .modify(self.block_offset, f)
    }
    /// Find inode under a disk inode by name
    pub fn find_inode_id(&self, name: &str, disk_inode: &DiskInode) -> Option<u32> {
        // assert it is a directory
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut dirent: DirEntry = DirEntry::empty();
        for i in 0..file_count {
            assert_eq!(
                disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                DIRENT_SZ,
            );
            if dirent.name() == name {
                return Some(dirent.inode_id() as u32);
            }
        }
        None
    }
    /// Find inode under current inode by name
    pub fn find(&self, name: &str) -> Option<Arc<Inode>> {
        let fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            self.find_inode_id(name, disk_inode).map(|inode_id| {
                let (block_id, block_offset) = fs.get_disk_inode_pos(inode_id);
                Arc::new(Self::new(
                    block_id,
                    block_offset,
                    self.fs.clone(),
                    self.block_device.clone(),
                ))
            })
        })
    }
    /// Increase the size of a disk inode
    fn increase_size(
        &self,
        new_size: u32,
        disk_inode: &mut DiskInode,
        fs: &mut MutexGuard<EasyFileSystem>,
    ) {
        if new_size < disk_inode.size {
            return;
        }
        let blocks_needed = disk_inode.blocks_num_needed(new_size);
        let mut v: Vec<u32> = Vec::new();
        for _ in 0..blocks_needed {
            v.push(fs.alloc_data());
        }
        disk_inode.increase_size(new_size, v, &self.block_device);
    }
    /// Create inode under current inode by name
    pub fn create(&self, name: &str) -> Option<Arc<Inode>> {
        let mut fs = self.fs.lock();
        let op = |root_inode: &DiskInode| {
            // assert it is a directory
            assert!(root_inode.is_dir());
            // has the file been created?
            self.find_inode_id(name, root_inode)
        };
        if self.read_disk_inode(op).is_some() {
            return None;
        }
        // create a new file
        // alloc a inode with an indirect block
        let new_inode_id = fs.alloc_inode();
        // initialize inode
        let (new_inode_block_id, new_inode_block_offset) = fs.get_disk_inode_pos(new_inode_id);
        get_block_cache(new_inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(new_inode_block_offset, |new_inode: &mut DiskInode| {
                new_inode.initialize(DiskInodeType::File);
            });
        self.modify_disk_inode(|root_inode| {
            // append file in the dirent
            let file_count = (root_inode.size as usize) / DIRENT_SZ;
            let new_size = (file_count + 1) * DIRENT_SZ;
            // increase size
            self.increase_size(new_size as u32, root_inode, &mut fs);
            // write dirent
            let dirent = DirEntry::new(name, new_inode_id);
            root_inode.write_at(
                file_count * DIRENT_SZ,
                dirent.as_bytes(),
                &self.block_device,
            );
        });

        let (block_id, block_offset) = fs.get_disk_inode_pos(new_inode_id);
        block_cache_sync_all();
        // return inode
        Some(Arc::new(Self::new(
            block_id,
            block_offset,
            self.fs.clone(),
            self.block_device.clone(),
        )))
        // release efs lock automatically by compiler
    }
    /// create a new hard link to current inode
    pub fn linkat(&self, name: &str, old_inode: &Inode) -> Option<Arc<Inode>> {
        let inode_id = old_inode.get_inode_id_by_block_id_and_offset() as u32;
        let dirent = DirEntry::new(name, inode_id);
        let mut fs = self.fs.lock();

        // update self(root_inode)
        self.modify_disk_inode(|root_inode| {
            // append file in the dirent
            let file_count = (root_inode.size as usize) / DIRENT_SZ;
            let new_size = (file_count + 1) * DIRENT_SZ;
            // increase size
            self.increase_size(new_size as u32, root_inode, &mut fs);
            // write dirent
            root_inode.write_at(
                file_count * DIRENT_SZ,
                dirent.as_bytes(),
                &self.block_device,
            );
        });

        let (block_id, block_offset) = fs.get_disk_inode_pos(
            inode_id
        );
        // sync all block caches
        block_cache_sync_all();
        // return inode
        Some(Arc::new(Self::new(
            block_id,
            block_offset,
            self.fs.clone(),
            self.block_device.clone(),
        )))
    }
    /// unlink a hard link
    pub fn unlinkat(&self, name: &str) {
        let (inode_id, mut root_disk_inode) = self.read_disk_inode(|disk_inode| {
            // assert it is a directory
            assert!(disk_inode.is_dir());
            // find the inode id
            (self.find_inode_id(name, disk_inode).unwrap(), disk_inode.clone())
        });

        let hardlink_count = self.get_hardlink_count_by_id(inode_id as usize).1;
        let (block_id, block_offset) = {
            let fs = self.fs.lock();
            fs.get_disk_inode_pos(inode_id)
        };

        let mut fs = self.fs.lock();

        if hardlink_count == 1 {
            get_block_cache(block_id as usize, Arc::clone(&self.block_device))
                .lock()
                .modify(block_offset, |disk_inode: &mut DiskInode| {
                    // dealloc data blocks
                    let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
                    for data_block in data_blocks_dealloc.into_iter() {
                        fs.dealloc_data(data_block);
                    }

                });

            // dealloc the inode
            fs.dealloc_inode(inode_id);
        }

        // remove the dirent
        self.remove_dirent(name, &mut root_disk_inode);
        
        block_cache_sync_all();
    }
    /// Remove a dirent from current inode
    pub fn remove_dirent(&self, name: &str, disk_inode: &mut DiskInode) {
        // assert it is a directory
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut dirent = DirEntry::empty();
        let mut found = false;
        for i in 0..file_count {
            assert_eq!(
                disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                DIRENT_SZ,
            );
            if dirent.name() == name {
                found = true;
                // shift the rest of the dirents
                for j in i..file_count - 1 {
                    // 直接用后一个dirent覆盖当前dirent
                    let next_dirent = &mut DirEntry::empty();
                    assert_eq!(
                        disk_inode.read_at(DIRENT_SZ * (j + 1), next_dirent.as_bytes_mut(), &self.block_device,),
                        DIRENT_SZ,
                    );
                    disk_inode.write_at(DIRENT_SZ * j, next_dirent.as_bytes(), &self.block_device);
                }
                break;
            }
        }
        // clear the last dirent
        let last_dirent = &mut DirEntry::empty();
        assert_eq!(
            disk_inode.write_at(DIRENT_SZ * (file_count - 1), last_dirent.as_bytes_mut(), &self.block_device,),
            DIRENT_SZ,
        );

        assert!(found, "Dirent {} not found in inode", name);
        // decrease size
        let new_size = (file_count - 1) * DIRENT_SZ;
        disk_inode.increase_size(new_size as u32, Vec::new(), &self.block_device);
    }
    /// List inodes under current inode
    pub fn ls(&self) -> Vec<String> {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            let mut v: Vec<String> = Vec::new();
            for i in 0..file_count {
                let mut dirent = DirEntry::empty();
                assert_eq!(
                    disk_inode.read_at(i * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                v.push(String::from(dirent.name()));
            }
            v
        })
    }
    /// Read data from current inode
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.read_at(offset, buf, &self.block_device))
    }
    /// Write data to current inode
    pub fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut fs = self.fs.lock();
        let size = self.modify_disk_inode(|disk_inode| {
            self.increase_size((offset + buf.len()) as u32, disk_inode, &mut fs);
            disk_inode.write_at(offset, buf, &self.block_device)
        });
        block_cache_sync_all();
        size
    }
    /// Clear the data in current inode
    pub fn clear(&self) {
        let mut fs = self.fs.lock();
        self.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;
            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });
        block_cache_sync_all();
    }
    /// get hardlink count by inode id
    pub fn get_hardlink_count_by_id(&self, id: usize) -> (Vec<String>, u32) {
        let names = self.ls();
        let inode_with_id = {
            let fs = self.fs.lock();
            let (block_id, block_offset) = fs.get_disk_inode_pos(id as u32);
            Arc::new(Self::new(
                block_id,
                block_offset,
                self.fs.clone(),
                self.block_device.clone(),
            ))
        };
        
        let disk_inode = self.read_disk_inode(|disk_inode| {
            disk_inode.clone()
        });

        // assert it is a directory
        assert!(disk_inode.is_dir());
        // find the inode id
        let mut hardlink_count = 0;
        let mut matched_names: Vec<String> = Vec::new();
        for dirent_name in names {
            if let Some(inode_id) = self.find_inode_id(&dirent_name, &disk_inode) {
                if inode_with_id.has_same_inode(inode_id) {
                    // if the inode with id has the same data area as current inode  
                    matched_names.push(dirent_name);
                    hardlink_count += 1;
                }
            }
        }
        (matched_names.clone(), hardlink_count)
    }
    /// get inode id by block id and offset
    pub fn get_inode_id_by_block_id_and_offset(&self) -> usize {
        let fs = self.fs.lock();
        let inode_size = core::mem::size_of::<DiskInode>();
        let inodes_per_block = BLOCK_SZ / inode_size;
        
        let inode_blocks = self.block_id - fs.inode_area_start_block as usize;
        inode_blocks * inodes_per_block + (self.block_offset / inode_size) as usize
    }
    /// is dir?
    pub fn is_dir(&self) -> bool {
        self.read_disk_inode(|disk_inode| disk_inode.is_dir())
    }
    /// is file?
    pub fn is_file(&self) -> bool {
        self.read_disk_inode(|disk_inode| disk_inode.is_file())
    }
    /// if the two inode points to the same data_area_blocks
    pub fn has_same_inode(&self, inode_id: u32) -> bool {
        inode_id == self.get_inode_id_by_block_id_and_offset() as u32
    }
}
