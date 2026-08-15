use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use eframe::egui;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, info, warn};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use winit::platform::windows::EventLoopBuilderExtWindows;

use crate::{
    command::{
        Command, CommandExecutor, CommandResult, ModuleInfo, ScanCandidateInfo, ScanSummary,
        ThreadInfo, WatchValue,
    },
    config::UiConfig,
    scanner::{ScalarValue, ScanPredicate, ValueType},
};

const UI_STATE_KEY: &str = "intimatr.ui.state";
const HOTKEY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MIN_WINDOW_SIZE: [f32; 2] = [820.0, 520.0];

pub struct UiHandle {
    shutdown: Arc<AtomicBool>,
    context: Arc<Mutex<Option<egui::Context>>>,
    thread: Option<JoinHandle<()>>,
}

impl UiHandle {
    pub fn start(
        config: UiConfig,
        target_executable: String,
        persistence_path: PathBuf,
        executor: Arc<dyn CommandExecutor>,
    ) -> Result<Self, UiError> {
        let toggle_key = parse_virtual_key(&config.toggle_key)?;
        fs::create_dir_all(&persistence_path).map_err(|source| UiError::PersistenceDirectory {
            path: persistence_path.clone(),
            source,
        })?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let context = Arc::new(Mutex::new(None));
        let thread_shutdown = Arc::clone(&shutdown);
        let thread_context = Arc::clone(&context);
        let thread = thread::Builder::new()
            .name("intimatr-ui".to_owned())
            .spawn(move || {
                run_ui_thread(
                    config,
                    toggle_key,
                    target_executable,
                    persistence_path,
                    executor,
                    thread_shutdown,
                    thread_context,
                );
            })
            .map_err(UiError::ThreadSpawn)?;

        Ok(Self {
            shutdown,
            context,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) -> Result<(), UiError> {
        self.shutdown.store(true, Ordering::Release);
        if let Ok(context) = self.context.lock()
            && let Some(context) = context.as_ref()
        {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            context.request_repaint();
        }

        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| UiError::ThreadPanicked)?;
        }
        Ok(())
    }
}

impl Drop for UiHandle {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            warn!(error = %error, "failed to stop Intimatr UI cleanly");
        }
    }
}

