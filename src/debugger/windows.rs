use std::{
    array,
    ffi::c_void,
    mem::zeroed,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};

use thiserror::Error;
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE},
    System::{
        Diagnostics::Debug::{
            AddVectoredExceptionHandler, CONTEXT, CONTEXT_ALL_AMD64, CONTEXT_CONTROL_AMD64,
            CONTEXT_DEBUG_REGISTERS_AMD64, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH,
            EXCEPTION_POINTERS, GetThreadContext, RemoveVectoredExceptionHandler,
            RtlCaptureContext, SetThreadContext,
        },
        Threading::{
            GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT,
            THREAD_QUERY_INFORMATION, THREAD_SET_CONTEXT, THREAD_SUSPEND_RESUME,
        },
    },
};

use super::{
    DebuggerEvent, DebuggerEventKind, HardwareBreakpoint, HardwareBreakpointKind, RegisterSnapshot,
    RegisterValue,
};

const EXCEPTION_SINGLE_STEP_CODE: i32 = 0x8000_0004_u32 as i32;
const TRAP_FLAG: u32 = 1 << 8;
const RESUME_FLAG: u32 = 1 << 16;
const EVENT_CAPACITY: usize = 512;
const REGISTRY_CAPACITY: usize = 64;
const SINGLE_STEP_CAPACITY: usize = 64;

pub fn snapshot_registers(
    thread_id: u32,
    already_suspended: bool,
) -> Result<RegisterSnapshot, WindowsDebuggerError> {
    let current = unsafe { GetCurrentThreadId() };
    let context = if thread_id == current {
        let mut context: CONTEXT = unsafe { zeroed() };
        unsafe { RtlCaptureContext(&mut context) };
        context
    } else {
        let handle = ThreadHandle::open(
            thread_id,
            THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION | THREAD_SUSPEND_RESUME,
        )?;
        let _suspension = TemporarySuspension::maybe(&handle, already_suspended)?;
        get_context(&handle, CONTEXT_ALL_AMD64)?
    };
    Ok(context_snapshot(thread_id, &context))
}

pub fn pause_thread(thread_id: u32) -> Result<(), WindowsDebuggerError> {
    reject_current_thread(thread_id)?;
    let handle = ThreadHandle::open(thread_id, THREAD_SUSPEND_RESUME)?;
    let previous = unsafe { SuspendThread(handle.0) };
    if previous == u32::MAX {
        return Err(last_api_error("SuspendThread"));
    }
    if previous != 0 {
        let restore = unsafe { ResumeThread(handle.0) };
        if restore == u32::MAX {
            return Err(last_api_error("ResumeThread(rollback)"));
        }
        return Err(WindowsDebuggerError::ExternallySuspended {
            thread_id,
            previous_count: previous,
        });
    }
    Ok(())
}

pub fn resume_thread(thread_id: u32) -> Result<(), WindowsDebuggerError> {
    reject_current_thread(thread_id)?;
    let handle = ThreadHandle::open(thread_id, THREAD_SUSPEND_RESUME)?;
    let previous = unsafe { ResumeThread(handle.0) };
    if previous == u32::MAX {
        return Err(last_api_error("ResumeThread"));
    }
    if previous == 0 {
        return Err(WindowsDebuggerError::NotSuspended(thread_id));
    }
    Ok(())
}

pub fn arm_single_step(
    thread_id: u32,
    already_suspended: bool,
) -> Result<(), WindowsDebuggerError> {
    reject_current_thread(thread_id)?;
    let handle = ThreadHandle::open(
        thread_id,
        THREAD_GET_CONTEXT | THREAD_SET_CONTEXT | THREAD_QUERY_INFORMATION | THREAD_SUSPEND_RESUME,
    )?;
    let _suspension = TemporarySuspension::maybe(&handle, already_suspended)?;
    let mut context = get_context(&handle, CONTEXT_CONTROL_AMD64)?;
    context.EFlags |= TRAP_FLAG;
    reserve_single_step(thread_id)?;
    if unsafe { SetThreadContext(handle.0, &context) } == 0 {
        release_single_step(thread_id);
        return Err(last_api_error("SetThreadContext(single-step)"));
    }
    Ok(())
}

pub fn set_hardware_breakpoint(
    breakpoint: HardwareBreakpoint,
    already_suspended: bool,
) -> Result<(), WindowsDebuggerError> {
    reject_current_thread(breakpoint.thread_id)?;
    reserve_breakpoint(breakpoint.thread_id, breakpoint.slot)?;
    let result = set_hardware_breakpoint_inner(breakpoint, already_suspended);
    if result.is_err() {
        release_breakpoint(breakpoint.thread_id, breakpoint.slot);
    }
    result
}

