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

use crate::{
    config::{PolicyConfig, ScannerConfig},
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
    pub registers: Vec<RegisterValue>,
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
        Self {
            memory,
            scanner_config,
            policy,
            limits,
            scans: Mutex::new(HashMap::new()),
            active_scans: Mutex::new(HashMap::new()),
            watches: Mutex::new(HashMap::new()),
            next_scan_id: AtomicU64::new(1),
            next_watch_id: AtomicU64::new(1),
        }
    }

    pub fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {
        let command_name = command.name();
        debug!(command = command_name, "dispatching frontend command");

        let execution = match command {
            Command::Ping => CommandExecution::immediate(CommandResult::Pong),
            Command::LifecycleState => {
                let state = crate::runtime::lifecycle_state();
                CommandExecution::immediate(CommandResult::Lifecycle {
                    state: format!("{state:?}").to_ascii_lowercase(),
                    shutdown_requested: crate::runtime::shutdown_requested(),
                })
            }
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
                let address_native = address_to_usize(address)?;
                let value = read_scalar(&self.memory, address_native, value_type)?;
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
                let address_native = address_to_usize(address)?;
                write_scalar(
                    &self.memory,
                    address_native,
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
                let scan_result = first_scan(
                    &self.memory,
                    value_type,
                    predicate,
                    ScanOptions::from(&self.scanner_config),
                    &cancellation,
                );
                self.remove_active_scan(scan_id)?;
                let session = scan_result?;
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
                let scan_result = previous.next_scan(
                    &self.memory,
                    predicate,
                    ScanOptions::from(&self.scanner_config),
                    &cancellation,
                );
                self.remove_active_scan(scan_id)?;
                let session = scan_result?;
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
                    watch
                        .value_type
                        .encode(value)
                        .map_err(MemoryError::from)?;
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
                {
                    return Err(CommandError::NotImplemented("module enumeration"));
                }
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
                {
                    return Err(CommandError::NotImplemented("thread enumeration"));
                }
            }
            Command::ReadThreadRegisters { .. } => {
                self.require_debugger()?;
                return Err(CommandError::NotImplemented("debugger register inspection"));
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
        }
    }
}
