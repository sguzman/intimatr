from pathlib import Path

path = Path("src/command.rs")
text = path.read_text()

replacements = [
    (
        "    DebuggerEvents {\n        after_sequence: u64,\n        limit: usize,\n    },\n    Shutdown,",
        "    DebuggerEvents {\n        after_sequence: u64,\n        limit: usize,\n    },\n    Analysis {\n        request: crate::analysis::AnalysisCommand,\n    },\n    Shutdown,",
    ),
    (
        '            Self::DebuggerEvents { .. } => "debugger_events",\n            Self::Shutdown => "shutdown",',
        '            Self::DebuggerEvents { .. } => "debugger_events",\n            Self::Analysis { .. } => "analysis",\n            Self::Shutdown => "shutdown",',
    ),
    (
        "    DebuggerEvents {\n        events: Vec<DebuggerEvent>,\n        latest_sequence: u64,\n    },\n    ShutdownAccepted,",
        "    DebuggerEvents {\n        events: Vec<DebuggerEvent>,\n        latest_sequence: u64,\n    },\n    Analysis {\n        result: crate::analysis::AnalysisResult,\n    },\n    ShutdownAccepted,",
    ),
    (
        "    debugger: DebuggerCore,\n    scans: Mutex<HashMap<u64, ScanSession>>,",
        "    debugger: DebuggerCore,\n    analysis: Mutex<crate::analysis::AnalysisWorkspace>,\n    analysis_directory: Option<std::path::PathBuf>,\n    scans: Mutex<HashMap<u64, ScanSession>>,","
    ),
    (
        "            debugger: DebuggerCore::new(debugger_config),\n            scans: Mutex::new(HashMap::new()),",
        "            debugger: DebuggerCore::new(debugger_config),\n            analysis: Mutex::new(crate::analysis::AnalysisWorkspace::default()),\n            analysis_directory: None,\n            scans: Mutex::new(HashMap::new()),",
    ),
]

for old, new in replacements:
    if old not in text:
        raise SystemExit(f"missing command.rs replacement marker: {old[:80]!r}")
    text = text.replace(old, new, 1)

marker = "    pub fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {"
builder = """    pub fn with_analysis_directory(mut self, directory: std::path::PathBuf) -> Self {
        self.analysis_directory = Some(directory);
        self
    }

"""
if builder not in text:
    if marker not in text:
        raise SystemExit("execute marker missing")
    text = text.replace(marker, builder + marker, 1)

old = "            Command::Shutdown => {\n                if !self.policy.allow_remote_shutdown {"
new = "            Command::Analysis { request } => {\n                let result = self.execute_analysis(request)?;\n                CommandExecution::immediate(CommandResult::Analysis { result })\n            }\n            Command::Shutdown => {\n                if !self.policy.allow_remote_shutdown {"
if old not in text:
    raise SystemExit("shutdown dispatch marker missing")
text = text.replace(old, new, 1)

insert_marker = "    pub fn cancel_all_scans(&self) -> Result<usize, CommandError> {"
analysis_methods = r'''    fn execute_analysis(
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
                let modules = self.analysis_modules()?;
                let address = AddressExpression::parse(&expression)?.resolve(&modules)?;
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
                let paths = search_pointer_chains(
                    &self.memory,
                    target,
                    &self.scanner_config,
                    options,
                )?;
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
                let template = SavedWatchTemplate {
                    name: name.clone(),
                    address: format!("0x{:X}", watch.address),
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
                    template.value_type.encode(value).map_err(MemoryError::from)?;
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

'''
if "fn execute_analysis(" not in text:
    if insert_marker not in text:
        raise SystemExit("cancel_all_scans marker missing")
    text = text.replace(insert_marker, analysis_methods + insert_marker, 1)

old = "    #[error(transparent)]\n    Debugger(#[from] DebuggerError),"
new = "    #[error(transparent)]\n    Debugger(#[from] DebuggerError),\n    #[error(transparent)]\n    Analysis(#[from] crate::analysis::AnalysisError),"
if old not in text:
    raise SystemExit("CommandError Debugger marker missing")
text = text.replace(old, new, 1)

old = '            Self::Debugger(_) => "debugger_error",'
new = '            Self::Debugger(_) => "debugger_error",\n            Self::Analysis(_) => "analysis_error",'
if old not in text:
    raise SystemExit("CommandError code marker missing")
text = text.replace(old, new, 1)
path.write_text(text)

analysis_path = Path("src/analysis.rs")
analysis_text = analysis_path.read_text()
old = '    #[error("analysis workspace version mismatch: expected {expected}, got {actual}")]\n    WorkspaceVersion { expected: u32, actual: u32 },'
new = '    #[error("analysis workspace version mismatch: expected {expected}, got {actual}")]\n    WorkspaceVersion { expected: u32, actual: u32 },\n    #[error("analysis workspace storage is not configured for this runtime")]\n    WorkspaceStorageUnavailable,'
if old not in analysis_text:
    raise SystemExit("AnalysisError workspace marker missing")
analysis_path.write_text(analysis_text.replace(old, new, 1))

runtime = Path("src/runtime.rs")
runtime_text = runtime.read_text()
old = "    let command_executor = create_command_executor(&config);"
new = "    let command_executor = create_command_executor(&config, &module_directory);"
if old not in runtime_text:
    raise SystemExit("runtime create_command_executor call marker missing")
runtime_text = runtime_text.replace(old, new, 1)

old = '''fn create_command_executor(config: &AppConfig) -> Arc<dyn CommandExecutor> {
    Arc::new(CommandDispatcher::new_with_debugger(
        CurrentProcessMemory::new(),
        config.scanner.clone(),
        config.debugger.clone(),
        config.policy.clone(),
        CommandLimits {
            max_memory_transfer_bytes: config.rpc.max_memory_transfer_bytes,
            max_scan_results_per_page: config.rpc.max_scan_results_per_page,
        },
    ))
}'''
new = '''fn create_command_executor(
    config: &AppConfig,
    module_directory: &Path,
) -> Arc<dyn CommandExecutor> {
    let analysis_directory = module_directory
        .join("analysis")
        .join(&config.target.executable);
    Arc::new(
        CommandDispatcher::new_with_debugger(
            CurrentProcessMemory::new(),
            config.scanner.clone(),
            config.debugger.clone(),
            config.policy.clone(),
            CommandLimits {
                max_memory_transfer_bytes: config.rpc.max_memory_transfer_bytes,
                max_scan_results_per_page: config.rpc.max_scan_results_per_page,
            },
        )
        .with_analysis_directory(analysis_directory),
    )
}'''
if old not in runtime_text:
    raise SystemExit("runtime create_command_executor definition marker missing")
runtime.write_text(runtime_text.replace(old, new, 1))
