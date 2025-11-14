//! Process management syscalls (ch5-compatible)

use alloc::sync::Arc;
use log::trace;

use crate::{
    loader::get_app_data_by_name,
    mm::{translated_refmut, translated_str, MapPermission, VirtAddr, VPNRange},
    task::{ add_task, current_task, current_user_token, exit_current_and_run_next, suspend_current_and_run_next },
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    let pid = current_task().unwrap().pid.0;
    trace!("kernel:pid[{}] sys_exit", pid);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    trace!("kernel:pid[{}] sys_yield", current_task().unwrap().pid.0);
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    let pid = current_task().unwrap().pid.0;
    trace!("kernel: sys_getpid pid:{}", pid);
    pid as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.getpid();
    // child returns 0
    {
        let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
        trap_cx.x[10] = 0;
    }
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path_ptr: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path_ptr);
    if let Some(data) = get_app_data_by_name(path.as_str()) {
        let task = current_task().unwrap();
        task.exec(data);
        0
    } else {
        -1
    }
}

/// waitpid
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    trace!("kernel::pid[{}] sys_waitpid [{}]", current_task().unwrap().pid.0, pid);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();

    if !inner.children.iter().any(|p| pid == -1 || pid as usize == p.getpid()) {
        return -1;
    }

    // find a child that is zombie
    if let Some((idx, _)) = inner.children.iter().enumerate().find(|(_, p)| {
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
    }) {
        let child = inner.children.remove(idx);
        assert_eq!(alloc::sync::Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        let exit_code = child.inner_exclusive_access().exit_code;
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
}

/// sys_get_time: translate user pointer (TimeVal) and write
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    let token = current_user_token();
    let time_us = crate::timer::get_time();

    // translated_refmut 直接返回 &mut TimeVal —— 不会返回 Option
    let tv: &mut TimeVal = translated_refmut(token, ts);

    tv.sec = time_us / 1_000_000;
    tv.usec = time_us % 1_000_000;

    0
}


/// mmap: anonymous mapping
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    let task = current_task().unwrap();
    trace!("kernel:pid[{}] sys_mmap start={:#x} len={} prot={:#x}", task.pid.0, start, len, prot);

    const PAGE_SIZE: usize = crate::config::PAGE_SIZE;
    if start % PAGE_SIZE != 0 { return -1; }
    if prot & !0x7 != 0 || prot & 0x7 == 0 { return -1; }

    let mut perm = MapPermission::U;
    if prot & 0x1 != 0 { perm |= MapPermission::R; }
    if prot & 0x2 != 0 { perm |= MapPermission::W; }
    if prot & 0x4 != 0 { perm |= MapPermission::X; }

    let len_aligned = (len + PAGE_SIZE - 1) / PAGE_SIZE * PAGE_SIZE;
    let mut inner = task.inner_exclusive_access();
    let vpn_range = VPNRange::new(
        VirtAddr::from(start).floor(),
        VirtAddr::from(start + len_aligned).ceil(),
    );

    if inner.memory_set.is_overlapped(&vpn_range) { return -1; }

    if inner.memory_set.insert_framed_area_with_result(start.into(), (start + len_aligned).into(), perm).is_err() {
        return -1;
    }
    0
}

/// munmap
pub fn sys_munmap(start: usize, len: usize) -> isize {
    let task = current_task().unwrap();
    trace!("kernel:pid[{}] sys_munmap start={:#x} len={}", task.pid.0, start, len);

    const PAGE_SIZE: usize = crate::config::PAGE_SIZE;
    if start % PAGE_SIZE != 0 { return -1; }
    let len_aligned = (len + PAGE_SIZE - 1) / PAGE_SIZE * PAGE_SIZE;

    let mut inner = task.inner_exclusive_access();
    let vpn_range = VPNRange::new(
        VirtAddr::from(start).floor(),
        VirtAddr::from(start + len_aligned).ceil(),
    );

    if !inner.memory_set.is_fully_mapped(&vpn_range) { return -1; }

    inner.memory_set.remove_area(&vpn_range);
    0
}

/// sbrk (change brk)
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// spawn(path) — create new process that runs program `path`
pub fn sys_spawn(path_ptr: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_spawn", current_task().unwrap().pid.0);

    let token = current_user_token();
    let path = translated_str(token, path_ptr); // ch5: returns String
    let elf = match get_app_data_by_name(path.as_str()) {
        Some(d) => d,
        None => return -1,
    };

    let child = Arc::new(crate::task::TaskControlBlock::new(elf));

    // set parent <-> child links
    if let Some(parent) = current_task() {
        {
            let mut child_inner = child.inner_exclusive_access();
            child_inner.parent = Some(alloc::sync::Arc::downgrade(&parent));
        }
        {
            let mut parent_inner = parent.inner_exclusive_access();
            parent_inner.children.push(child.clone());
        }
    }

    add_task(child.clone());
    child.getpid() as isize
}

/// set_priority(prio)
pub fn sys_set_priority(prio: isize) -> isize {
    trace!("kernel:pid[{}] sys_set_priority -> {}", current_task().unwrap().pid.0, prio);
    if prio < 2 { return -1; }
    let pr = prio as usize;
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();

    // NOTE: you must add `priority: usize` and `pass: usize` fields to TaskControlBlockInner
    inner.priority = pr;
    // BIG_STRIDE needs to be a large const defined somewhere in scheduler module
    inner.pass = crate::task::BIG_STRIDE / pr;

    pr as isize
}
