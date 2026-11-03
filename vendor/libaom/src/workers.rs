//! libaom's worker interface (aom_util/aom_thread.h) on rayon's threads, in place of libaom's own:
//! an encoder's workers are jobs that whichever thread gets to first runs, rayon's (with the
//! parallel feature) or the encoder's own when it waits for one nobody has started, so encoders
//! running at once never wait on each other for a thread. The browser has no threads of its own to
//! give libaom, only rayon's Web Workers; without the parallel feature (web/'s single-threaded
//! build) each job runs when the encoder waits for it, one after another. libaom hands rows out to
//! workers as they ask, so one worker may do them all, and the bytes don't change: any number of
//! workers from 2 writes the same file.

use std::ffi::{c_char, c_int, c_void};
use std::sync::{Arc, Condvar, Mutex};

/// Workers per encoder: 2 or more, for libaom's multithreaded encoder, whose files differ from its
/// single-threaded one's; and as many as rayon has threads, which are the ones that can run them.
pub fn count() -> usize {
    #[cfg(feature = "parallel")]
    let threads = rayon::current_num_threads();
    #[cfg(not(feature = "parallel"))]
    let threads = 1;
    // libaom's MAX_NUM_THREADS
    threads.clamp(2, 64)
}

/// Installs the interface; before any encoder starts.
pub unsafe fn install() {
    let interface = AVxWorkerInterface { init, reset, sync, launch, execute, end };
    // SAFETY: called once, before any encoder, as aom_set_worker_interface requires
    assert!(unsafe { aom_set_worker_interface(&interface) } != 0);
}

type Hook = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;

#[repr(C)]
struct AVxWorker {
    implementation: *mut c_void,
    status: c_int,
    thread_name: *const c_char,
    hook: Option<Hook>,
    data1: *mut c_void,
    data2: *mut c_void,
    had_error: c_int,
}

#[repr(C)]
struct AVxWorkerInterface {
    init: unsafe extern "C" fn(*mut AVxWorker),
    reset: unsafe extern "C" fn(*mut AVxWorker) -> c_int,
    sync: unsafe extern "C" fn(*mut AVxWorker) -> c_int,
    launch: unsafe extern "C" fn(*mut AVxWorker),
    execute: unsafe extern "C" fn(*mut AVxWorker),
    end: unsafe extern "C" fn(*mut AVxWorker),
}

extern "C" {
    fn aom_set_worker_interface(interface: *const AVxWorkerInterface) -> c_int;
}

// AVxWorkerStatus
const NOT_OK: c_int = 0;
const OK: c_int = 1;
const WORKING: c_int = 2;

/// A launched hook and its arguments.
#[derive(Clone, Copy)]
struct Task {
    hook: Option<Hook>,
    data1: *mut c_void,
    data2: *mut c_void,
}

// SAFETY: libaom hands a task's pointers to another thread, as its own workers do, and keeps them
// valid until it syncs the worker, which waits for the task
unsafe impl Send for Task {}

enum State {
    Idle,
    Launched(Task),
    Running,
    Done { ok: bool },
}

/// A worker's job, shared with the rayon task that runs it, which may only get to it once libaom
/// has synced it (having run it itself), or launched it again, or ended the worker.
struct Job {
    state: Mutex<State>,
    done: Condvar,
}

impl Job {
    /// Runs the launched task, unless something else already has.
    fn run(&self) {
        let task = {
            let mut state = self.state.lock().unwrap();
            let State::Launched(task) = *state else { return };
            *state = State::Running;
            task
        };
        // SAFETY: libaom's hook with its arguments (see Task)
        let ok = task.hook.is_none_or(|hook| unsafe { hook(task.data1, task.data2) } != 0);
        *self.state.lock().unwrap() = State::Done { ok };
        self.done.notify_all();
    }
}

/// The worker's job; libaom only calls the interface on a worker after init.
unsafe fn job<'a>(worker: *mut AVxWorker) -> &'a Job {
    unsafe { &*((*worker).implementation as *const Job) }
}

unsafe extern "C" fn init(worker: *mut AVxWorker) {
    unsafe {
        std::ptr::write_bytes(worker, 0, 1);
        (*worker).status = NOT_OK;
    }
}

unsafe extern "C" fn reset(worker: *mut AVxWorker) -> c_int {
    unsafe {
        (*worker).had_error = 0;
        if (*worker).status < OK {
            let job = Job { state: Mutex::new(State::Idle), done: Condvar::new() };
            (*worker).implementation = Arc::into_raw(Arc::new(job)) as *mut c_void;
            (*worker).status = OK;
            1
        } else if (*worker).status > OK {
            sync(worker)
        } else {
            1
        }
    }
}

unsafe extern "C" fn sync(worker: *mut AVxWorker) -> c_int {
    unsafe {
        if (*worker).status == WORKING {
            let job = job(worker);
            // not started: run it here
            job.run();
            let mut state = job.state.lock().unwrap();
            while let State::Running = *state {
                state = job.done.wait(state).unwrap();
            }
            if let State::Done { ok: false } = *state {
                (*worker).had_error = 1;
            }
            *state = State::Idle;
            (*worker).status = OK;
        }
        ((*worker).had_error == 0) as c_int
    }
}

unsafe extern "C" fn launch(worker: *mut AVxWorker) {
    unsafe {
        if (*worker).status != OK {
            return;
        }
        let task = Task { hook: (*worker).hook, data1: (*worker).data1, data2: (*worker).data2 };
        *job(worker).state.lock().unwrap() = State::Launched(task);
        (*worker).status = WORKING;
        #[cfg(feature = "parallel")]
        {
            let job = (*worker).implementation as *const Job;
            Arc::increment_strong_count(job);
            let job = Arc::from_raw(job);
            rayon::spawn(move || job.run());
        }
    }
}

unsafe extern "C" fn execute(worker: *mut AVxWorker) {
    unsafe {
        if let Some(hook) = (*worker).hook {
            (*worker).had_error |= (hook((*worker).data1, (*worker).data2) == 0) as c_int;
        }
    }
}

unsafe extern "C" fn end(worker: *mut AVxWorker) {
    unsafe {
        if !(*worker).implementation.is_null() {
            sync(worker);
            drop(Arc::from_raw((*worker).implementation as *const Job));
            (*worker).implementation = std::ptr::null_mut();
        }
        (*worker).status = NOT_OK;
    }
}