fn set_hardware_breakpoint_inner(
    breakpoint: HardwareBreakpoint,
    already_suspended: bool,
) -> Result<(), WindowsDebuggerError> {
    let handle = ThreadHandle::open(
        breakpoint.thread_id,
        THREAD_GET_CONTEXT | THREAD_SET_CONTEXT | THREAD_QUERY_INFORMATION | THREAD_SUSPEND_RESUME,
    )?;
    let _suspension = TemporarySuspension::maybe(&handle, already_suspended)?;
    let mut context = get_context(
        &handle,
        CONTEXT_DEBUG_REGISTERS_AMD64 | CONTEXT_CONTROL_AMD64,
    )?;

    set_debug_address(&mut context, breakpoint.slot, breakpoint.address)?;
    let enable_shift = u32::from(breakpoint.slot) * 2;
    context.Dr7 &= !(0b11_u64 << enable_shift);
    context.Dr7 |= 1_u64 << enable_shift;

    let control_shift = 16 + u32::from(breakpoint.slot) * 4;
    context.Dr7 &= !(0b1111_u64 << control_shift);
    let rw = match breakpoint.kind {
        HardwareBreakpointKind::Execute => 0_u64,
        HardwareBreakpointKind::Write => 1_u64,
        HardwareBreakpointKind::ReadWrite => 3_u64,
    };
    let len = match breakpoint.size {
        1 => 0_u64,
        2 => 1_u64,
        4 => 3_u64,
        8 => 2_u64,
        _ => return Err(WindowsDebuggerError::InvalidBreakpointEncoding),
    };
    context.Dr7 |= (rw | (len << 2)) << control_shift;
    context.Dr6 = 0;

    if unsafe { SetThreadContext(handle.0, &context) } == 0 {
        return Err(last_api_error("SetThreadContext(hardware breakpoint)"));
    }
    Ok(())
}

pub fn clear_hardware_breakpoint(
    breakpoint: HardwareBreakpoint,
    already_suspended: bool,
) -> Result<(), WindowsDebuggerError> {
    reject_current_thread(breakpoint.thread_id)?;
    let handle = ThreadHandle::open(
        breakpoint.thread_id,
        THREAD_GET_CONTEXT | THREAD_SET_CONTEXT | THREAD_QUERY_INFORMATION | THREAD_SUSPEND_RESUME,
    )?;
    let _suspension = TemporarySuspension::maybe(&handle, already_suspended)?;
    let mut context = get_context(
        &handle,
        CONTEXT_DEBUG_REGISTERS_AMD64 | CONTEXT_CONTROL_AMD64,
    )?;
    set_debug_address(&mut context, breakpoint.slot, 0)?;
    let enable_shift = u32::from(breakpoint.slot) * 2;
    context.Dr7 &= !(0b11_u64 << enable_shift);
    let control_shift = 16 + u32::from(breakpoint.slot) * 4;
    context.Dr7 &= !(0b1111_u64 << control_shift);
    context.Dr6 = 0;
    if unsafe { SetThreadContext(handle.0, &context) } == 0 {
        return Err(last_api_error(
            "SetThreadContext(clear hardware breakpoint)",
        ));
    }
    release_breakpoint(breakpoint.thread_id, breakpoint.slot);
    Ok(())
}

pub fn acquire_vectored_handler() -> Result<(), WindowsDebuggerError> {
    LazyLock::force(&EVENT_RING);
    LazyLock::force(&BREAKPOINT_REGISTRY);
    LazyLock::force(&SINGLE_STEP_REGISTRY);
    let mut registration = VEH_REGISTRATION
        .lock()
        .map_err(|_| WindowsDebuggerError::RegistrationPoisoned)?;
    if registration.users == 0 {
        let handle = unsafe { AddVectoredExceptionHandler(1, Some(vectored_handler)) };
        if handle.is_null() {
            return Err(last_api_error("AddVectoredExceptionHandler"));
        }
        registration.handle = handle as usize;
    }
    registration.users += 1;
    Ok(())
}

pub fn release_vectored_handler() -> Result<(), WindowsDebuggerError> {
    let mut registration = VEH_REGISTRATION
        .lock()
        .map_err(|_| WindowsDebuggerError::RegistrationPoisoned)?;
    if registration.users == 0 {
        return Ok(());
    }
    registration.users -= 1;
    if registration.users == 0 && registration.handle != 0 {
        let handle = registration.handle as *mut c_void;
        if unsafe { RemoveVectoredExceptionHandler(handle) } == 0 {
            registration.users = 1;
            return Err(WindowsDebuggerError::Api {
                operation: "RemoveVectoredExceptionHandler",
                code: unsafe { GetLastError() },
            });
        }
        registration.handle = 0;
    }
    Ok(())
}

