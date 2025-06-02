//! File and filesystem-related syscalls
use crate::fs::{open_file, OpenFlags, Stat};
use crate::mm::{translated_byte_buffer, translated_str, UserBuffer, translated_refmut};
use crate::task::{current_task, current_user_token};
use crate::fs::{OSInode, ROOT_INODE};

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_write", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_read", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        trace!("kernel: sys_read .. file.read");
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_open(path: *const u8, flags: u32) -> isize {
    trace!("kernel:pid[{}] sys_open", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(inode) = open_file(path.as_str(), OpenFlags::from_bits(flags).unwrap()) {
        let mut inner = task.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    trace!("kernel:pid[{}] sys_close", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    0
}

/// YOUR JOB: Implement fstat.
pub fn sys_fstat(_fd: usize, _st: *mut Stat) -> isize {
    let current_user_token = {current_user_token()};
    let current_task = current_task().unwrap();
    let fd_table = &current_task.inner_exclusive_access().fd_table;
    let fd = fd_table.get(_fd).unwrap().as_ref()
                            .unwrap().as_any().downcast_ref::<OSInode>().unwrap();
    let st = translated_refmut(current_user_token, _st);

    let mode = fd.get_mode();
    let ino = fd.get_inode_id();
    let (names, nlink) = fd.get_nlink();
    for name in names {
        println!("{}", name);
    }
    *st = Stat {
        dev: 0,
        ino,
        mode,
        nlink,
        pad: [0; 7],
    };
    0
}

/// YOUR JOB: Implement linkat.
pub fn sys_linkat(_old_name: *const u8, _new_name: *const u8) -> isize {
    let current_task = current_task().unwrap();
    let current_user_token = current_user_token();
    let old_path = translated_str(current_user_token, _old_name);
    let new_path = translated_str(current_user_token, _new_name);
    if old_path == new_path {
        trace!(
            "kernel:pid[{}] sys_linkat failed: old_name and new_name are the same",
            current_task.pid.0
        );
        return -1;
    }

    // get the inode of the file.
    let old_inode = open_file(old_path.as_str(), OpenFlags::RDONLY).unwrap();
    // create new inode for the new_name
    ROOT_INODE.linkat(new_path.as_str(), &old_inode.inner.exclusive_access().inode);
    let names = ROOT_INODE.ls();
    for name in names {
        let inode_id = ROOT_INODE.find(name.as_str()).unwrap().get_inode_id_by_block_id_and_offset();
        println!("inode_id: {}, name: {}", inode_id, name);
    }
    0
}

/// YOUR JOB: Implement unlinkat.
pub fn sys_unlinkat(_name: *const u8) -> isize {
    let current_task = current_task().unwrap();
    let name = translated_str(current_user_token(), _name);

    if ROOT_INODE.find(name.as_str()).is_none() {
        trace!(
            "kernel:pid[{}] sys_unlinkat failed: file not found",
            current_task.pid.0
        );
        return -1;
    }

    ROOT_INODE.unlinkat(name.as_str());
    0
}
