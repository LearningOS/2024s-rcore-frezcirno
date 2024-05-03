use crate::sync::{Condvar, Mutex, MutexBlocking, MutexSpin, Semaphore, UPSafeCell};
use crate::task::{block_current_and_run_next, current_process, current_task};
use crate::timer::{add_timer, get_time_ms};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use lazy_static::*;

type Map = BTreeMap<usize, usize>;
type Set = BTreeSet<usize>;

lazy_static! {
    /// Resource -> Count
    static ref AVAILABLE: Arc<UPSafeCell<Map>> = Arc::new(unsafe { UPSafeCell::new(Map::new()) });
    /// Task -> (Holds, Waits)
    /// If Waits is not empty, the task is blocking wait the resource
    static ref TASK_STATUS: Arc<UPSafeCell<BTreeMap<usize, (Map, Set)>>> = Arc::new(unsafe { UPSafeCell::new(BTreeMap::new()) });
}

fn create_resource(res_id: usize, res_count: usize) {
    warn!("create_resource({}, {})", res_id, res_count);
    AVAILABLE.exclusive_access().insert(res_id, res_count);
}

fn deadlock_check() -> bool {
    // deadlock detection
    let available = AVAILABLE.exclusive_access();
    let tasks = TASK_STATUS.exclusive_access();

    // Simulate the deadlock detection algorithm
    let mut simulator = available.clone();
    let mut finished = BTreeMap::new();
    let mut deadlock = false;
    loop {
        // Find next task that needs to be finished
        let mut have_advance = false;
        let mut all_clear: bool = true;
        for (&tid, (holds, waits)) in tasks.iter() {
            if finished.get(&tid) == Some(&true) {
                continue;
            }

            all_clear = false;

            let mut can_finish = true;
            for res_id in waits.iter() {
                let av = simulator.get(res_id).unwrap();
                if av == &0 {
                    can_finish = false;
                    break;
                }
            }

            if can_finish {
                // Finish the task
                finished.insert(tid, true);
                // Release its resources
                for (res_id, count) in holds.iter() {
                    *simulator.get_mut(res_id).unwrap() += count;
                }
                have_advance = true;
                break;
            }
        }
        if !have_advance {
            if !all_clear {
                deadlock = true;
            }
            break;
        }
    }
    deadlock
}

fn wait_resource(tid: usize, res_id: usize) -> bool {
    warn!("[{}] wait_resource({})", tid, res_id);

    // add res to waits
    TASK_STATUS
        .exclusive_access()
        .entry(tid)
        .or_default()
        .1
        .insert(res_id);

    let deadlock = deadlock_check();
    if deadlock {
        // dump the deadlock status
        warn!("[{}] deadlock detected", tid);
        for (&res_id, count) in AVAILABLE.exclusive_access().iter() {
            warn!("   {} available: {}", res_id, count);
        }
        for (&tid1, (holds, waits)) in TASK_STATUS.exclusive_access().iter() {
            warn!("   {} holds: {:?}, waits: {:?}", tid1, holds, waits);
        }
    }
    deadlock
}

fn acquire_resource(tid: usize, res_id: usize) {
    let mut allo = TASK_STATUS.exclusive_access();
    let (holds, waits) = allo.entry(tid).or_default();
    assert!(waits.remove(&res_id));
    let hold_count: &mut usize = holds.entry(res_id).or_default();
    warn!(
        "[{}] acquire_resource({}, {}->{})",
        tid,
        res_id,
        *hold_count,
        *hold_count + 1
    );
    *hold_count += 1;

    let mut aval = AVAILABLE.exclusive_access();
    let avail: &mut usize = aval.get_mut(&res_id).unwrap();
    *avail -= 1;
}