fn run_ui_thread(
    config: UiConfig,
    toggle_key: i32,
    target_executable: String,
    persistence_path: PathBuf,
    executor: Arc<dyn CommandExecutor>,
    shutdown: Arc<AtomicBool>,
    shared_context: Arc<Mutex<Option<egui::Context>>>,
) {
    let title = format!("Intimatr — {target_executable}");
    let app_id = format!("intimatr.{}", sanitize_app_id(&target_executable));
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title.clone())
        .with_app_id(app_id)
        .with_inner_size([config.width, config.height])
        .with_min_inner_size(MIN_WINDOW_SIZE)
        .with_visible(config.initially_visible)
        .with_taskbar(true);
    if config.always_on_top {
        viewport = viewport.with_always_on_top();
    }

    let native_options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        persistence_path: Some(persistence_path),
        persist_window: true,
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };

    let app_config = config.clone();
    let app_shutdown = Arc::clone(&shutdown);
    let app_context = Arc::clone(&shared_context);
    let result = eframe::run_native(
        &title,
        native_options,
        Box::new(move |creation_context| {
            if let Ok(mut context) = app_context.lock() {
                *context = Some(creation_context.egui_ctx.clone());
            }
            Ok(Box::new(IntimatrApp::new(
                creation_context,
                app_config,
                toggle_key,
                target_executable,
                executor,
                app_shutdown,
            )))
        }),
    );

    if let Ok(mut context) = shared_context.lock() {
        *context = None;
    }
    match result {
        Ok(()) => info!("Intimatr UI thread exited"),
        Err(error) => error!(error = %error, "Intimatr UI renderer/event loop failed"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum UiTaskKind {
    FirstScan,
    NextScan,
    ScanResults,
    CancelScan,
    AddWatch,
    SetWatchFreeze,
    RemoveWatch,
    ListWatches,
    RefreshWatches,
    ReadMemory,
    WriteMemory,
    ListModules,
    ListThreads,
}

struct UiTaskResponse {
    kind: UiTaskKind,
    result: Result<CommandResult, String>,
}

struct IntimatrApp {
    config: UiConfig,
    target_executable: String,
    toggle_key: i32,
    toggle_key_down: bool,
    visible: bool,
    shutdown: Arc<AtomicBool>,
    executor: Arc<dyn CommandExecutor>,
    response_tx: Sender<UiTaskResponse>,
    response_rx: Receiver<UiTaskResponse>,
    pending: HashSet<UiTaskKind>,
    status: String,
    tab: ToolTab,

    scan_value_type: ValueType,
    scan_mode: ScanMode,
    scan_operand_a: String,
    scan_operand_b: String,
    scan_summary: Option<ScanSummary>,
    scan_results: Vec<ScanCandidateInfo>,
    scan_total: usize,
    scan_page: usize,

    watch_address: String,
    watch_value_type: ValueType,
    watch_label: String,
    watch_values: Vec<WatchValue>,
    last_watch_refresh: Instant,

    memory_address: String,
    memory_size: String,
    memory_write_hex: String,
    memory_loaded_address: Option<u64>,
    memory_bytes: Vec<u8>,

    modules: Vec<ModuleInfo>,
    threads: Vec<ThreadInfo>,
}

impl IntimatrApp {
    fn new(
        creation_context: &eframe::CreationContext<'_>,
        config: UiConfig,
        toggle_key: i32,
        target_executable: String,
        executor: Arc<dyn CommandExecutor>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        let persisted = creation_context
            .storage
            .and_then(|storage| eframe::get_value::<PersistedUiState>(storage, UI_STATE_KEY))
            .unwrap_or_default();
        let (response_tx, response_rx) = mpsc::channel();

        let mut app = Self {
            visible: config.initially_visible,
            config,
            target_executable,
            toggle_key,
            toggle_key_down: false,
            shutdown,
            executor,
            response_tx,
            response_rx,
            pending: HashSet::new(),
            status: "Ready".to_owned(),
            tab: persisted.tab,
            scan_value_type: persisted.scan_value_type,
            scan_mode: persisted.scan_mode,
            scan_operand_a: persisted.scan_operand_a,
            scan_operand_b: persisted.scan_operand_b,
            scan_summary: None,
            scan_results: Vec::new(),
            scan_total: 0,
            scan_page: 0,
            watch_address: persisted.watch_address,
            watch_value_type: persisted.watch_value_type,
            watch_label: persisted.watch_label,
            watch_values: Vec::new(),
            last_watch_refresh: Instant::now(),
            memory_address: persisted.memory_address,
            memory_size: persisted.memory_size,
            memory_write_hex: String::new(),
            memory_loaded_address: None,
            memory_bytes: Vec::new(),
            modules: Vec::new(),
            threads: Vec::new(),
        };
        app.submit(UiTaskKind::ListWatches, Command::ListWatches);
        app
    }

    fn submit(&mut self, kind: UiTaskKind, command: Command) {
        if !self.pending.insert(kind) {
            return;
        }

        let executor = Arc::clone(&self.executor);
        let sender = self.response_tx.clone();
        let task_name = command.name();
        let spawn = thread::Builder::new()
            .name(format!("intimatr-ui-{task_name}"))
            .spawn(move || {
                let result = executor
                    .execute(command)
                    .map(|execution| execution.result)
                    .map_err(|error| error.to_string());
                let _ = sender.send(UiTaskResponse { kind, result });
            });

        if let Err(error) = spawn {
            self.pending.remove(&kind);
            self.status = format!("Could not start {task_name}: {error}");
        }
    }

    fn drain_responses(&mut self) {
        let responses: Vec<_> = self.response_rx.try_iter().collect();
        for response in responses {
            self.pending.remove(&response.kind);
            match response.result {
                Ok(result) => self.handle_result(response.kind, result),
                Err(error) => self.status = error,
            }
        }
    }

    fn handle_result(&mut self, kind: UiTaskKind, result: CommandResult) {
        match result {
            CommandResult::Scan { summary } => {
                self.scan_summary = Some(summary);
                self.scan_total = summary.result_count;
                self.scan_page = 0;
                self.status = format!("Scan {}: {} results", summary.scan_id, summary.result_count);
                self.request_scan_page();
            }
            CommandResult::ScanResults {
                total,
                candidates,
                ..
            } => {
                self.scan_total = total;
                self.scan_results = candidates;
                self.status = format!("Showing {} of {} scan results", self.scan_results.len(), total);
            }
            CommandResult::ScanCancellation {
                scan_id,
                was_active,
            } => {
                self.status = if was_active {
                    format!("Cancellation requested for scan {scan_id}")
                } else {
                    format!("Scan {scan_id} was not active")
                };
            }
            CommandResult::WatchAdded { watch } => {
                self.status = format!("Added watch {} at 0x{:X}", watch.id, watch.address);
                self.request_watch_refresh();
            }
            CommandResult::WatchUpdated { watch } => {
                self.status = if watch.frozen.is_some() {
                    format!("Watch {} frozen", watch.id)
                } else {
                    format!("Watch {} unfrozen", watch.id)
                };
                self.request_watch_refresh();
            }
            CommandResult::WatchRemoved { watch_id, existed } => {
                self.status = if existed {
                    format!("Removed watch {watch_id}")
                } else {
                    format!("Watch {watch_id} no longer existed")
                };
                self.request_watch_refresh();
            }
            CommandResult::Watches { watches } => {
                self.watch_values = watches
                    .into_iter()
                    .map(|watch| WatchValue {
                        watch,
                        value: None,
                        error: None,
                    })
                    .collect();
                self.request_watch_refresh();
            }
            CommandResult::WatchValues { values } => {
                self.watch_values = values;
                self.last_watch_refresh = Instant::now();
                if kind != UiTaskKind::RefreshWatches {
                    self.status = "Watch list refreshed".to_owned();
                }
            }
            CommandResult::MemoryBytes { address, bytes } => {
                self.memory_loaded_address = Some(address);
                self.memory_bytes = bytes;
                self.status = format!("Read {} bytes at 0x{address:X}", self.memory_bytes.len());
            }
            CommandResult::WriteComplete { address, size } => {
                self.status = format!("Wrote {size} bytes at 0x{address:X}");
                if kind == UiTaskKind::WriteMemory {
                    self.request_memory_read();
                }
            }
            CommandResult::Modules { modules } => {
                self.status = format!("Enumerated {} loaded modules", modules.len());
                self.modules = modules;
            }
            CommandResult::Threads { threads } => {
                self.status = format!("Enumerated {} process threads", threads.len());
                self.threads = threads;
            }
            other => {
                self.status = format!("{} completed: {other:?}", task_label(kind));
            }
        }
    }

    fn request_scan_page(&mut self) {
        let Some(summary) = self.scan_summary else {
            return;
        };
        let offset = self.scan_page.saturating_mul(self.config.scan_page_size);
        self.submit(
            UiTaskKind::ScanResults,
            Command::ScanResults {
                scan_id: summary.scan_id,
                offset,
                limit: self.config.scan_page_size,
            },
        );
    }

    fn request_watch_refresh(&mut self) {
        self.submit(UiTaskKind::RefreshWatches, Command::RefreshWatches);
    }

    fn request_memory_read(&mut self) {
        let address = match parse_address(&self.memory_address) {
            Ok(address) => address,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let size = match self.memory_size.trim().parse::<usize>() {
            Ok(size) if size > 0 => size,
            _ => {
                self.status = "Memory read size must be a positive integer".to_owned();
                return;
            }
        };
        self.submit(UiTaskKind::ReadMemory, Command::ReadMemory { address, size });
    }

    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Intimatr");
            ui.separator();
            ui.label(&self.target_executable);
            ui.separator();
            ui.label(format!("Toggle: {}", self.config.toggle_key));
        });
        ui.horizontal(|ui| {
            for (tab, label) in [
                (ToolTab::Scan, "Scan"),
                (ToolTab::Watches, "Watches"),
                (ToolTab::Memory, "Memory"),
                (ToolTab::Modules, "Modules"),
                (ToolTab::Threads, "Threads"),
            ] {
                ui.selectable_value(&mut self.tab, tab, label);
            }
        });
        ui.separator();
    }

    fn render_scan(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Type");
            value_type_combo(ui, "scan_value_type", &mut self.scan_value_type);
            ui.label("Comparison");
            egui::ComboBox::from_id_salt("scan_mode")
                .selected_text(self.scan_mode.label())
                .show_ui(ui, |ui| {
                    for mode in ScanMode::ALL {
                        ui.selectable_value(&mut self.scan_mode, mode, mode.label());
                    }
                });
        });

        ui.horizontal(|ui| {
            if self.scan_mode.needs_operand_a() {
                ui.label(if self.scan_mode == ScanMode::BetweenInclusive {
                    "Min"
                } else {
                    "Value"
                });
                ui.text_edit_singleline(&mut self.scan_operand_a);
            }
            if self.scan_mode.needs_operand_b() {
                ui.label("Max");
                ui.text_edit_singleline(&mut self.scan_operand_b);
            }
        });

        ui.horizontal(|ui| {
            let scanning = self.pending.contains(&UiTaskKind::FirstScan)
                || self.pending.contains(&UiTaskKind::NextScan);
            if ui.add_enabled(!scanning, egui::Button::new("First scan")).clicked() {
                match self.scan_mode.to_predicate(
                    self.scan_value_type,
                    &self.scan_operand_a,
                    &self.scan_operand_b,
                ) {
                    Ok(predicate) if !predicate.requires_previous() => {
                        self.scan_results.clear();
                        self.scan_summary = None;
                        self.scan_total = 0;
                        self.submit(
                            UiTaskKind::FirstScan,
                            Command::FirstScan {
                                value_type: self.scan_value_type,
                                predicate,
                            },
                        );
                        self.status = "First scan running…".to_owned();
                    }
                    Ok(_) => {
                        self.status = "That comparison requires a previous scan; use Next scan".to_owned();
                    }
                    Err(error) => self.status = error,
                }
            }

            let can_next = !scanning && self.scan_summary.is_some();
            if ui.add_enabled(can_next, egui::Button::new("Next scan")).clicked() {
                let summary = self.scan_summary.expect("checked above");
                match self.scan_mode.to_predicate(
                    summary.value_type,
                    &self.scan_operand_a,
                    &self.scan_operand_b,
                ) {
                    Ok(predicate) => {
                        self.submit(
                            UiTaskKind::NextScan,
                            Command::NextScan {
                                scan_id: summary.scan_id,
                                predicate,
                            },
                        );
                        self.status = format!("Refining scan {}…", summary.scan_id);
                    }
                    Err(error) => self.status = error,
                }
            }

            if self.pending.contains(&UiTaskKind::NextScan)
                && let Some(summary) = self.scan_summary
                && ui.button("Cancel next scan").clicked()
            {
                self.submit(
                    UiTaskKind::CancelScan,
                    Command::CancelScan {
                        scan_id: summary.scan_id,
                    },
                );
            }
        });

        if let Some(summary) = self.scan_summary {
            ui.label(format!(
                "Scan {} · {} · {} candidates · {:.2} MiB/s",
                summary.scan_id,
                value_type_label(summary.value_type),
                summary.result_count,
                summary.stats.throughput_mib_per_sec
            ));
        }

        self.render_scan_results(ui);
    }

    fn render_scan_results(&mut self, ui: &mut egui::Ui) {
        let page_size = self.config.scan_page_size;
        let page_count = if self.scan_total == 0 {
            1
        } else {
            self.scan_total.saturating_add(page_size - 1) / page_size
        };
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.scan_page > 0, egui::Button::new("Previous"))
                .clicked()
            {
                self.scan_page -= 1;
                self.request_scan_page();
            }
            ui.label(format!("Page {} / {page_count}", self.scan_page + 1));
            if ui
                .add_enabled(
                    self.scan_page + 1 < page_count,
                    egui::Button::new("Next"),
                )
                .clicked()
            {
                self.scan_page += 1;
                self.request_scan_page();
            }
            ui.label(format!("{} total", self.scan_total));
        });

        let mut add_watch = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("scan_results")
                .striped(true)
                .num_columns(4)
                .show(ui, |ui| {
                    ui.strong("Address");
                    ui.strong("Current");
                    ui.strong("Previous");
                    ui.strong("Action");
                    ui.end_row();
                    for candidate in &self.scan_results {
                        ui.monospace(format!("0x{:016X}", candidate.address));
                        ui.monospace(format_scalar(candidate.current));
                        ui.monospace(
                            candidate
                                .previous
                                .map(format_scalar)
                                .unwrap_or_else(|| "—".to_owned()),
                        );
                        if ui.button("Watch").clicked() {
                            add_watch = Some(candidate.address);
                        }
                        ui.end_row();
                    }
                });
        });
        if let Some(address) = add_watch {
            let value_type = self
                .scan_summary
                .map(|summary| summary.value_type)
                .unwrap_or(self.scan_value_type);
            self.submit(
                UiTaskKind::AddWatch,
                Command::AddWatch {
                    address,
                    value_type,
                    label: None,
                },
            );
        }
    }

    fn render_watches(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Address");
            ui.text_edit_singleline(&mut self.watch_address);
            ui.label("Type");
            value_type_combo(ui, "watch_value_type", &mut self.watch_value_type);
            ui.label("Label");
            ui.text_edit_singleline(&mut self.watch_label);
            if ui.button("Add").clicked() {
                match parse_address(&self.watch_address) {
                    Ok(address) => self.submit(
                        UiTaskKind::AddWatch,
                        Command::AddWatch {
                            address,
                            value_type: self.watch_value_type,
                            label: nonempty_string(&self.watch_label),
                        },
                    ),
                    Err(error) => self.status = error,
                }
            }
            if ui.button("Refresh").clicked() {
                self.request_watch_refresh();
            }
        });

        let mut freeze_action = None;
        let mut remove_action = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("watch_grid")
                .striped(true)
                .num_columns(7)
                .show(ui, |ui| {
                    ui.strong("Label");
                    ui.strong("Address");
                    ui.strong("Type");
                    ui.strong("Value");
                    ui.strong("Freeze");
                    ui.strong("Error");
                    ui.strong("Action");
                    ui.end_row();
                    for watch_value in &self.watch_values {
                        let watch = &watch_value.watch;
                        ui.label(watch.label.as_deref().unwrap_or(""));
                        ui.monospace(format!("0x{:016X}", watch.address));
                        ui.label(value_type_label(watch.value_type));
                        ui.monospace(
                            watch_value
                                .value
                                .map(format_scalar)
                                .unwrap_or_else(|| "—".to_owned()),
                        );
                        let mut frozen = watch.frozen.is_some();
                        if ui.checkbox(&mut frozen, "").changed() {
                            let value = if frozen { watch_value.value } else { None };
                            if frozen && value.is_none() {
                                self.status = "Cannot freeze a watch until it has a readable value".to_owned();
                            } else {
                                freeze_action = Some((watch.id, value));
                            }
                        }
                        ui.label(watch_value.error.as_deref().unwrap_or(""));
                        if ui.button("Remove").clicked() {
                            remove_action = Some(watch.id);
                        }
                        ui.end_row();
                    }
                });
        });

        if let Some((watch_id, value)) = freeze_action {
            self.submit(
                UiTaskKind::SetWatchFreeze,
                Command::SetWatchFreeze { watch_id, value },
            );
        }
        if let Some(watch_id) = remove_action {
            self.submit(UiTaskKind::RemoveWatch, Command::RemoveWatch { watch_id });
        }
    }

    fn render_memory(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Address");
            ui.text_edit_singleline(&mut self.memory_address);
            ui.label("Bytes");
            ui.text_edit_singleline(&mut self.memory_size);
            if ui.button("Read").clicked() {
                self.request_memory_read();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Write hex");
            ui.text_edit_singleline(&mut self.memory_write_hex);
            if ui.button("Write at address").clicked() {
                let address = parse_address(&self.memory_address);
                let bytes = parse_hex_bytes(&self.memory_write_hex);
                match (address, bytes) {
                    (Ok(address), Ok(bytes)) if !bytes.is_empty() => self.submit(
                        UiTaskKind::WriteMemory,
                        Command::WriteMemory { address, bytes },
                    ),
                    (Ok(_), Ok(_)) => self.status = "Enter at least one byte to write".to_owned(),
                    (Err(error), _) | (_, Err(error)) => self.status = error,
                }
            }
        });

        if let Some(base) = self.memory_loaded_address {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("memory_hex")
                    .striped(true)
                    .num_columns(3)
                    .show(ui, |ui| {
                        ui.strong("Address");
                        ui.strong("Hex");
                        ui.strong("ASCII");
                        ui.end_row();
                        for (row, bytes) in self.memory_bytes.chunks(16).enumerate() {
                            ui.monospace(format!("0x{:016X}", base + (row * 16) as u64));
                            let hex = bytes
                                .iter()
                                .map(|byte| format!("{byte:02X}"))
                                .collect::<Vec<_>>()
                                .join(" ");
                            ui.monospace(hex);
                            let ascii: String = bytes
                                .iter()
                                .map(|byte| {
                                    if byte.is_ascii_graphic() || *byte == b' ' {
                                        *byte as char
                                    } else {
                                        '.'
                                    }
                                })
                                .collect();
                            ui.monospace(ascii);
                            ui.end_row();
                        }
                    });
            });
        }
    }

    fn render_modules(&mut self, ui: &mut egui::Ui) {
        if ui.button("Refresh modules").clicked() {
            self.submit(UiTaskKind::ListModules, Command::ListModules);
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("modules")
                .striped(true)
                .num_columns(4)
                .show(ui, |ui| {
                    ui.strong("Module");
                    ui.strong("Base");
                    ui.strong("Size");
                    ui.strong("Path");
                    ui.end_row();
                    for module in &self.modules {
                        ui.label(&module.name);
                        ui.monospace(format!("0x{:016X}", module.base));
                        ui.label(format!("{} KiB", module.size / 1024));
                        ui.label(&module.path);
                        ui.end_row();
                    }
                });
        });
    }

    fn render_threads(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Refresh threads").clicked() {
                self.submit(UiTaskKind::ListThreads, Command::ListThreads);
            }
            ui.label("Register/context inspection lands in Milestone 5.");
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("threads")
                .striped(true)
                .num_columns(1)
                .show(ui, |ui| {
                    ui.strong("Thread ID");
                    ui.end_row();
                    for thread in &self.threads {
                        ui.monospace(thread.thread_id.to_string());
                        ui.end_row();
                    }
                });
        });
    }
}

