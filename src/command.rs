use std::{
    collections::HashMap,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

pub use crate::debugger::{
    DebuggerEvent, DebuggerEventKind, DebuggerStatus, DisassemblyLine, HardwareBreakpoint,
    HardwareBreakpointKind, RegisterSnapshot, RegisterValue, ThreadControlState,
};
use crate::{
    config::{DebuggerConfig, PolicyConfig, ScannerConfig},
    debugger::{DebuggerCore, DebuggerError},
    memory::{MemoryError, MemoryTarget, WritePolicy, read_scalar, write_scalar},
    scanner::{
        CancellationToken, ScalarValue, ScanCandidate, ScanError, ScanOptions, ScanPredicate,
        ScanSession, ScanStats, ValueType, first_scan,
    },
};

#[cfg(windows)]
use crate::platform::windows::WindowsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandLimits {
    pub max_memory_transfer_bytes: usize,
    pub max_scan_results_per_page: usize,
}

impl Default for CommandLimits {
    fn default() -> Self {
        Self {
            max_memory_transfer_bytes: 256 * 1024,
            max_scan_results_per_page: 4096,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Ping,
    LifecycleState,
    ListMemoryRegions,
    ReadMemory {
        address: u64,
        size: usize,
    },
    ReadScalar {
        address: u64,
        value_type: ValueType,
    },
    WriteMemory {
        address: u64,
        bytes: Vec<u8>,
    },
    WriteScalar {
        address: u64,
        value_type: ValueType,
        value: ScalarValue,
    },
    FirstScan {
        value_type: ValueType,
        predicate: ScanPredicate,
    },
    NextScan {
        scan_id: u64,
        predicate: ScanPredicate,
    },
    ScanSummary {
        scan_id: u64,
    },
    ScanResults {
        scan_id: u64,
        offset: usize,
        limit: usize,
    },
    CancelScan {
        scan_id: u64,
    },
    DeleteScan {
        scan_id: u64,
    },
    AddWatch {
        address: u64,
        value_type: ValueType,
        label: Option<String>,
    },
    SetWatchFreeze {
        watch_id: u64,
        value: Option<ScalarValue>,
    },
    RemoveWatch {
        watch_id: u64,
    },
    ListWatches,
    RefreshWatches,
    ListModules,
    ListThreads,
    ReadThreadRegisters {
        thread_id: u32,
    },
    Disassemble {
        address: u64,
        byte_count: usize,
        max_instructions: usize,
        bitness: u32,
    },
    DebuggerStatus,
    PauseThread {
        thread_id: u32,
    },
    ResumeThread {
        thread_id: u32,
    },
    SingleStepThread {
        thread_id: u32,
    },
    SetHardwareBreakpoint {
        thread_id: u32,
        address: u64,
        kind: HardwareBreakpointKind,
        size: u8,
    },
    RemoveHardwareBreakpoint {
        breakpoint_id: u64,
    },
    ListHardwareBreakpoints,
    DebuggerEvents {
        after_sequence: u64,
        limit: usize,
    },
    Analysis {
        request: crate::analysis::AnalysisCommand,
    },
    Shutdown,
}

impl Command {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::LifecycleState => "lifecycle_state",
            Self::ListMemoryRegions => "list_memory_regions",
            Self::ReadMemory { .. } => "read_memory",
            Self::ReadScalar { .. } => "read_scalar",
            Self::WriteMemory { .. } => "write_memory",
            Self::WriteScalar { .. } => "write_scalar",
            Self::FirstScan { .. } => "first_scan",
            Self::NextScan { .. } => "next_scan",
            Self::ScanSummary { .. } => "scan_summary",
            Self::ScanResults { .. } => "scan_results",
            Self::CancelScan { .. } => "cancel_scan",
            Self::DeleteScan { .. } => "delete_scan",
            Self::AddWatch { .. } => "add_watch",
            Self::SetWatchFreeze { .. } => "set_watch_freeze",
            Self::RemoveWatch { .. } => "remove_watch",
            Self::ListWatches => "list_watches",
            Self::RefreshWatches => "refresh_watches",
            Self::ListModules => "list_modules",
            Self::ListThreads => "list_threads",
            Self::ReadThreadRegisters { .. } => "read_thread_registers",
            Self::Disassemble { .. } => "disassemble",
            Self::DebuggerStatus => "debugger_status",
            Self::PauseThread { .. } => "pause_thread",
            Self::ResumeThread { .. } => "resume_thread",
            Self::SingleStepThread { .. } => "single_step_thread",
            Self::SetHardwareBreakpoint { .. } => "set_hardware_breakpoint",
            Self::RemoveHardwareBreakpoint { .. } => "remove_hardware_breakpoint",
            Self::ListHardwareBreakpoints => "list_hardware_breakpoints",
            Self::DebuggerEvents { .. } => "debugger_events",
            Self::Analysis { .. } => "analysis",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CommandResult {
    Pong,
    Lifecycle {
        state: String,
        shutdown_requested: bool,
    },
    MemoryRegions {
        regions: Vec<MemoryRegionInfo>,
    },
    MemoryBytes {
        address: u64,
        bytes: Vec<u8>,
    },
    Scalar {
        address: u64,
        value_type: ValueType,
        value: ScalarValue,
    },
    WriteComplete {
        address: u64,
        size: usize,
    },
    Scan {
        summary: ScanSummary,
    },
    ScanResults {
        scan_id: u64,
        offset: usize,
        total: usize,
        candidates: Vec<ScanCandidateInfo>,
    },
    ScanCancellation {
        scan_id: u64,
        was_active: bool,
    },
    ScanDeleted {
        scan_id: u64,
        existed: bool,
    },
    WatchAdded {
        watch: WatchDefinition,
    },
    WatchUpdated {
        watch: WatchDefinition,
    },
    WatchRemoved {
        watch_id: u64,
        existed: bool,
    },
    Watches {
        watches: Vec<WatchDefinition>,
    },
    WatchValues {
        values: Vec<WatchValue>,
    },
    Modules {
        modules: Vec<ModuleInfo>,
    },
    Threads {
        threads: Vec<ThreadInfo>,
    },
    ThreadRegisters {
        registers: RegisterSnapshot,
    },
    Disassembly {
        address: u64,
        bitness: u32,
        lines: Vec<DisassemblyLine>,
    },
    DebuggerStatus {
        status: DebuggerStatus,
    },
    ThreadControl {
        state: ThreadControlState,
    },
    HardwareBreakpointAdded {
        breakpoint: HardwareBreakpoint,
    },
    HardwareBreakpointRemoved {
        breakpoint_id: u64,
        existed: bool,
    },
    HardwareBreakpoints {
        breakpoints: Vec<HardwareBreakpoint>,
    },
    DebuggerEvents {
        events: Vec<DebuggerEvent>,
        latest_sequence: u64,
    },
    Analysis {
        analysis: crate::analysis::AnalysisResult,
    },
    ShutdownAccepted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRegionInfo {
    pub base: u64,
    pub size: u64,
    pub committed: bool,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    pub guard: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScanCandidateInfo {
    pub address: u64,
    pub current: ScalarValue,
    pub previous: Option<ScalarValue>,
}

impl From<&ScanCandidate> for ScanCandidateInfo {
    fn from(candidate: &ScanCandidate) -> Self {
        Self {
            address: candidate.address as u64,
            current: candidate.current,
            previous: candidate.previous,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScanSummary {
    pub scan_id: u64,
    pub value_type: ValueType,
    pub result_count: usize,
    pub stats: ScanStats,
}

impl ScanSummary {
    fn from_session(scan_id: u64, session: &ScanSession) -> Self {
        Self {
            scan_id,
            value_type: session.value_type,
            result_count: session.candidates.len(),
            stats: session.stats,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchDefinition {
    pub id: u64,
    pub address: u64,
    pub value_type: ValueType,
    pub label: Option<String>,
    pub frozen: Option<ScalarValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchValue {
    pub watch: WatchDefinition,
    pub value: Option<ScalarValue>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub name: String,
    pub path: String,
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadInfo {
    pub thread_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostAction {
    Shutdown,
}

#[derive(Debug)]
pub struct CommandExecution {
    pub result: CommandResult,
    pub post_action: Option<PostAction>,
}

impl CommandExecution {
    fn immediate(result: CommandResult) -> Self {
        Self {
            result,
            post_action: None,
        }
    }
}

pub trait CommandExecutor: Send + Sync {
    fn execute(&self, command: Command) -> Result<CommandExecution, CommandError>;
    fn shutdown(&self) {}
}

pub struct CommandDispatcher<M> {
    memory: M,
    scanner_config: ScannerConfig,
    policy: PolicyConfig,
    limits: CommandLimits,
    debugger: DebuggerCore,
    analysis: Mutex<crate::analysis::AnalysisWorkspace>,
    analysis_directory: Option<std::path::PathBuf>,
    scans: Mutex<HashMap<u64, ScanSession>>,
    active_scans: Mutex<HashMap<u64, CancellationToken>>,
    watches: Mutex<HashMap<u64, WatchDefinition>>,
    next_scan_id: AtomicU64,
    next_watch_id: AtomicU64,
}

impl<M> CommandDispatcher<M>
where
    M: MemoryTarget + Send + Sync,
{
    pub fn new(
        memory: M,
        scanner_config: ScannerConfig,
        policy: PolicyConfig,
        limits: CommandLimits,
    ) -> Self {
        Self::new_with_debugger(
            memory,
            scanner_config,
            DebuggerConfig::default(),
            policy,
            limits,
        )
    }

    pub fn new_with_debugger(
        memory: M,
        scanner_config: ScannerConfig,
        debugger_config: DebuggerConfig,
        policy: PolicyConfig,
        limits: CommandLimits,
    ) -> Self {
        Self {
            memory,
            scanner_config,
            policy,
            limits,
            debugger: DebuggerCore::new(debugger_config),
            analysis: Mutex::new(crate::analysis::AnalysisWorkspace::default()),
            analysis_directory: None,
            scans: Mutex::new(HashMap::new()),
            active_scans: Mutex::new(HashMap::new()),
            watches: Mutex::new(HashMap::new()),
            next_scan_id: AtomicU64::new(1),
            next_watch_id: AtomicU64::new(1),
        }
    }

    pub fn with_analysis_directory(mut self, directory: std::path::PathBuf) -> Self {
        self.analysis_directory = Some(directory);
        self
    }

    pub fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {
        let command_name = command.name();
        debug!(command = command_name, "dispatching frontend command");

        let execution = match command {
            Command::Ping => CommandExecution::immediate(CommandResult::Pong),
            Command::LifecycleState => CommandExecution::immediate(CommandResult::Lifecycle {
                state: format!("{:?}", crate::runtime::lifecycle_state()).to_ascii_lowercase(),
                shutdown_requested: crate::runtime::shutdown_requested(),
            }),
            Command::ListMemoryRegions => {
                self.require_memory_read()?;
                let regions = self
                    .memory
                    .regions()?
                    .into_iter()
                    .map(|region| MemoryRegionInfo {
                        base: region.base as u64,
                        size: region.size as u64,
                        committed: region.committed,
                        readable: region.readable,
                        writable: region.writable,
                        executable: region.executable,
                        guard: region.guard,
                    })
                    .collect();
                CommandExecution::immediate(CommandResult::MemoryRegions { regions })
            }
            Command::ReadMemory { address, size } => {
                self.require_memory_read()?;
                self.require_transfer_size(size)?;
                let address_native = address_to_usize(address)?;
                ensure_address_range(address_native, size)?;
                let mut bytes = vec![0_u8; size];
                self.memory.read_exact(address_native, &mut bytes)?;
                CommandExecution::immediate(CommandResult::MemoryBytes { address, bytes })
            }
            Command::ReadScalar {
                address,
                value_type,
            } => {
                self.require_memory_read()?;
                let value = read_scalar(&self.memory, address_to_usize(address)?, value_type)?;
                CommandExecution::immediate(CommandResult::Scalar {
                    address,
                    value_type,
                    value,
                })
            }
            Command::WriteMemory { address, bytes } => {
                self.require_memory_write()?;
                self.require_transfer_size(bytes.len())?;
                let address_native = address_to_usize(address)?;
                ensure_address_range(address_native, bytes.len())?;
                self.memory
                    .write_exact(address_native, &bytes, WritePolicy::from(&self.policy))?;
                CommandExecution::immediate(CommandResult::WriteComplete {
                    address,
                    size: bytes.len(),
                })
            }
            Command::WriteScalar {
                address,
                value_type,
                value,
            } => {
                self.require_memory_write()?;
                write_scalar(
                    &self.memory,
                    address_to_usize(address)?,
                    value_type,
                    value,
                    WritePolicy::from(&self.policy),
                )?;
                CommandExecution::immediate(CommandResult::WriteComplete {
                    address,
                    size: value_type.byte_width(),
                })
            }
            Command::FirstScan {
                value_type,
                predicate,
            } => {
                self.require_memory_read()?;
                let scan_id = self.next_scan_id.fetch_add(1, Ordering::Relaxed);
                let cancellation = CancellationToken::new();
                self.register_active_scan(scan_id, cancellation.clone())?;
                let result = first_scan(
                    &self.memory,
                    value_type,
                    predicate,
                    ScanOptions::from(&self.scanner_config),
                    &cancellation,
                );
                self.remove_active_scan(scan_id)?;
                let session = result?;
                let summary = ScanSummary::from_session(scan_id, &session);
                lock(&self.scans)?.insert(scan_id, session);
                CommandExecution::immediate(CommandResult::Scan { summary })
            }
            Command::NextScan { scan_id, predicate } => {
                self.require_memory_read()?;
                let previous = lock(&self.scans)?
                    .get(&scan_id)
                    .cloned()
                    .ok_or(CommandError::ScanNotFound(scan_id))?;
                let cancellation = CancellationToken::new();
                self.register_active_scan(scan_id, cancellation.clone())?;
                let result = previous.next_scan(
                    &self.memory,
                    predicate,
                    ScanOptions::from(&self.scanner_config),
                    &cancellation,
                );
                self.remove_active_scan(scan_id)?;
                let session = result?;
                let summary = ScanSummary::from_session(scan_id, &session);
                lock(&self.scans)?.insert(scan_id, session);
                CommandExecution::immediate(CommandResult::Scan { summary })
            }
            Command::ScanSummary { scan_id } => {
                let scans = lock(&self.scans)?;
                let session = scans
                    .get(&scan_id)
                    .ok_or(CommandError::ScanNotFound(scan_id))?;
                CommandExecution::immediate(CommandResult::Scan {
                    summary: ScanSummary::from_session(scan_id, session),
                })
            }
            Command::ScanResults {
                scan_id,
                offset,
                limit,
            } => {
                if limit > self.limits.max_scan_results_per_page {
                    return Err(CommandError::LimitExceeded {
                        resource: "scan result page",
                        requested: limit,
                        limit: self.limits.max_scan_results_per_page,
                    });
                }
                let scans = lock(&self.scans)?;
                let session = scans
                    .get(&scan_id)
                    .ok_or(CommandError::ScanNotFound(scan_id))?;
                let total = session.candidates.len();
                let start = offset.min(total);
                let end = start.saturating_add(limit).min(total);
                let candidates = session.candidates[start..end]
                    .iter()
                    .map(ScanCandidateInfo::from)
                    .collect();
                CommandExecution::immediate(CommandResult::ScanResults {
                    scan_id,
                    offset: start,
                    total,
                    candidates,
                })
            }
            Command::CancelScan { scan_id } => {
                let cancellation = lock(&self.active_scans)?.get(&scan_id).cloned();
                let was_active = cancellation.is_some();
                if let Some(cancellation) = cancellation {
                    cancellation.cancel();
                    info!(scan_id, "scan cancellation requested");
                }
                CommandExecution::immediate(CommandResult::ScanCancellation {
                    scan_id,
                    was_active,
                })
            }
            Command::DeleteScan { scan_id } => {
                if lock(&self.active_scans)?.contains_key(&scan_id) {
                    return Err(CommandError::ScanBusy(scan_id));
                }
                let existed = lock(&self.scans)?.remove(&scan_id).is_some();
                CommandExecution::immediate(CommandResult::ScanDeleted { scan_id, existed })
            }
            Command::AddWatch {
                address,
                value_type,
                label,
            } => {
                address_to_usize(address)?;
                let id = self.next_watch_id.fetch_add(1, Ordering::Relaxed);
                let watch = WatchDefinition {
                    id,
                    address,
                    value_type,
                    label,
                    frozen: None,
                };
                lock(&self.watches)?.insert(id, watch.clone());
                CommandExecution::immediate(CommandResult::WatchAdded { watch })
            }
            Command::SetWatchFreeze { watch_id, value } => {
                if value.is_some() {
                    self.require_memory_write()?;
                }
                let mut watches = lock(&self.watches)?;
                let watch = watches
                    .get_mut(&watch_id)
                    .ok_or(CommandError::WatchNotFound(watch_id))?;
                if let Some(value) = value {
                    watch.value_type.encode(value).map_err(MemoryError::from)?;
                }
                watch.frozen = value;
                CommandExecution::immediate(CommandResult::WatchUpdated {
                    watch: watch.clone(),
                })
            }
            Command::RemoveWatch { watch_id } => {
                let existed = lock(&self.watches)?.remove(&watch_id).is_some();
                CommandExecution::immediate(CommandResult::WatchRemoved { watch_id, existed })
            }
            Command::ListWatches => {
                let mut watches: Vec<_> = lock(&self.watches)?.values().cloned().collect();
                watches.sort_unstable_by_key(|watch| watch.id);
                CommandExecution::immediate(CommandResult::Watches { watches })
            }
            Command::RefreshWatches => {
                self.require_memory_read()?;
                let mut watches: Vec<_> = lock(&self.watches)?.values().cloned().collect();
                watches.sort_unstable_by_key(|watch| watch.id);
                let values = watches
                    .into_iter()
                    .map(|watch| self.refresh_watch(watch))
                    .collect();
                CommandExecution::immediate(CommandResult::WatchValues { values })
            }
            Command::ListModules => {
                self.require_memory_read()?;
                #[cfg(windows)]
                {
                    let modules = crate::platform::windows::loaded_modules()?
                        .into_iter()
                        .map(|module| ModuleInfo {
                            name: module.name,
                            path: module.path,
                            base: module.base,
                            size: module.size,
                        })
                        .collect();
                    CommandExecution::immediate(CommandResult::Modules { modules })
                }
                #[cfg(not(windows))]
                return Err(CommandError::NotImplemented("module enumeration"));
            }
            Command::ListThreads => {
                self.require_debugger()?;
                #[cfg(windows)]
                {
                    let threads = crate::platform::windows::current_process_threads()?
                        .into_iter()
                        .map(|thread_id| ThreadInfo { thread_id })
                        .collect();
                    CommandExecution::immediate(CommandResult::Threads { threads })
                }
                #[cfg(not(windows))]
                return Err(CommandError::NotImplemented("thread enumeration"));
            }
            Command::ReadThreadRegisters { thread_id } => {
                self.require_debugger()?;
                let registers = self.debugger.read_registers(thread_id)?;
                CommandExecution::immediate(CommandResult::ThreadRegisters { registers })
            }
            Command::Disassemble {
                address,
                byte_count,
                max_instructions,
                bitness,
            } => {
                self.require_debugger()?;
                self.require_memory_read()?;
                self.require_transfer_size(byte_count)?;
                let lines = self.debugger.disassemble(
                    &self.memory,
                    address,
                    byte_count,
                    max_instructions,
                    bitness,
                )?;
                CommandExecution::immediate(CommandResult::Disassembly {
                    address,
                    bitness,
                    lines,
                })
            }
            Command::DebuggerStatus => {
                self.require_debugger()?;
                CommandExecution::immediate(CommandResult::DebuggerStatus {
                    status: self.debugger.status()?,
                })
            }
            Command::PauseThread { thread_id } => {
                self.require_debugger()?;
                CommandExecution::immediate(CommandResult::ThreadControl {
                    state: self.debugger.pause_thread(thread_id)?,
                })
            }
            Command::ResumeThread { thread_id } => {
                self.require_debugger()?;
                CommandExecution::immediate(CommandResult::ThreadControl {
                    state: self.debugger.resume_thread(thread_id)?,
                })
            }
            Command::SingleStepThread { thread_id } => {
                self.require_debugger()?;
                CommandExecution::immediate(CommandResult::ThreadControl {
                    state: self.debugger.single_step_thread(thread_id)?,
                })
            }
            Command::SetHardwareBreakpoint {
                thread_id,
                address,
                kind,
                size,
            } => {
                self.require_debugger()?;
                let breakpoint = self
                    .debugger
                    .set_hardware_breakpoint(thread_id, address, kind, size)?;
                CommandExecution::immediate(CommandResult::HardwareBreakpointAdded { breakpoint })
            }
            Command::RemoveHardwareBreakpoint { breakpoint_id } => {
                self.require_debugger()?;
                let existed = self.debugger.remove_hardware_breakpoint(breakpoint_id)?;
                CommandExecution::immediate(CommandResult::HardwareBreakpointRemoved {
                    breakpoint_id,
                    existed,
                })
            }
            Command::ListHardwareBreakpoints => {
                self.require_debugger()?;
                CommandExecution::immediate(CommandResult::HardwareBreakpoints {
                    breakpoints: self.debugger.list_hardware_breakpoints()?,
                })
            }
            Command::DebuggerEvents {
                after_sequence,
                limit,
            } => {
                self.require_debugger()?;
                let events = self.debugger.events(after_sequence, limit)?;
                let latest_sequence = self.debugger.status()?.latest_event_sequence;
                CommandExecution::immediate(CommandResult::DebuggerEvents {
                    events,
                    latest_sequence,
                })
            }
            Command::Analysis { request } => {
                let result = self.execute_analysis(request)?;
                CommandExecution::immediate(CommandResult::Analysis { analysis: result })
            }
            Command::Shutdown => {
                if !self.policy.allow_remote_shutdown {
                    return Err(CommandError::PolicyDenied {
                        capability: "remote shutdown",
                    });
                }
                CommandExecution {
                    result: CommandResult::ShutdownAccepted,
                    post_action: Some(PostAction::Shutdown),
                }
            }
        };

        debug!(command = command_name, "frontend command completed");
        Ok(execution)
    }

    fn execute_analysis(
        &self,
        request: crate::analysis::AnalysisCommand,
    ) -> Result<crate::analysis::AnalysisResult, CommandError> {
        use crate::analysis::{
            AddressExpression, AnalysisCommand, AnalysisResult, ModuleDescriptor,
            PatternScanOptions, SavedWatchTemplate, inspect_structure, resolve_pointer_chain,
            scan_pattern, search_pointer_chains, validate_workspace_name,
        };

        match request {
            AnalysisCommand::AobScan {
                pattern,
                alignment,
                max_results,
            } => {
                self.require_memory_read()?;
                let scan = scan_pattern(
                    &self.memory,
                    &pattern,
                    &self.scanner_config,
                    PatternScanOptions {
                        alignment,
                        max_results,
                    },
                )?;
                Ok(AnalysisResult::PatternScan { scan })
            }
            AnalysisCommand::ResolveAddress { expression } => {
                let address = match AddressExpression::parse(&expression)? {
                    AddressExpression::Absolute { address } => address,
                    relative @ AddressExpression::ModuleOffset { .. } => {
                        self.require_memory_read()?;
                        let modules = self.analysis_modules()?;
                        relative.resolve(&modules)?
                    }
                };
                Ok(AnalysisResult::Address {
                    expression,
                    address,
                })
            }
            AnalysisCommand::ResolvePointerChain { spec } => {
                self.require_memory_read()?;
                let modules = self.analysis_modules()?;
                let resolution = resolve_pointer_chain(&self.memory, &modules, &spec)?;
                Ok(AnalysisResult::PointerChain { resolution })
            }
            AnalysisCommand::SearchPointerChains { target, options } => {
                self.require_memory_read()?;
                let paths =
                    search_pointer_chains(&self.memory, target, &self.scanner_config, options)?;
                Ok(AnalysisResult::PointerPaths { paths })
            }
            AnalysisCommand::InspectStructure { base, fields } => {
                self.require_memory_read()?;
                let modules = self.analysis_modules()?;
                let fields = inspect_structure(
                    &self.memory,
                    &modules,
                    &base,
                    &fields,
                    self.limits.max_memory_transfer_bytes,
                )?;
                Ok(AnalysisResult::Structure { fields })
            }
            AnalysisCommand::SaveScan { scan_id, name } => {
                let session = lock(&self.scans)?
                    .get(&scan_id)
                    .cloned()
                    .ok_or(CommandError::ScanNotFound(scan_id))?;
                lock(&self.analysis)?.save_scan(name.clone(), session)?;
                Ok(AnalysisResult::ScanSaved { name })
            }
            AnalysisCommand::RestoreScan { name } => {
                let session = lock(&self.analysis)?.scan(&name)?;
                let scan_id = self.next_scan_id.fetch_add(1, Ordering::Relaxed);
                lock(&self.scans)?.insert(scan_id, session);
                Ok(AnalysisResult::ScanRestored { name, scan_id })
            }
            AnalysisCommand::SaveWatchTemplate { watch_id, name } => {
                let watch = lock(&self.watches)?
                    .get(&watch_id)
                    .cloned()
                    .ok_or(CommandError::WatchNotFound(watch_id))?;
                let modules = self.analysis_modules()?;
                let address = modules
                    .iter()
                    .find(|module| {
                        watch.address >= module.base
                            && watch.address < module.base.saturating_add(module.size)
                    })
                    .map_or_else(
                        || format!("0x{:X}", watch.address),
                        |module| {
                            format!(
                                "{}+0x{:X}",
                                module.name,
                                watch.address.saturating_sub(module.base)
                            )
                        },
                    );
                let template = SavedWatchTemplate {
                    name: name.clone(),
                    address,
                    value_type: watch.value_type,
                    frozen: watch.frozen,
                };
                lock(&self.analysis)?.save_watch_template(template)?;
                Ok(AnalysisResult::WatchTemplateSaved { name })
            }
            AnalysisCommand::AddWatchFromTemplate { name, label } => {
                let template = lock(&self.analysis)?.watch_template(&name)?;
                let modules: Vec<ModuleDescriptor> = self.analysis_modules()?;
                let address = AddressExpression::parse(&template.address)?.resolve(&modules)?;
                address_to_usize(address)?;
                if let Some(value) = template.frozen {
                    self.require_memory_write()?;
                    template
                        .value_type
                        .encode(value)
                        .map_err(MemoryError::from)?;
                }
                let watch_id = self.next_watch_id.fetch_add(1, Ordering::Relaxed);
                lock(&self.watches)?.insert(
                    watch_id,
                    WatchDefinition {
                        id: watch_id,
                        address,
                        value_type: template.value_type,
                        label: label.or_else(|| Some(name.clone())),
                        frozen: template.frozen,
                    },
                );
                Ok(AnalysisResult::WatchAdded { name, watch_id })
            }
            AnalysisCommand::ListSaved => Ok(AnalysisResult::Saved {
                summary: lock(&self.analysis)?.summary(),
            }),
            AnalysisCommand::SaveWorkspace { name } => {
                validate_workspace_name(&name)?;
                let path = self.analysis_workspace_path(&name)?;
                lock(&self.analysis)?.save_to_path(&path)?;
                Ok(AnalysisResult::WorkspaceSaved { name })
            }
            AnalysisCommand::LoadWorkspace { name } => {
                validate_workspace_name(&name)?;
                let path = self.analysis_workspace_path(&name)?;
                let mut workspace = lock(&self.analysis)?;
                workspace.load_from_path(&path)?;
                Ok(AnalysisResult::WorkspaceLoaded {
                    name,
                    summary: workspace.summary(),
                })
            }
            AnalysisCommand::Batch { commands } => {
                if commands.is_empty() || commands.len() > 128 {
                    return Err(crate::analysis::AnalysisError::InvalidLimit(
                        "analysis batch command count",
                    )
                    .into());
                }
                if commands
                    .iter()
                    .any(|command| matches!(command, AnalysisCommand::Batch { .. }))
                {
                    return Err(crate::analysis::AnalysisError::InvalidLimit(
                        "nested analysis batch",
                    )
                    .into());
                }
                let mut results = Vec::with_capacity(commands.len());
                for command in commands {
                    results.push(self.execute_analysis(command)?);
                }
                Ok(AnalysisResult::Batch { results })
            }
        }
    }

    fn analysis_workspace_path(&self, name: &str) -> Result<std::path::PathBuf, CommandError> {
        let directory = self
            .analysis_directory
            .as_ref()
            .ok_or(crate::analysis::AnalysisError::WorkspaceStorageUnavailable)?;
        Ok(directory.join(format!("{name}.json")))
    }

    fn analysis_modules(&self) -> Result<Vec<crate::analysis::ModuleDescriptor>, CommandError> {
        #[cfg(windows)]
        {
            Ok(crate::platform::windows::loaded_modules()?
                .into_iter()
                .map(|module| crate::analysis::ModuleDescriptor {
                    name: module.name,
                    path: module.path,
                    base: module.base,
                    size: module.size,
                })
                .collect())
        }
        #[cfg(not(windows))]
        {
            Ok(Vec::new())
        }
    }

    pub fn cancel_all_scans(&self) -> Result<usize, CommandError> {
        let cancellations: Vec<_> = lock(&self.active_scans)?.values().cloned().collect();
        for cancellation in &cancellations {
            cancellation.cancel();
        }
        if !cancellations.is_empty() {
            warn!(
                count = cancellations.len(),
                "cancelled active scans during command shutdown"
            );
        }
        Ok(cancellations.len())
    }

    fn refresh_watch(&self, watch: WatchDefinition) -> WatchValue {
        let result = address_to_usize(watch.address).and_then(|address| {
            if let Some(frozen) = watch.frozen {
                self.require_memory_write()?;
                write_scalar(
                    &self.memory,
                    address,
                    watch.value_type,
                    frozen,
                    WritePolicy::from(&self.policy),
                )?;
            }
            read_scalar(&self.memory, address, watch.value_type).map_err(CommandError::from)
        });
        match result {
            Ok(value) => WatchValue {
                watch,
                value: Some(value),
                error: None,
            },
            Err(error) => WatchValue {
                watch,
                value: None,
                error: Some(error.to_string()),
            },
        }
    }

    fn register_active_scan(
        &self,
        scan_id: u64,
        cancellation: CancellationToken,
    ) -> Result<(), CommandError> {
        let mut active = lock(&self.active_scans)?;
        if active.contains_key(&scan_id) {
            return Err(CommandError::ScanBusy(scan_id));
        }
        active.insert(scan_id, cancellation);
        Ok(())
    }

    fn remove_active_scan(&self, scan_id: u64) -> Result<(), CommandError> {
        lock(&self.active_scans)?.remove(&scan_id);
        Ok(())
    }

    fn require_memory_read(&self) -> Result<(), CommandError> {
        if self.policy.allow_memory_read {
            Ok(())
        } else {
            Err(CommandError::PolicyDenied {
                capability: "memory read",
            })
        }
    }
    fn require_memory_write(&self) -> Result<(), CommandError> {
        if self.policy.allow_memory_write {
            Ok(())
        } else {
            Err(CommandError::PolicyDenied {
                capability: "memory write",
            })
        }
    }
    fn require_debugger(&self) -> Result<(), CommandError> {
        if self.policy.allow_debugger {
            Ok(())
        } else {
            Err(CommandError::PolicyDenied {
                capability: "debugger",
            })
        }
    }
    fn require_transfer_size(&self, requested: usize) -> Result<(), CommandError> {
        if requested <= self.limits.max_memory_transfer_bytes {
            Ok(())
        } else {
            Err(CommandError::LimitExceeded {
                resource: "memory transfer",
                requested,
                limit: self.limits.max_memory_transfer_bytes,
            })
        }
    }
}

impl<M> CommandExecutor for CommandDispatcher<M>
where
    M: MemoryTarget + Send + Sync,
{
    fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {
        CommandDispatcher::execute(self, command)
    }

    fn shutdown(&self) {
        if let Err(error) = self.cancel_all_scans() {
            warn!(error = %error, "failed to cancel active scans during command executor shutdown");
        }
        self.debugger.shutdown();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, CommandError> {
    mutex.lock().map_err(|_| CommandError::StatePoisoned)
}
fn address_to_usize(address: u64) -> Result<usize, CommandError> {
    usize::try_from(address).map_err(|_| CommandError::AddressOutOfRange(address))
}
fn ensure_address_range(address: usize, size: usize) -> Result<(), CommandError> {
    address
        .checked_add(size)
        .ok_or(MemoryError::AddressRangeOverflow { address, size })?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("{capability} is disabled by policy")]
    PolicyDenied { capability: &'static str },
    #[error("requested {requested} units for {resource}, limit is {limit}")]
    LimitExceeded {
        resource: &'static str,
        requested: usize,
        limit: usize,
    },
    #[error("address 0x{0:X} cannot be represented by this process architecture")]
    AddressOutOfRange(u64),
    #[error("scan {0} does not exist")]
    ScanNotFound(u64),
    #[error("scan {0} already has an active operation")]
    ScanBusy(u64),
    #[error("watch {0} does not exist")]
    WatchNotFound(u64),
    #[error("shared command state mutex was poisoned")]
    StatePoisoned,
    #[error("{0} is defined in the command contract but is not implemented yet")]
    NotImplemented(&'static str),
    #[cfg(windows)]
    #[error(transparent)]
    Windows(#[from] WindowsError),
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error(transparent)]
    Scan(#[from] ScanError),
    #[error(transparent)]
    Debugger(#[from] DebuggerError),
    #[error(transparent)]
    Analysis(#[from] crate::analysis::AnalysisError),
}

impl CommandError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PolicyDenied { .. } => "policy_denied",
            Self::LimitExceeded { .. } => "limit_exceeded",
            Self::AddressOutOfRange(_) => "address_out_of_range",
            Self::ScanNotFound(_) => "scan_not_found",
            Self::ScanBusy(_) => "scan_busy",
            Self::WatchNotFound(_) => "watch_not_found",
            Self::StatePoisoned => "state_poisoned",
            Self::NotImplemented(_) => "not_implemented",
            #[cfg(windows)]
            Self::Windows(_) => "platform_error",
            Self::Memory(_) => "memory_error",
            Self::Scan(_) => "scan_error",
            Self::Debugger(_) => "debugger_error",
            Self::Analysis(_) => "analysis_error",
        }
    }
}
