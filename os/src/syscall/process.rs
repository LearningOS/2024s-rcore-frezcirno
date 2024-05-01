//! Process management syscalls
use crate::{
    config::MAX_SYSCALL_NUM,
    task::{
        change_program_brk, copy_to_user, exit_current_and_run_next, get_current_task, mmap,
        munmap, suspend_current_and_run_next, TaskStatus,
    },
    timer::get_time_ms,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// Task information
#[allow(dead_code)]
pub struct TaskInfo {
    /// Task status in it's life cycle
    status: TaskStatus,
    /// The numbers of syscall called by task
    syscall_times: [u32; MAX_SYSCALL_NUM],
    /// Total running time of task
    time: usize,
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
    let time_ms = get_time_ms();
    let tv = TimeVal {
        sec: time_ms / 1000,
        usec: (time_ms % 1000) * 1000,
    };
    unsafe {
        copy_to_user(
            _ts as usize,
            core::slice::from_raw_parts(
                &tv as *const _ as *const u8,
                core::mem::size_of::<TimeVal>(),
            ),
        );
    }
    0
}

/// YOUR JOB: Finish sys_task_info to pass testcases
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TaskInfo`] is splitted by two pages ?
pub fn sys_task_info(_ti: *mut TaskInfo) -> isize {
    trace!("kernel: sys_task_info");
    let current_info = get_current_task();
    let task_info = TaskInfo {
        status: current_info.0,
        syscall_times: current_info.1,
        time: get_time_ms() - current_info.2,
    };
    unsafe {
        copy_to_user(
            _ti as usize,
            core::slice::from_raw_parts(
                &task_info as *const _ as *const u8,
                core::mem::size_of::<TaskInfo>(),
            ),
        );
    }
    0
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(_start: usize, _len: usize, _port: usize) -> isize {
    trace!("kernel: sys_mmap");

    // start 没有按页大小对齐
    if _start & 0xfff != 0 {
        return -1;
    }

    // port & !0x7 != 0 (port 其余位必须为0)
    if _port & !0x7 != 0 {
        return -1;
    }

    // port & 0x7 = 0 (这样的内存无意义)
    if _port & 0x7 == 0 {
        return -1;
    }

    // [start, start + len) 中存在已经被映射的页
    // 物理内存不足
    mmap(_start, _len, _port)
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
    trace!("kernel: sys_munmap");

    // start 没有按页大小对齐
    if _start & 0xfff != 0 {
        return -1;
    }

    // [start, start + len) 中存在未被映射的页
    munmap(_start, _len)
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
