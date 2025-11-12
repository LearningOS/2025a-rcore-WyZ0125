//! Process management syscalls
use crate::task::{
    change_program_brk, current_user_token, exit_current_and_run_next, suspend_current_and_run_next,
};
use crate::mm::{
    {translated_mut_slice, translated_slice},
    MapPermission, VirtAddr, VPNRange,
};
use crate::config::PAGE_SIZE;
use crate::timer::get_time;
use log::trace;

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

/// get time with second and microsecond
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    // 获取当前任务的 token，用于地址转换
    let token = current_user_token();
    let time_us = get_time();

    // 转换用户空间指针到内核可访问区域
    if let Some(mut slices) =
        translated_mut_slice(token, ts as usize, core::mem::size_of::<TimeVal>())
    {
        let tv_ptr = slices[0].as_mut_ptr() as *mut TimeVal;
        unsafe {
            (*tv_ptr).sec = time_us / 1_000_000;
            (*tv_ptr).usec = time_us % 1_000_000;
        }
        0
    } else {
        // 地址不可写
        -1
    }
}

/// sys_trace: read/write trace flag for current task
///
/// trace_request = 0 -> read kernel -> user
/// trace_request = 1 -> write user -> kernel
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    let token = current_user_token();

    match trace_request {
        // 读取 trace 标志
        0 => {
            // kernel -> user
            let flag_val: u8 = if id == 0 { 0 } else { 1 }; // 仅作演示标志位
            if let Some(mut slices) = translated_mut_slice(token, data, 1) {
                slices[0][0] = flag_val;
                0
            } else {
                // 地址不可写
                -1
            }
        }
        // 写入 trace 标志
        1 => {
            if let Some(slices) = translated_slice(token, data, 1) {
                let val = slices[0][0];
                trace!("trace flag set to {}", val);
                0
            } else {
                // 地址不可读
                -1
            }
        }
        _ => -1,
    }
}

/// mmap anonymous mapping
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    trace!(
        "kernel: sys_mmap start={:#x}, len={}, prot={:#x}",
        start,
        len,
        prot
    );

    // ---- 参数合法性检查 ----
    if start % PAGE_SIZE != 0 {
        return -1;
    }
    if prot & !0x7 != 0 || prot & 0x7 == 0 {
        return -1;
    }

    // ---- 权限转换 prot → MapPermission ----
    let mut perm = MapPermission::U;
    if prot & 0x1 != 0 {
        perm |= MapPermission::R;
    }
    if prot & 0x2 != 0 {
        perm |= MapPermission::W;
    }
    if prot & 0x4 != 0 {
        perm |= MapPermission::X;
    }

    crate::task::TASK_MANAGER.access_current_task(|task| {
        // ✅ 直接访问 task.memory_set，而不是 inner_exclusive_access()
        let len_aligned = ((len + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
        let vpn_range = VPNRange::new(
            VirtAddr::from(start).floor(),
            VirtAddr::from(start + len_aligned).ceil(),
        );

        // 检查是否重叠
        if task.memory_set.is_overlapped(&vpn_range) {
            return -1;
        }

        // 插入新的映射区域
        if task
            .memory_set
            .insert_framed_area_with_result(start.into(), (start + len_aligned).into(), perm)
            .is_err()
        {
            return -1;
        }

        0
    })
}

/// munmap unmap anonymous mapping
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap start={:#x}, len={}", start, len);

    if start % PAGE_SIZE != 0 {
        return -1;
    }

    let len_aligned = ((len + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;

    crate::task::TASK_MANAGER.access_current_task(|task| {
        let vpn_range = VPNRange::new(
            VirtAddr::from(start).floor(),
            VirtAddr::from(start + len_aligned).ceil(),
        );

        if !task.memory_set.is_fully_mapped(&vpn_range) {
            return -1;
        }

        task.memory_set.remove_area(&vpn_range);
        0
    })
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