impl eframe::App for IntimatrApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_responses();

        let key_down = unsafe { GetAsyncKeyState(self.toggle_key) } as u16 & 0x8000 != 0;
        if key_down && !self.toggle_key_down {
            self.visible = !self.visible;
            context.send_viewport_cmd(egui::ViewportCommand::Visible(self.visible));
            if self.visible {
                context.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        self.toggle_key_down = key_down;

        let close_requested = context.input(|input| input.viewport().close_requested());
        if close_requested && !self.shutdown.load(Ordering::Acquire) {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.visible = false;
        } else if self.shutdown.load(Ordering::Acquire) {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if self.last_watch_refresh.elapsed() >= Duration::from_millis(self.config.watch_refresh_ms)
            && !self.watch_values.is_empty()
            && !self.pending.contains(&UiTaskKind::RefreshWatches)
        {
            self.request_watch_refresh();
        }

        context.request_repaint_after(HOTKEY_POLL_INTERVAL);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            self.render_top_bar(ui);
            match self.tab {
                ToolTab::Scan => self.render_scan(ui),
                ToolTab::Watches => self.render_watches(ui),
                ToolTab::Memory => self.render_memory(ui),
                ToolTab::Modules => self.render_modules(ui),
                ToolTab::Threads => self.render_threads(ui),
            }
            ui.separator();
            ui.label(format!("Status: {}", self.status));
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(
            storage,
            UI_STATE_KEY,
            &PersistedUiState {
                tab: self.tab,
                scan_value_type: self.scan_value_type,
                scan_mode: self.scan_mode,
                scan_operand_a: self.scan_operand_a.clone(),
                scan_operand_b: self.scan_operand_b.clone(),
                watch_address: self.watch_address.clone(),
                watch_value_type: self.watch_value_type,
                watch_label: self.watch_label.clone(),
                memory_address: self.memory_address.clone(),
                memory_size: self.memory_size.clone(),
            },
        );
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ToolTab {
    #[default]
    Scan,
    Watches,
    Memory,
    Modules,
    Threads,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScanMode {
    #[default]
    Exact,
    UnknownInitialValue,
    NotEqual,
    GreaterThan,
    GreaterOrEqual,
    LessThan,
    LessOrEqual,
    BetweenInclusive,
    Changed,
    Unchanged,
    Increased,
    Decreased,
    IncreasedBy,
    DecreasedBy,
}

impl ScanMode {
    const ALL: [Self; 14] = [
        Self::Exact,
        Self::UnknownInitialValue,
        Self::NotEqual,
        Self::GreaterThan,
        Self::GreaterOrEqual,
        Self::LessThan,
        Self::LessOrEqual,
        Self::BetweenInclusive,
        Self::Changed,
        Self::Unchanged,
        Self::Increased,
        Self::Decreased,
        Self::IncreasedBy,
        Self::DecreasedBy,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Exact => "Exact",
            Self::UnknownInitialValue => "Unknown initial value",
            Self::NotEqual => "Not equal",
            Self::GreaterThan => "Greater than",
            Self::GreaterOrEqual => "Greater or equal",
            Self::LessThan => "Less than",
            Self::LessOrEqual => "Less or equal",
            Self::BetweenInclusive => "Between (inclusive)",
            Self::Changed => "Changed",
            Self::Unchanged => "Unchanged",
            Self::Increased => "Increased",
            Self::Decreased => "Decreased",
            Self::IncreasedBy => "Increased by",
            Self::DecreasedBy => "Decreased by",
        }
    }

    const fn needs_operand_a(self) -> bool {
        matches!(
            self,
            Self::Exact
                | Self::NotEqual
                | Self::GreaterThan
                | Self::GreaterOrEqual
                | Self::LessThan
                | Self::LessOrEqual
                | Self::BetweenInclusive
                | Self::IncreasedBy
                | Self::DecreasedBy
        )
    }

    const fn needs_operand_b(self) -> bool {
        matches!(self, Self::BetweenInclusive)
    }

    fn to_predicate(
        self,
        value_type: ValueType,
        operand_a: &str,
        operand_b: &str,
    ) -> Result<ScanPredicate, String> {
        let a = || parse_scalar(value_type, operand_a);
        Ok(match self {
            Self::Exact => ScanPredicate::Exact(a()?),
            Self::UnknownInitialValue => ScanPredicate::UnknownInitialValue,
            Self::NotEqual => ScanPredicate::NotEqual(a()?),
            Self::GreaterThan => ScanPredicate::GreaterThan(a()?),
            Self::GreaterOrEqual => ScanPredicate::GreaterOrEqual(a()?),
            Self::LessThan => ScanPredicate::LessThan(a()?),
            Self::LessOrEqual => ScanPredicate::LessOrEqual(a()?),
            Self::BetweenInclusive => ScanPredicate::BetweenInclusive {
                min: a()?,
                max: parse_scalar(value_type, operand_b)?,
            },
            Self::Changed => ScanPredicate::Changed,
            Self::Unchanged => ScanPredicate::Unchanged,
            Self::Increased => ScanPredicate::Increased,
            Self::Decreased => ScanPredicate::Decreased,
            Self::IncreasedBy => ScanPredicate::IncreasedBy(a()?),
            Self::DecreasedBy => ScanPredicate::DecreasedBy(a()?),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct PersistedUiState {
    tab: ToolTab,
    scan_value_type: ValueType,
    scan_mode: ScanMode,
    scan_operand_a: String,
    scan_operand_b: String,
    watch_address: String,
    watch_value_type: ValueType,
    watch_label: String,
    memory_address: String,
    memory_size: String,
}

impl Default for PersistedUiState {
    fn default() -> Self {
        Self {
            tab: ToolTab::Scan,
            scan_value_type: ValueType::I32,
            scan_mode: ScanMode::Exact,
            scan_operand_a: "0".to_owned(),
            scan_operand_b: "0".to_owned(),
            watch_address: "0x0".to_owned(),
            watch_value_type: ValueType::I32,
            watch_label: String::new(),
            memory_address: "0x0".to_owned(),
            memory_size: "256".to_owned(),
        }
    }
}

fn value_type_combo(ui: &mut egui::Ui, id: &str, value_type: &mut ValueType) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(value_type_label(*value_type))
        .show_ui(ui, |ui| {
            for candidate in [
                ValueType::I8,
                ValueType::I16,
                ValueType::I32,
                ValueType::I64,
                ValueType::U8,
                ValueType::U16,
                ValueType::U32,
                ValueType::U64,
                ValueType::F32,
                ValueType::F64,
            ] {
                ui.selectable_value(value_type, candidate, value_type_label(candidate));
            }
        });
}

const fn value_type_label(value_type: ValueType) -> &'static str {
    match value_type {
        ValueType::I8 => "i8",
        ValueType::I16 => "i16",
        ValueType::I32 => "i32",
        ValueType::I64 => "i64",
        ValueType::U8 => "u8",
        ValueType::U16 => "u16",
        ValueType::U32 => "u32",
        ValueType::U64 => "u64",
        ValueType::F32 => "f32",
        ValueType::F64 => "f64",
    }
}

fn parse_scalar(value_type: ValueType, text: &str) -> Result<ScalarValue, String> {
    let text = text.trim();
    let value = match value_type {
        ValueType::I8 | ValueType::I16 | ValueType::I32 | ValueType::I64 => text
            .parse::<i64>()
            .map(ScalarValue::Signed)
            .map_err(|_| format!("{text:?} is not a valid signed integer"))?,
        ValueType::U8 | ValueType::U16 | ValueType::U32 | ValueType::U64 => text
            .parse::<u64>()
            .map(ScalarValue::Unsigned)
            .map_err(|_| format!("{text:?} is not a valid unsigned integer"))?,
        ValueType::F32 | ValueType::F64 => text
            .parse::<f64>()
            .map(ScalarValue::Float)
            .map_err(|_| format!("{text:?} is not a valid floating-point value"))?,
    };
    value_type
        .encode(value)
        .map_err(|error| error.to_string())?;
    Ok(value)
}

fn parse_address(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    if digits.is_empty() {
        return Err("Address must not be empty".to_owned());
    }
    u64::from_str_radix(digits, 16).map_err(|_| format!("{text:?} is not a valid hexadecimal address"))
}

fn parse_hex_bytes(text: &str) -> Result<Vec<u8>, String> {
    let compact: String = text
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != ',' && *character != '_')
        .collect();
    if compact.is_empty() {
        return Ok(Vec::new());
    }
    if compact.len() % 2 != 0 {
        return Err("Hex byte input must contain an even number of digits".to_owned());
    }
    (0..compact.len())
        .step_by(2)
        .map(|offset| {
            u8::from_str_radix(&compact[offset..offset + 2], 16)
                .map_err(|_| format!("Invalid hex byte near offset {offset}"))
        })
        .collect()
}

fn format_scalar(value: ScalarValue) -> String {
    match value {
        ScalarValue::Signed(value) => value.to_string(),
        ScalarValue::Unsigned(value) => value.to_string(),
        ScalarValue::Float(value) => format!("{value:.8}"),
    }
}

fn nonempty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn sanitize_app_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn task_label(kind: UiTaskKind) -> &'static str {
    match kind {
        UiTaskKind::FirstScan => "first scan",
        UiTaskKind::NextScan => "next scan",
        UiTaskKind::ScanResults => "scan results",
        UiTaskKind::CancelScan => "scan cancellation",
        UiTaskKind::AddWatch => "add watch",
        UiTaskKind::SetWatchFreeze => "watch freeze",
        UiTaskKind::RemoveWatch => "remove watch",
        UiTaskKind::ListWatches => "list watches",
        UiTaskKind::RefreshWatches => "refresh watches",
        UiTaskKind::ReadMemory => "memory read",
        UiTaskKind::WriteMemory => "memory write",
        UiTaskKind::ListModules => "module enumeration",
        UiTaskKind::ListThreads => "thread enumeration",
    }
}

fn parse_virtual_key(name: &str) -> Result<i32, UiError> {
    let normalized = name.trim().to_ascii_uppercase();
    let key = match normalized.as_str() {
        "INSERT" | "INS" => 0x2D,
        "DELETE" | "DEL" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" | "PAGE_UP" | "PGUP" => 0x21,
        "PAGEDOWN" | "PAGE_DOWN" | "PGDN" => 0x22,
        "PAUSE" => 0x13,
        _ if normalized.len() == 1 => {
            let byte = normalized.as_bytes()[0];
            if byte.is_ascii_alphanumeric() {
                byte as i32
            } else {
                return Err(UiError::InvalidToggleKey(name.to_owned()));
            }
        }
        _ if normalized.starts_with('F') => {
            let number = normalized[1..]
                .parse::<i32>()
                .map_err(|_| UiError::InvalidToggleKey(name.to_owned()))?;
            if !(1..=24).contains(&number) {
                return Err(UiError::InvalidToggleKey(name.to_owned()));
            }
            0x70 + number - 1
        }
        _ => return Err(UiError::InvalidToggleKey(name.to_owned())),
    };
    Ok(key)
}

#[derive(Debug, Error)]
pub enum UiError {
    #[error("unsupported UI toggle key {0:?}")]
    InvalidToggleKey(String),
    #[error("failed to create UI persistence directory {path}")]
    PersistenceDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to spawn the Intimatr UI thread: {0}")]
    ThreadSpawn(std::io::Error),
    #[error("Intimatr UI thread panicked")]
    ThreadPanicked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_addresses() {
        assert_eq!(parse_address("0x1234").unwrap(), 0x1234);
        assert_eq!(parse_address("ABCD").unwrap(), 0xABCD);
        assert!(parse_address("xyz").is_err());
    }

    #[test]
    fn parses_scalar_kinds_and_widths() {
        assert_eq!(parse_scalar(ValueType::I32, "-7").unwrap(), ScalarValue::Signed(-7));
        assert_eq!(parse_scalar(ValueType::U16, "42").unwrap(), ScalarValue::Unsigned(42));
        assert_eq!(parse_scalar(ValueType::F32, "1.5").unwrap(), ScalarValue::Float(1.5));
        assert!(parse_scalar(ValueType::I8, "128").is_err());
    }

    #[test]
    fn parses_hex_editor_input() {
        assert_eq!(parse_hex_bytes("90 FF 00").unwrap(), vec![0x90, 0xFF, 0x00]);
        assert_eq!(parse_hex_bytes("90_ff,00").unwrap(), vec![0x90, 0xFF, 0x00]);
        assert!(parse_hex_bytes("ABC").is_err());
    }

    #[test]
    fn parses_toggle_keys() {
        assert_eq!(parse_virtual_key("Insert").unwrap(), 0x2D);
        assert_eq!(parse_virtual_key("F12").unwrap(), 0x7B);
        assert_eq!(parse_virtual_key("A").unwrap(), 0x41);
        assert!(parse_virtual_key("F25").is_err());
    }
}
