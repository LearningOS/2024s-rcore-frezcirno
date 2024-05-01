//!Implementation of [`Processor`] and Intersection of control flow
//!
//! Here, the continuous operation of user apps in CPU is maintained,
//! the current running state of CPU is recorded,
//! and the replacement and transfer of control flow of different applications are executed.

use super::__switch;
use super::{fetch_task, TaskStatus};
use super::{TaskContext, TaskControlBlock};
use crate::config::{MAX_SYSCALL_NUM, PAGE_SIZE};
use crate::mm::VirtAddr;
use crate::sync::UPSafeCell;
use crate::timer::get_time_ms;
use crate::trap::TrapContext;
use alloc::sync::Arc;
use lazy_static::*;

/// Processor management structure
pub struct Processor {
    ///The task currently executing on the current processor
    current: Option<Arc<TaskControlBlock>>,

    ///The basic control flow of each core, helping to select and switch process
    idle_task_cx: TaskContext,
}

impl Processor {
    ///Create an empty Processor
    pub fn new() -> Self {
        Self {
            current: None,
            idle_task_cx: TaskContext::zero_init(),
        }
    }

    ///Get mutable reference to `idle_task_cx`
    fn get_idle_task_cx_ptr(&mut self) -> *mut TaskContext {
        &mut self.idle_task_cx as *mut _
    }

    ///Get current task in moving semanteme
    pub fn take_current(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.current.take()
    }

    ///Get current task in cloning semanteme
    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(Arc::clone)
    }
}

lazy_static! {
    pub static ref PROCESSOR: UPSafeCell<Processor> = unsafe { UPSafeCell::new(Processor::new()) };
}

///The main part of process execution and scheduling
///Loop `fetch_task` to get the process that needs to run, and switch the process through `__switch`
pub fn run_tasks() {
    loop {
        let mut processor = PROCESSOR.exclusive_access();
        if let Some(task) = fetch_task() {
            let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
            // access coming task TCB exclusively
            let mut task_inner = task.inner_exclusive_access();
            let next_task_cx_ptr = &task_inner.task_cx as *const TaskContext;
            task_inner.task_status = TaskStatus::Running;
            if task_inner.first_run == 0 {
                task_inner.first_run = get_time_ms();
            }
            // release coming task_inner manually
            drop(task_inner);
            // release coming task TCB manually
            processor.current = Some(task);
            // release processor manually
            drop(processor);
            unsafe {
                __switch(idle_task_cx_ptr, next_task_cx_ptr);
            }
        } else {
            warn!("no tasks available in run_tasks");
        }
    }
}

/// Get current task through take, leaving a None in its place
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.exclusive_access().take_current()
}

/// Get a copy of the current task
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.exclusive_access().current()
}

/// Get the current user token(addr of page table)
pub fn current_user_token() -> usize {
    let task = current_task().unwrap();
    task.get_user_token()
}

///Get the mutable reference to trap context of current task
pub fn current_trap_cx() -> &'static mut TrapContext {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .get_trap_cx()
}

///Return to idle control flow for new scheduling
pub fn schedule(switched_task_cx_ptr: *mut TaskContext) {
    let mut processor = PROCESSOR.exclusive_access();
    let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
    drop(processor);
    unsafe {
        __switch(switched_task_cx_ptr, idle_task_cx_ptr);
    }
}

/// Record the syscall times of the current task.
pub fn record_syscall(id: usize) {
    let current = current_task().unwrap();
    current.inner_exclusive_access().syscall_times[id] += 1;
}

/// Get the current task's status, syscall times and first run time.
pub fn get_current_task() -> (TaskStatus, [u32; MAX_SYSCALL_NUM], usize) {
    let current = current_task().unwrap();
    let current = current.inner_exclusive_access();
    (
        current.task_status,
        current.syscall_times,
        current.first_run,
    )
}

/// Map memory
pub fn mmap(start: usize, len: usize, prot: usize) -> isize {
    let current = current_task().unwrap();
    let mut current = current.inner_exclusive_access();
    current.memory_set.mmap(start.into(), len, prot)
}

/// Unmap memory
pub fn munmap(start: usize, len: usize) -> isize {
    let current = current_task().unwrap();
    let mut current = current.inner_exclusive_access();
    current.memory_set.munmap(start.into(), len)
}

/// Copy data from kernel to user space
pub fn copy_to_user(user: usize, kern: &[u8]) {
    let current = current_task().unwrap();
    let current = current.inner_exclusive_access();

    let mut user_pos = user;
    let mut need_copy = kern.len();

    while need_copy > 0 {
        let va = VirtAddr::from(user_pos);
        let vpn = va.floor();
        let vpoff = va.page_offset();

        let pte = current.memory_set.translate(vpn).unwrap();
        let ppn = pte.ppn();
        let dst = ppn.get_bytes_array()[vpoff..].as_mut();

        let src = &kern[kern.len() - need_copy..];

        let len = dst.len().min(need_copy).min(PAGE_SIZE - vpoff);
        dst[..len].copy_from_slice(&src[..len]);

        user_pos += len;
        need_copy -= len;
    }
}
