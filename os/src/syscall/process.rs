//! Process management syscalls
//!
use alloc::sync::Arc;

use crate::{
    fs::{open_file, OpenFlags},
    mm::{translated_refmut, translated_str},
    mm::{VirtAddr, frame_alloc, PTEFlags,
        PageTable, VirtPageNum},
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next, mmap_current_task, munmap_current_task
    },
    config::{PAGE_SIZE, PAGE_SIZE_BITS, TRAMPOLINE},
};

use core::mem::size_of;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    //trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap();
        task.exec(all_data.as_slice());
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    //trace!("kernel: sys_waitpid");
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
        trace!("kernel: sys_get_time");
    let us = crate::timer::get_time_us();
    let first_page = VirtAddr::from(_ts as usize).floor(); 
    let last_page = VirtAddr::from(_ts as usize + size_of::<TimeVal>()).floor();
    let current_user_pt = &mut PageTable::from_token(current_user_token());
    let flags = PTEFlags::U | PTEFlags::R | PTEFlags::W;
    let offset = VirtAddr::from(_ts as usize).page_offset();

    //if page not found, map the page first.
    let first_page_pte = current_user_pt.find_pte_create(first_page).unwrap();
    if !first_page_pte.is_valid() {
        let frame = frame_alloc().unwrap();
        current_user_pt.map(first_page, frame.ppn, flags);
    }

    let last_page_pte = current_user_pt.find_pte_create(last_page).unwrap();
    if !last_page_pte.is_valid() {
        let frame = frame_alloc().unwrap();
        current_user_pt.map(last_page, frame.ppn, flags);
    }

    let first_page_pte = current_user_pt.translate(first_page).unwrap();
    // 恒等映射！直接获取物理地址即可
    let ts = ((first_page_pte.ppn().0 << PAGE_SIZE_BITS) + offset) as *mut TimeVal;
    unsafe {
        *ts = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    };

    0
}

/// YOUR JOB: Implement mmap.
pub fn sys_mmap(_start: usize, _len: usize, _port: usize) -> isize {
        let current_pt = &mut PageTable::from_token(current_user_token());

    if _start % PAGE_SIZE != 0 {
        trace!("kernel: sys_mmap start address is not page aligned");
        return -1;
    }
    if (_port & !0x7) != 0 {
        trace!("kernel: sys_mmap invalid prot flags");
        return -1;
    }
    if (_port & 0x7) == 0 {
        trace!("kernel: meaningless prot flags");
        return -1;
    }
    
    let start_va = VirtAddr::from(_start);
    let end_va = VirtAddr::from(_start + _len);
    let start_vpn = start_va.floor();
    let end_vpn = end_va.ceil();

    // choose data randomly(choose page_table here)
    let data: Option<&[u8]> = Some(
        unsafe {
            core::slice::from_raw_parts(
                TRAMPOLINE as *const u8,
                _len,
            )
        }
    );

    for vpn in start_vpn.0..end_vpn.0 {
        if current_pt.translate(VirtPageNum::from(vpn)).is_some() {
            trace!("kernel: sys_mmap address already mapped");
            return -1;
        }
    }
    
    let flags = PTEFlags::from_bits((_port << 1) as u8).unwrap() | PTEFlags::U;

    mmap_current_task(start_va, end_va, flags.bits() as usize, data);
    
    0
}

/// YOUR JOB: Implement munmap.
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
        let current_pt = PageTable::from_token(current_user_token());

    let start_va = VirtAddr::from(_start);
    let end_va = VirtAddr::from(_start + _len);
    let start_vpn = start_va.floor();
    let end_vpn = end_va.ceil();

    for vpn in start_vpn.0..end_vpn.0 {
        if current_pt.translate(VirtPageNum::from(vpn)).is_none() {
            trace!("kernel: sys_munmap address not mapped");
            return -1;
        }
    }

    munmap_current_task(start_va, end_va)
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// YOUR JOB: Implement spawn.
/// HINT: fork + exec =/= spawn
pub fn sys_spawn(_path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_spawn", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let path = translated_str(current_user_token(), _path);
    let elf_data = open_file(path.as_str(), OpenFlags::RDONLY);

    if elf_data.is_some() {
        current_task.spawn(path)
    } else {
        trace!("kernel:pid[{}] sys_spawn failed to get app data: not such app!", current_task.pid.0);
        -1
    }
}

// YOUR JOB: Set task priority.
pub fn sys_set_priority(_prio: isize) -> isize {
    if _prio < 2 {
        trace!("kernel:pid[{}] sys_set_priority failed: priority must be greater than 1", current_task().unwrap().pid.0);
        return -1;
    }
    let current_task = current_task().unwrap();
    let priority = &mut current_task.inner_exclusive_access().priority;
    *priority = _prio;
    _prio
}