fn release_resource(tid: usize, res_id: usize) {
    let mut allo = TASK_STATUS.exclusive_access();
    let (holds, _) = allo.get_mut(&tid).unwrap();
    let hold_count = match holds.get_mut(&res_id) {
        Some(count) => count,
        None => {
            // The task does not hold the resource
            return;
        }
    };
    warn!(
        "[{}] release_resource({}, {}->{})",
        tid,
        res_id,
        *hold_count,
        *hold_count - 1
    );
    *hold_count -= 1;
    if *hold_count == 0 {
        holds.remove(&res_id);
        if holds.is_empty() {
            allo.remove(&tid);
        }
    }

    let mut aval = AVAILABLE.exclusive_access();
    let avail: &mut usize = aval.get_mut(&res_id).unwrap();
    *avail += 1;
}

/// sleep syscall
pub fn sys_sleep(ms: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_sleep",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let expire_ms = get_time_ms() + ms;
    let task = current_task().unwrap();
    add_timer(expire_ms, task);
    block_current_and_run_next();
    0
}
/// mutex create syscall
pub fn sys_mutex_create(blocking: bool) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mutex: Option<Arc<dyn Mutex>> = if !blocking {
        Some(Arc::new(MutexSpin::new()))
    } else {
        Some(Arc::new(MutexBlocking::new()))
    };

    create_resource(mutex.as_ref().unwrap().id(), 1);

    let mut process_inner = process.inner_exclusive_access();
    if let Some(id) = process_inner
        .mutex_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.mutex_list[id] = mutex;
        id as isize
    } else {
        process_inner.mutex_list.push(mutex);
        process_inner.mutex_list.len() as isize - 1
    }
}
/// mutex lock syscall
pub fn sys_mutex_lock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_lock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    let do_deadlock_detect = process_inner.deadlock_detect;
    drop(process_inner);
    drop(process);
    let deadlock = wait_resource(tid, mutex.id());
    if do_deadlock_detect && deadlock {
        return -0xdead;
    }
    mutex.lock();
    acquire_resource(tid, mutex.id());
    0
}
/// mutex unlock syscall
pub fn sys_mutex_unlock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_unlock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    drop(process);
    mutex.unlock();
    release_resource(tid, mutex.id());
    0
}
/// semaphore create syscall
pub fn sys_semaphore_create(res_count: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .semaphore_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.semaphore_list[id] = Some(Arc::new(Semaphore::new(res_count)));
        id
    } else {
        process_inner
            .semaphore_list
            .push(Some(Arc::new(Semaphore::new(res_count))));
        process_inner.semaphore_list.len() - 1
    };
    create_resource(
        process_inner.semaphore_list[id].as_ref().unwrap().id,
        res_count,
    );
    id as isize
}
/// semaphore up syscall
pub fn sys_semaphore_up(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_up",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());
    drop(process_inner);
    drop(process);
    sem.up();
    release_resource(tid, sem.id);
    0
}
/// semaphore down syscall
pub fn sys_semaphore_down(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_down",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());
    let do_deadlock_detect = process_inner.deadlock_detect;
    drop(process_inner);
    let deadlock = wait_resource(tid, sem.id);
    if do_deadlock_detect && deadlock {
        return -0xdead;
    }
    sem.down();
    acquire_resource(tid, sem.id);
    0
}
/// condvar create syscall
pub fn sys_condvar_create() -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .condvar_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.condvar_list[id] = Some(Arc::new(Condvar::new()));
        id
    } else {
        process_inner
            .condvar_list
            .push(Some(Arc::new(Condvar::new())));
        process_inner.condvar_list.len() - 1
    };
    id as isize
}
/// condvar signal syscall
pub fn sys_condvar_signal(condvar_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_signal",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    drop(process_inner);
    condvar.signal();
    0
}
/// condvar wait syscall
pub fn sys_condvar_wait(condvar_id: usize, mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_wait",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    condvar.wait(mutex);
    0
}
/// enable deadlock detection syscall
///
/// YOUR JOB: Implement deadlock detection, but might not all in this syscall
pub fn sys_enable_deadlock_detect(enabled: usize) -> isize {
    trace!("kernel: sys_enable_deadlock_detect");
    let process = current_process();
    process.inner_exclusive_access().deadlock_detect = enabled != 0;
    0
}