pub fn read_events(after_sequence: u64, limit: usize) -> Vec<DebuggerEvent> {
    EVENT_RING.read(after_sequence, limit)
}

pub fn latest_event_sequence() -> u64 {
    EVENT_RING.latest_sequence()
}

unsafe extern "system" fn vectored_handler(exception_info: *mut EXCEPTION_POINTERS) -> i32 {
    if exception_info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let pointers = unsafe { &mut *exception_info };
    if pointers.ExceptionRecord.is_null() || pointers.ContextRecord.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let record = unsafe { &*pointers.ExceptionRecord };
    if record.ExceptionCode != EXCEPTION_SINGLE_STEP_CODE {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let context = unsafe { &mut *pointers.ContextRecord };
    let thread_id = unsafe { GetCurrentThreadId() };
    let address = record.ExceptionAddress as usize as u64;
    let hit_mask = context.Dr6 & 0xF;
    let mut handled = false;

    for slot in 0_u8..4 {
        if hit_mask & (1_u64 << slot) == 0 || !breakpoint_is_ours(thread_id, slot) {
            continue;
        }
        EVENT_RING.push(thread_id, address, 1, slot);
        let control_shift = 16 + u32::from(slot) * 4;
        let rw = (context.Dr7 >> control_shift) & 0b11;
        if rw == 0 {
            context.EFlags |= RESUME_FLAG;
        }
        context.Dr6 &= !(1_u64 << slot);
        handled = true;
    }

    if take_single_step(thread_id) {
        context.EFlags &= !TRAP_FLAG;
        EVENT_RING.push(thread_id, address, 2, 0);
        handled = true;
    }

    if handled {
        EXCEPTION_CONTINUE_EXECUTION
    } else {
        EXCEPTION_CONTINUE_SEARCH
    }
}

fn context_snapshot(thread_id: u32, context: &CONTEXT) -> RegisterSnapshot {
    let registers = [
        ("RAX", context.Rax),
        ("RBX", context.Rbx),
        ("RCX", context.Rcx),
        ("RDX", context.Rdx),
        ("RSI", context.Rsi),
        ("RDI", context.Rdi),
        ("RBP", context.Rbp),
        ("R8", context.R8),
        ("R9", context.R9),
        ("R10", context.R10),
        ("R11", context.R11),
        ("R12", context.R12),
        ("R13", context.R13),
        ("R14", context.R14),
        ("R15", context.R15),
        ("DR0", context.Dr0),
        ("DR1", context.Dr1),
        ("DR2", context.Dr2),
        ("DR3", context.Dr3),
        ("DR6", context.Dr6),
        ("DR7", context.Dr7),
    ]
    .into_iter()
    .map(|(name, value)| RegisterValue {
        name: name.to_owned(),
        value,
    })
    .collect();
    RegisterSnapshot {
        thread_id,
        instruction_pointer: context.Rip,
        stack_pointer: context.Rsp,
        flags: u64::from(context.EFlags),
        registers,
    }
}

fn get_context(handle: &ThreadHandle, flags: u32) -> Result<CONTEXT, WindowsDebuggerError> {
    let mut context: CONTEXT = unsafe { zeroed() };
    context.ContextFlags = flags;
    if unsafe { GetThreadContext(handle.0, &mut context) } == 0 {
        return Err(last_api_error("GetThreadContext"));
    }
    Ok(context)
}

fn set_debug_address(
    context: &mut CONTEXT,
    slot: u8,
    address: u64,
) -> Result<(), WindowsDebuggerError> {
    match slot {
        0 => context.Dr0 = address,
        1 => context.Dr1 = address,
        2 => context.Dr2 = address,
        3 => context.Dr3 = address,
        _ => return Err(WindowsDebuggerError::InvalidBreakpointSlot(slot)),
    }
    Ok(())
}

fn reject_current_thread(thread_id: u32) -> Result<(), WindowsDebuggerError> {
    if thread_id == unsafe { GetCurrentThreadId() } {
        Err(WindowsDebuggerError::CurrentThreadOperation(thread_id))
    } else {
        Ok(())
    }
}

struct ThreadHandle(HANDLE);
impl ThreadHandle {
    fn open(thread_id: u32, access: u32) -> Result<Self, WindowsDebuggerError> {
        let handle = unsafe { OpenThread(access, 0, thread_id) };
        if handle.is_null() {
            return Err(last_api_error("OpenThread"));
        }
        Ok(Self(handle))
    }
}
impl Drop for ThreadHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

struct TemporarySuspension<'a> {
    handle: Option<&'a ThreadHandle>,
}
impl<'a> TemporarySuspension<'a> {
    fn maybe(
        handle: &'a ThreadHandle,
        already_suspended: bool,
    ) -> Result<Self, WindowsDebuggerError> {
        if already_suspended {
            return Ok(Self { handle: None });
        }
        let previous = unsafe { SuspendThread(handle.0) };
        if previous == u32::MAX {
            return Err(last_api_error("SuspendThread(temporary)"));
        }
        if previous != 0 {
            let restore = unsafe { ResumeThread(handle.0) };
            if restore == u32::MAX {
                return Err(last_api_error("ResumeThread(temporary rollback)"));
            }
            return Err(WindowsDebuggerError::UnexpectedSuspendCount(previous));
        }
        Ok(Self {
            handle: Some(handle),
        })
    }
}
impl Drop for TemporarySuspension<'_> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle {
            let _ = unsafe { ResumeThread(handle.0) };
        }
    }
}

