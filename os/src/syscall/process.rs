//! Process management syscalls
use crate::mm::{PageTable, PTEFlags};
use crate::mm::{VirtAddr, VirtPageNum, frame_alloc};
use crate::task::{mmap_current_task, munmap_current_task, change_program_brk, exit_current_and_run_next, suspend_current_and_run_next, get_syscall_num,
                 current_user_token};
use core::ptr::{read_volatile, write_volatile};
use core::mem::size_of;
use core::option::Option::{Some};
use crate::config::{PAGE_SIZE, TRAMPOLINE, PAGE_SIZE_BITS};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
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

/// TODO: Finish sys_trace to pass testcases
/// HINT: You might reimplement it with virtual memory management.
pub fn sys_trace(_trace_request: usize, _id: usize, _data: usize) -> isize {
    trace!("kernel: sys_trace");
    let current_pt = &mut PageTable::from_token(current_user_token());
    let id_va = VirtAddr::from(_id);
    let id_vpn = id_va.floor();
    let offset = id_va.page_offset();

    let flags = match _trace_request {
        0 => PTEFlags::U | PTEFlags::R,
        1 => PTEFlags::U | PTEFlags::W,
        _ => PTEFlags::from_bits(0).unwrap(),
    };

    let valid = {
        let id_pte = current_pt.find_pte_create(id_vpn).unwrap();
        id_pte.is_valid()
    };

    if !valid {
        let frame = frame_alloc().unwrap();
        current_pt.map(id_vpn, frame.ppn, flags);
    }

    // 恒等映射！
    let id_pte = current_pt.find_pte_create(id_vpn).unwrap();
    let id_kernel_va = (id_pte.ppn().0 << PAGE_SIZE_BITS) + offset;
    match _trace_request {
        0 => {
            if id_pte.user_accessible() 
                && id_pte.readable() {
                unsafe { 
                    read_volatile(id_kernel_va as *const u8) as isize
                }
            } else {
                trace!(
                    "kernel: sys_trace read id={:#x} failed, not accessible",
                    _id
                );
                -1
            }
        }
        1 => {
            // write data to the address pointed by _i
            if id_pte.user_accessible() 
                && id_pte.writable() {
                unsafe {
                    write_volatile(id_kernel_va as *mut u8, _data as u8);
                }
                0
            } else {
                trace!(
                    "kernel: sys_trace write id={:#x} failed, not accessible",
                    _id
                );
                -1
            }
        }
        2 => {
            get_syscall_num(_id) as isize
        }
        _ => {
            trace!("Invalid sys_trace_request: {}", _trace_request);
            -1
        }
    }
}

// YOUR JOB: Implement mmap.
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
        if current_pt.find_pte(VirtPageNum::from(vpn)).is_some() {
            trace!("kernel: sys_mmap address already mapped");
            return -1;
        }
    }
    
    let flags = PTEFlags::from_bits((_port << 1) as u8).unwrap() | PTEFlags::U;

    mmap_current_task(start_va, end_va, flags.bits() as usize, data);
    
    0
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
    let current_pt = PageTable::from_token(current_user_token());

    let start_va = VirtAddr::from(_start);
    let end_va = VirtAddr::from(_start + _len);
    let start_vpn = start_va.floor();
    let end_vpn = end_va.ceil();

    for vpn in start_vpn.0..end_vpn.0 {
        if current_pt.find_pte(VirtPageNum::from(vpn)).is_none() {
            trace!("kernel: sys_munmap address not mapped");
            return -1;
        }
    }

    munmap_current_task(start_va, end_va)
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
