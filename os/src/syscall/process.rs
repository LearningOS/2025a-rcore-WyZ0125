//! Process management syscalls
use crate::{
    task::{exit_current_and_run_next, suspend_current_and_run_next},
    timer::get_time_us,
    task::TASK_MANAGER,
    config::MAX_SYSCALL_NUM,
};
use core::ptr;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("[kernel] Application exited with code {}", exit_code);
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
    let us = get_time_us();
    unsafe {
        *ts = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    }
    0
}

// TODO: implement the syscall
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
     match trace_request {
        // 功能 0：读取用户地址 id 处的一个字节
        0 => {
            let addr = id as *const u8;
            unsafe { ptr::read_volatile(addr) as isize } // 直接读取用户内存（无检查）
        }
        // 功能 1：向用户地址 id 处写入 data 的低 1 字节
        1 => {
            let addr = id as *mut u8;
            let value = data as u8;
            unsafe { ptr::write_volatile(addr, value) }; // 直接写入用户内存（无检查）
            0
        }
        // 功能 2：查询当前任务的 syscall id 调用次数
        2 => {
            if id >= MAX_SYSCALL_NUM {
                -1
            } else {
                TASK_MANAGER.get_current_syscall_count(id).unwrap_or(0) as isize
            }
        }
        // 其他请求返回 -1
        _ => -1,
    }
}