struct VehRegistration {
    handle: usize,
    users: usize,
}
static VEH_REGISTRATION: Mutex<VehRegistration> = Mutex::new(VehRegistration {
    handle: 0,
    users: 0,
});

struct EventSlot {
    sequence: AtomicU64,
    thread_id: AtomicU32,
    address: AtomicU64,
    kind: AtomicU32,
    slot: AtomicU32,
}
impl EventSlot {
    fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            thread_id: AtomicU32::new(0),
            address: AtomicU64::new(0),
            kind: AtomicU32::new(0),
            slot: AtomicU32::new(0),
        }
    }
}
struct EventRing {
    next_sequence: AtomicU64,
    slots: [EventSlot; EVENT_CAPACITY],
}
impl EventRing {
    fn new() -> Self {
        Self {
            next_sequence: AtomicU64::new(0),
            slots: array::from_fn(|_| EventSlot::new()),
        }
    }
    fn push(&self, thread_id: u32, address: u64, kind: u32, slot: u8) {
        let sequence = self.next_sequence.fetch_add(1, Ordering::AcqRel) + 1;
        let entry = &self.slots[(sequence as usize - 1) % EVENT_CAPACITY];
        entry.thread_id.store(thread_id, Ordering::Relaxed);
        entry.address.store(address, Ordering::Relaxed);
        entry.kind.store(kind, Ordering::Relaxed);
        entry.slot.store(u32::from(slot), Ordering::Relaxed);
        entry.sequence.store(sequence, Ordering::Release);
    }
    fn latest_sequence(&self) -> u64 {
        self.next_sequence.load(Ordering::Acquire)
    }
    fn read(&self, after: u64, limit: usize) -> Vec<DebuggerEvent> {
        let latest = self.latest_sequence();
        let oldest = latest.saturating_sub(EVENT_CAPACITY as u64 - 1).max(1);
        let start = after.saturating_add(1).max(oldest);
        let mut events = Vec::new();
        for sequence in start..=latest {
            if events.len() >= limit {
                break;
            }
            let entry = &self.slots[(sequence as usize - 1) % EVENT_CAPACITY];
            if entry.sequence.load(Ordering::Acquire) != sequence {
                continue;
            }
            let kind = match entry.kind.load(Ordering::Relaxed) {
                1 => DebuggerEventKind::HardwareBreakpoint {
                    slot: entry.slot.load(Ordering::Relaxed) as u8,
                },
                2 => DebuggerEventKind::SingleStep,
                _ => continue,
            };
            events.push(DebuggerEvent {
                sequence,
                thread_id: entry.thread_id.load(Ordering::Relaxed),
                address: entry.address.load(Ordering::Relaxed),
                kind,
            });
        }
        events
    }
}
static EVENT_RING: LazyLock<EventRing> = LazyLock::new(EventRing::new);

struct BreakpointRegistryEntry {
    state: AtomicU32,
    thread_id: AtomicU32,
    slot: AtomicU32,
}
impl BreakpointRegistryEntry {
    fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            thread_id: AtomicU32::new(0),
            slot: AtomicU32::new(0),
        }
    }
}
static BREAKPOINT_REGISTRY: LazyLock<[BreakpointRegistryEntry; REGISTRY_CAPACITY]> =
    LazyLock::new(|| array::from_fn(|_| BreakpointRegistryEntry::new()));

