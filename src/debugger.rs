use std::{
    collections::{HashMap, HashSet},
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::{
    config::DebuggerConfig,
    memory::{MemoryError, MemorySource},
};

#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareBreakpointKind {
    Execute,
    Write,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareBreakpoint {
    pub id: u64,
    pub thread_id: u32,
    pub slot: u8,
    pub address: u64,
    pub kind: HardwareBreakpointKind,
    pub size: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterValue {
    pub name: String,
    pub value: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterSnapshot {
    pub thread_id: u32,
    pub instruction_pointer: u64,
    pub stack_pointer: u64,
    pub flags: u64,
    pub registers: Vec<RegisterValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisassemblyLine {
    pub address: u64,
    pub bytes: Vec<u8>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebuggerEventKind {
    HardwareBreakpoint { slot: u8 },
    SingleStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebuggerEvent {
    pub sequence: u64,
    pub thread_id: u32,
    pub address: u64,
    pub kind: DebuggerEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadControlState {
    pub thread_id: u32,
    pub paused_by_intimatr: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebuggerStatus {
    pub paused_threads: Vec<u32>,
    pub breakpoints: Vec<HardwareBreakpoint>,
    pub latest_event_sequence: u64,
}

pub struct DebuggerCore {
    config: DebuggerConfig,
    operation_lock: Mutex<()>,
    paused_threads: Mutex<HashSet<u32>>,
    breakpoints: Mutex<HashMap<u64, HardwareBreakpoint>>,
    next_breakpoint_id: AtomicU64,
    #[cfg(windows)]
    veh_acquired: std::sync::atomic::AtomicBool,
}

impl DebuggerCore {
    pub fn new(config: DebuggerConfig) -> Self {
        Self {
            config,
            operation_lock: Mutex::new(()),
            paused_threads: Mutex::new(HashSet::new()),
            breakpoints: Mutex::new(HashMap::new()),
            next_breakpoint_id: AtomicU64::new(1),
            #[cfg(windows)]
            veh_acquired: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn config(&self) -> &DebuggerConfig {
        &self.config
    }

    pub fn read_registers(&self, thread_id: u32) -> Result<RegisterSnapshot, DebuggerError> {
        self.ensure_enabled()?;
        let _operation = lock(&self.operation_lock)?;
        #[cfg(windows)]
        {
            let paused = lock(&self.paused_threads)?.contains(&thread_id);
            return windows::snapshot_registers(thread_id, paused).map_err(DebuggerError::from);
        }
        #[cfg(not(windows))]
        {
            let _ = thread_id;
            Err(DebuggerError::UnsupportedPlatform)
        }
    }

    pub fn disassemble<M: MemorySource>(
        &self,
        memory: &M,
        address: u64,
        byte_count: usize,
        max_instructions: usize,
        bitness: u32,
    ) -> Result<Vec<DisassemblyLine>, DebuggerError> {
        self.ensure_enabled()?;
        if !matches!(bitness, 16 | 32 | 64) {
            return Err(DebuggerError::InvalidBitness(bitness));
        }
        if byte_count == 0 || byte_count > self.config.max_disassembly_bytes {
            return Err(DebuggerError::LimitExceeded {
                resource: "disassembly bytes",
                requested: byte_count,
                limit: self.config.max_disassembly_bytes,
            });
        }
        if max_instructions == 0 || max_instructions > self.config.max_disassembly_instructions {
            return Err(DebuggerError::LimitExceeded {
                resource: "disassembly instructions",
                requested: max_instructions,
                limit: self.config.max_disassembly_instructions,
            });
        }

        let native_address =
            usize::try_from(address).map_err(|_| DebuggerError::AddressOutOfRange(address))?;
        native_address
            .checked_add(byte_count)
            .ok_or(DebuggerError::AddressRangeOverflow {
                address,
                size: byte_count,
            })?;
        let mut bytes = vec![0_u8; byte_count];
        memory.read_exact(native_address, &mut bytes)?;

        let mut decoder = Decoder::with_ip(bitness, &bytes, address, DecoderOptions::NONE);
        let mut formatter = IntelFormatter::new();
        let mut lines = Vec::new();
        while decoder.can_decode() && lines.len() < max_instructions {
            let instruction = decoder.decode();
            let offset = instruction.ip().saturating_sub(address) as usize;
            let length = instruction.len();
            let end = offset.saturating_add(length).min(bytes.len());
            let instruction_bytes = if offset < bytes.len() {
                bytes[offset..end].to_vec()
            } else {
                Vec::new()
            };
            let mut text = String::new();
            formatter.format(&instruction, &mut text);
            lines.push(DisassemblyLine {
                address: instruction.ip(),
                bytes: instruction_bytes,
                text,
            });
        }
        debug!(
            address,
            byte_count,
            max_instructions,
            decoded = lines.len(),
            bitness,
            "disassembled memory"
        );
        Ok(lines)
    }

    pub fn pause_thread(&self, thread_id: u32) -> Result<ThreadControlState, DebuggerError> {
        self.ensure_enabled()?;
        let _operation = lock(&self.operation_lock)?;
        let mut paused = lock(&self.paused_threads)?;
        if paused.contains(&thread_id) {
            return Ok(ThreadControlState {
                thread_id,
                paused_by_intimatr: true,
            });
        }
        #[cfg(windows)]
        windows::pause_thread(thread_id)?;
        #[cfg(not(windows))]
        return Err(DebuggerError::UnsupportedPlatform);
        paused.insert(thread_id);
        info!(thread_id, "paused thread through Intimatr debugger");
        Ok(ThreadControlState {
            thread_id,
            paused_by_intimatr: true,
        })
    }

    pub fn resume_thread(&self, thread_id: u32) -> Result<ThreadControlState, DebuggerError> {
        self.ensure_enabled()?;
        let _operation = lock(&self.operation_lock)?;
        let mut paused = lock(&self.paused_threads)?;
        if !paused.contains(&thread_id) {
            return Err(DebuggerError::ThreadNotPaused(thread_id));
        }
        #[cfg(windows)]
        windows::resume_thread(thread_id)?;
        #[cfg(not(windows))]
        return Err(DebuggerError::UnsupportedPlatform);
        paused.remove(&thread_id);
        info!(thread_id, "resumed Intimatr-paused thread");
        Ok(ThreadControlState {
            thread_id,
            paused_by_intimatr: false,
        })
    }

    pub fn single_step_thread(&self, thread_id: u32) -> Result<ThreadControlState, DebuggerError> {
        self.ensure_enabled()?;
        let _operation = lock(&self.operation_lock)?;
        #[cfg(windows)]
        {
            self.ensure_veh()?;
            let mut paused = lock(&self.paused_threads)?;
            let already_paused = paused.contains(&thread_id);
            windows::arm_single_step(thread_id, already_paused)?;
            if already_paused {
                windows::resume_thread(thread_id)?;
                paused.remove(&thread_id);
            }
            info!(thread_id, "armed one-instruction trap flag step");
            return Ok(ThreadControlState {
                thread_id,
                paused_by_intimatr: false,
            });
        }
        #[cfg(not(windows))]
        {
            let _ = thread_id;
            Err(DebuggerError::UnsupportedPlatform)
        }
    }

    pub fn set_hardware_breakpoint(
        &self,
        thread_id: u32,
        address: u64,
        kind: HardwareBreakpointKind,
        size: u8,
    ) -> Result<HardwareBreakpoint, DebuggerError> {
        self.ensure_enabled()?;
        validate_breakpoint(address, kind, size)?;
        let _operation = lock(&self.operation_lock)?;
        let paused = lock(&self.paused_threads)?.contains(&thread_id);
        let mut breakpoints = lock(&self.breakpoints)?;
        if breakpoints.len() >= self.config.max_hardware_breakpoints {
            return Err(DebuggerError::LimitExceeded {
                resource: "hardware breakpoints",
                requested: breakpoints.len() + 1,
                limit: self.config.max_hardware_breakpoints,
            });
        }
        let mut used = [false; 4];
        for breakpoint in breakpoints
            .values()
            .filter(|item| item.thread_id == thread_id)
        {
            used[breakpoint.slot as usize] = true;
        }
        let slot = used
            .iter()
            .position(|in_use| !*in_use)
            .ok_or(DebuggerError::NoHardwareBreakpointSlot(thread_id))? as u8;
        let id = self.next_breakpoint_id.fetch_add(1, Ordering::Relaxed);
        let breakpoint = HardwareBreakpoint {
            id,
            thread_id,
            slot,
            address,
            kind,
            size,
        };

        #[cfg(windows)]
        {
            self.ensure_veh()?;
            windows::set_hardware_breakpoint(breakpoint, paused)?;
        }
        #[cfg(not(windows))]
        return Err(DebuggerError::UnsupportedPlatform);

        breakpoints.insert(id, breakpoint);
        info!(
            id,
            thread_id,
            slot,
            address,
            ?kind,
            size,
            "installed hardware breakpoint"
        );
        Ok(breakpoint)
    }

    pub fn remove_hardware_breakpoint(&self, id: u64) -> Result<bool, DebuggerError> {
        self.ensure_enabled()?;
        let _operation = lock(&self.operation_lock)?;
        let paused_threads = lock(&self.paused_threads)?;
        let mut breakpoints = lock(&self.breakpoints)?;
        let Some(breakpoint) = breakpoints.get(&id).copied() else {
            return Ok(false);
        };
        let paused = paused_threads.contains(&breakpoint.thread_id);
        #[cfg(windows)]
        windows::clear_hardware_breakpoint(breakpoint, paused)?;
        #[cfg(not(windows))]
        return Err(DebuggerError::UnsupportedPlatform);
        breakpoints.remove(&id);
        info!(
            id,
            thread_id = breakpoint.thread_id,
            slot = breakpoint.slot,
            "removed hardware breakpoint"
        );
        Ok(true)
    }

    pub fn list_hardware_breakpoints(&self) -> Result<Vec<HardwareBreakpoint>, DebuggerError> {
        let mut items: Vec<_> = lock(&self.breakpoints)?.values().copied().collect();
        items.sort_unstable_by_key(|item| item.id);
        Ok(items)
    }

    pub fn events(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<DebuggerEvent>, DebuggerError> {
        self.ensure_enabled()?;
        if limit == 0 || limit > self.config.max_events_per_poll {
            return Err(DebuggerError::LimitExceeded {
                resource: "debugger event poll",
                requested: limit,
                limit: self.config.max_events_per_poll,
            });
        }
        #[cfg(windows)]
        return Ok(windows::read_events(after_sequence, limit));
        #[cfg(not(windows))]
        {
            let _ = after_sequence;
            Err(DebuggerError::UnsupportedPlatform)
        }
    }

    pub fn status(&self) -> Result<DebuggerStatus, DebuggerError> {
        let _operation = lock(&self.operation_lock)?;
        let mut paused_threads: Vec<_> = lock(&self.paused_threads)?.iter().copied().collect();
        paused_threads.sort_unstable();
        let mut breakpoints: Vec<_> = lock(&self.breakpoints)?.values().copied().collect();
        breakpoints.sort_unstable_by_key(|item| item.id);
        #[cfg(windows)]
        let latest_event_sequence = windows::latest_event_sequence();
        #[cfg(not(windows))]
        let latest_event_sequence = 0;
        Ok(DebuggerStatus {
            paused_threads,
            breakpoints,
            latest_event_sequence,
        })
    }

    pub fn shutdown(&self) {
        let breakpoints = self.list_hardware_breakpoints().unwrap_or_default();
        for breakpoint in breakpoints {
            if let Err(error) = self.remove_hardware_breakpoint(breakpoint.id) {
                warn!(
                    id = breakpoint.id,
                    error = %error,
                    "failed to remove breakpoint during debugger shutdown"
                );
            }
        }
        let paused: Vec<_> = lock(&self.paused_threads)
            .map(|threads| threads.iter().copied().collect())
            .unwrap_or_default();
        for thread_id in paused {
            if let Err(error) = self.resume_thread(thread_id) {
                warn!(thread_id, error = %error, "failed to resume thread during debugger shutdown");
            }
        }
        #[cfg(windows)]
        self.release_veh();
    }

    fn ensure_enabled(&self) -> Result<(), DebuggerError> {
        if self.config.enabled {
            Ok(())
        } else {
            Err(DebuggerError::Disabled)
        }
    }

    #[cfg(windows)]
    fn ensure_veh(&self) -> Result<(), DebuggerError> {
        use std::sync::atomic::Ordering;
        if !self.veh_acquired.swap(true, Ordering::AcqRel) {
            if let Err(error) = windows::acquire_vectored_handler() {
                self.veh_acquired.store(false, Ordering::Release);
                return Err(error.into());
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    fn release_veh(&self) {
        use std::sync::atomic::Ordering;
        if self.veh_acquired.swap(false, Ordering::AcqRel)
            && let Err(error) = windows::release_vectored_handler()
        {
            warn!(error = %error, "failed to remove debugger vectored exception handler");
        }
    }
}

impl Drop for DebuggerCore {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn validate_breakpoint(
    address: u64,
    kind: HardwareBreakpointKind,
    size: u8,
) -> Result<(), DebuggerError> {
    if kind == HardwareBreakpointKind::Execute && size != 1 {
        return Err(DebuggerError::InvalidBreakpoint(
            "execute breakpoints must use size 1",
        ));
    }
    if !matches!(size, 1 | 2 | 4 | 8) {
        return Err(DebuggerError::InvalidBreakpoint(
            "hardware breakpoint size must be 1, 2, 4, or 8",
        ));
    }
    if size > 1 && !address.is_multiple_of(size as u64) {
        return Err(DebuggerError::InvalidBreakpoint(
            "data breakpoint address must be aligned to its size",
        ));
    }
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, DebuggerError> {
    mutex.lock().map_err(|_| DebuggerError::StatePoisoned)
}

#[derive(Debug, Error)]
pub enum DebuggerError {
    #[error("debugger is disabled by configuration")]
    Disabled,
    #[error("debugger is not implemented on this platform")]
    UnsupportedPlatform,
    #[error("unsupported disassembly bitness {0}; expected 16, 32, or 64")]
    InvalidBitness(u32),
    #[error("address 0x{0:X} cannot be represented by this architecture")]
    AddressOutOfRange(u64),
    #[error("address range overflow at 0x{address:X} for {size} bytes")]
    AddressRangeOverflow { address: u64, size: usize },
    #[error("requested {requested} units for {resource}, limit is {limit}")]
    LimitExceeded {
        resource: &'static str,
        requested: usize,
        limit: usize,
    },
    #[error("thread {0} is not paused by Intimatr")]
    ThreadNotPaused(u32),
    #[error("thread {0} has no free x86/x64 hardware breakpoint slots")]
    NoHardwareBreakpointSlot(u32),
    #[error("invalid hardware breakpoint: {0}")]
    InvalidBreakpoint(&'static str),
    #[error("debugger state mutex was poisoned")]
    StatePoisoned,
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[cfg(windows)]
    #[error(transparent)]
    Windows(#[from] windows::WindowsDebuggerError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryRegion, MemorySource};

    struct Bytes(Vec<u8>);

    impl MemorySource for Bytes {
        fn regions(&self) -> Result<Vec<MemoryRegion>, MemoryError> {
            Ok(Vec::new())
        }

        fn read_exact(&self, address: usize, buffer: &mut [u8]) -> Result<(), MemoryError> {
            let end = address + buffer.len();
            buffer.copy_from_slice(&self.0[address..end]);
            Ok(())
        }
    }

    #[test]
    fn disassembles_x64_bytes_with_addresses() {
        let debugger = DebuggerCore::new(DebuggerConfig::default());
        let memory = Bytes(vec![0x48, 0x89, 0xD8, 0xC3]);
        let lines = debugger.disassemble(&memory, 0, 4, 8, 64).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].address, 0);
        assert_eq!(lines[0].bytes, vec![0x48, 0x89, 0xD8]);
        assert!(lines[0].text.contains("mov"));
        assert!(lines[1].text.contains("ret"));
    }

    #[test]
    fn validates_hardware_breakpoint_alignment() {
        assert!(validate_breakpoint(0x1000, HardwareBreakpointKind::Write, 4).is_ok());
        assert!(validate_breakpoint(0x1001, HardwareBreakpointKind::Write, 4).is_err());
        assert!(validate_breakpoint(0x1000, HardwareBreakpointKind::Execute, 4).is_err());
    }
}
