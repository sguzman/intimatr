#![cfg(windows)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use intimatr::{
    config::DebuggerConfig,
    debugger::{DebuggerCore, DebuggerEventKind, HardwareBreakpointKind},
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

struct TestWorker {
    thread_id: u32,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl TestWorker {
    fn spawn() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let thread_id = unsafe { GetCurrentThreadId() };
            sender.send(thread_id).expect("send worker thread id");
            while !worker_stop.load(Ordering::Acquire) {
                thread::yield_now();
            }
        });
        let thread_id = receiver.recv().expect("receive worker thread id");
        Self {
            thread_id,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for TestWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.join().expect("debugger test worker should exit");
        }
    }
}

#[inline(never)]
fn breakpoint_probe() {
    std::hint::black_box(());
}

#[test]
fn controls_a_separate_thread_and_tracks_breakpoints_and_events() {
    let worker = TestWorker::spawn();
    let debugger = DebuggerCore::new(DebuggerConfig::default());

    let paused = debugger
        .pause_thread(worker.thread_id)
        .expect("pause selected worker thread");
    assert!(paused.paused_by_intimatr);
    assert!(
        debugger
            .status()
            .expect("read debugger status")
            .paused_threads
            .contains(&worker.thread_id)
    );

    let registers = debugger
        .read_registers(worker.thread_id)
        .expect("capture paused worker registers");
    assert_eq!(registers.thread_id, worker.thread_id);
    assert_ne!(registers.instruction_pointer, 0);
    assert_ne!(registers.stack_pointer, 0);

    let before_step = debugger
        .status()
        .expect("read pre-step debugger status")
        .latest_event_sequence;
    let stepped = debugger
        .single_step_thread(worker.thread_id)
        .expect("single-step paused worker");
    assert!(!stepped.paused_by_intimatr);

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_step = false;
    while Instant::now() < deadline {
        let events = debugger
            .events(before_step, 32)
            .expect("poll debugger event stream");
        if events.iter().any(|event| {
            event.thread_id == worker.thread_id
                && matches!(event.kind, DebuggerEventKind::SingleStep)
        }) {
            saw_step = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(saw_step, "single-step should produce a debugger event");

    let breakpoint = debugger
        .set_hardware_breakpoint(
            worker.thread_id,
            breakpoint_probe as usize as u64,
            HardwareBreakpointKind::Execute,
            1,
        )
        .expect("install hardware execute breakpoint");
    assert!(breakpoint.slot < 4);
    assert!(
        debugger
            .list_hardware_breakpoints()
            .expect("list hardware breakpoints")
            .contains(&breakpoint)
    );
    assert!(
        debugger
            .remove_hardware_breakpoint(breakpoint.id)
            .expect("remove hardware breakpoint")
    );
    assert!(
        debugger
            .list_hardware_breakpoints()
            .expect("list hardware breakpoints after removal")
            .is_empty()
    );

    let paused_again = debugger
        .pause_thread(worker.thread_id)
        .expect("pause worker after single-step");
    assert!(paused_again.paused_by_intimatr);
    let resumed = debugger
        .resume_thread(worker.thread_id)
        .expect("resume Intimatr-paused worker");
    assert!(!resumed.paused_by_intimatr);
}