fn reserve_breakpoint(thread_id: u32, slot: u8) -> Result<(), WindowsDebuggerError> {
    for entry in BREAKPOINT_REGISTRY.iter() {
        if entry
            .state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            entry.thread_id.store(thread_id, Ordering::Relaxed);
            entry.slot.store(u32::from(slot), Ordering::Relaxed);
            entry.state.store(1, Ordering::Release);
            return Ok(());
        }
    }
    Err(WindowsDebuggerError::BreakpointRegistryFull)
}
fn release_breakpoint(thread_id: u32, slot: u8) {
    for entry in BREAKPOINT_REGISTRY.iter() {
        if entry.state.load(Ordering::Acquire) == 1
            && entry.thread_id.load(Ordering::Relaxed) == thread_id
            && entry.slot.load(Ordering::Relaxed) == u32::from(slot)
        {
            entry.state.store(0, Ordering::Release);
            return;
        }
    }
}
fn breakpoint_is_ours(thread_id: u32, slot: u8) -> bool {
    BREAKPOINT_REGISTRY.iter().any(|entry| {
        entry.state.load(Ordering::Acquire) == 1
            && entry.thread_id.load(Ordering::Relaxed) == thread_id
            && entry.slot.load(Ordering::Relaxed) == u32::from(slot)
    })
}

static SINGLE_STEP_REGISTRY: LazyLock<[AtomicU32; SINGLE_STEP_CAPACITY]> =
    LazyLock::new(|| array::from_fn(|_| AtomicU32::new(0)));
fn reserve_single_step(thread_id: u32) -> Result<(), WindowsDebuggerError> {
    if SINGLE_STEP_REGISTRY
        .iter()
        .any(|entry| entry.load(Ordering::Acquire) == thread_id)
    {
        return Err(WindowsDebuggerError::SingleStepAlreadyArmed(thread_id));
    }
    for entry in SINGLE_STEP_REGISTRY.iter() {
        if entry
            .compare_exchange(0, thread_id, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Ok(());
        }
    }
    Err(WindowsDebuggerError::SingleStepRegistryFull)
}
fn release_single_step(thread_id: u32) {
    for entry in SINGLE_STEP_REGISTRY.iter() {
        let _ = entry.compare_exchange(thread_id, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}
fn take_single_step(thread_id: u32) -> bool {
    for entry in SINGLE_STEP_REGISTRY.iter() {
        if entry
            .compare_exchange(thread_id, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
    }
    false
}

fn last_api_error(operation: &'static str) -> WindowsDebuggerError {
    WindowsDebuggerError::Api {
        operation,
        code: unsafe { GetLastError() },
    }
}

#[derive(Debug, Error)]
pub enum WindowsDebuggerError {
    #[error("Windows debugger API call {operation} failed with error code {code}")]
    Api { operation: &'static str, code: u32 },
    #[error("refusing to suspend or mutate the current Intimatr worker thread {0}")]
    CurrentThreadOperation(u32),
    #[error(
        "thread {thread_id} was already externally suspended (previous count {previous_count})"
    )]
    ExternallySuspended { thread_id: u32, previous_count: u32 },
    #[error("thread {0} was not suspended when Intimatr attempted to resume it")]
    NotSuspended(u32),
    #[error("unexpected pre-existing thread suspend count {0}")]
    UnexpectedSuspendCount(u32),
    #[error("invalid hardware breakpoint slot {0}")]
    InvalidBreakpointSlot(u8),
    #[error("invalid hardware breakpoint encoding")]
    InvalidBreakpointEncoding,
    #[error("Intimatr hardware-breakpoint registry is full")]
    BreakpointRegistryFull,
    #[error("single-step is already armed for thread {0}")]
    SingleStepAlreadyArmed(u32),
    #[error("Intimatr single-step registry is full")]
    SingleStepRegistryFull,
    #[error("vectored exception handler registration mutex was poisoned")]
    RegistrationPoisoned,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_current_thread_registers_without_suspending_it() {
        let thread_id = unsafe { GetCurrentThreadId() };
        let snapshot =
            snapshot_registers(thread_id, false).expect("current context should capture");
        assert_eq!(snapshot.thread_id, thread_id);
        assert_ne!(snapshot.instruction_pointer, 0);
        assert_ne!(snapshot.stack_pointer, 0);
    }

    #[test]
    fn event_ring_preserves_order() {
        let before = latest_event_sequence();
        EVENT_RING.push(11, 0x1000, 2, 0);
        EVENT_RING.push(12, 0x2000, 1, 3);
        let events = read_events(before, 8);
        assert_eq!(events.len(), 2);
        assert!(events[0].sequence < events[1].sequence);
        assert_eq!(
            events[1].kind,
            DebuggerEventKind::HardwareBreakpoint { slot: 3 }
        );
    }
}
