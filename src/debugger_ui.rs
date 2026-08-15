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
        Command, CommandExecutor, CommandResult, DebuggerEvent, DebuggerStatus, DisassemblyLine,
        HardwareBreakpoint, HardwareBreakpointKind, RegisterSnapshot, ThreadInfo,
    },
    config::DebuggerConfig,
};

const DEBUGGER_UI_STATE_KEY: &str = "intimatr.debugger.ui.state";
const HOTKEY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MIN_WINDOW_SIZE: [f32; 2] = [860.0, 560.0];
const MAX_LOCAL_EVENTS: usize = 2_000;

pub struct DebuggerUiHandle {
    shutdown: Arc<AtomicBool>,
    context: Arc<Mutex<Option<egui::Context>>>,
    thread: Option<JoinHandle<()>>,
}

impl DebuggerUiHandle {
    pub fn start(
        config: DebuggerConfig,
        target_executable: String,
        persistence_path: PathBuf,
        executor: Arc<dyn CommandExecutor>,
    ) -> Result<Self, DebuggerUiError> {
        let toggle_key = parse_virtual_key(&config.ui_toggle_key)?;
        fs::create_dir_all(&persistence_path).map_err(|source| {
            DebuggerUiError::PersistenceDirectory {
                path: persistence_path.clone(),
                source,
            }
        })?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let context = Arc::new(Mutex::new(None));
        let thread_shutdown = Arc::clone(&shutdown);
        let thread_context = Arc::clone(&context);
        let thread = thread::Builder::new()
            .name("intimatr-debugger-ui".to_owned())
            .spawn(move || {
                run_debugger_ui_thread(
                    config,
                    toggle_key,
                    target_executable,
                    persistence_path,
                    executor,
                    thread_shutdown,
                    thread_context,
                );
            })
            .map_err(DebuggerUiError::ThreadSpawn)?;

        Ok(Self {
            shutdown,
            context,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) -> Result<(), DebuggerUiError> {
        self.shutdown.store(true, Ordering::Release);
        if let Ok(context) = self.context.lock()
            && let Some(context) = context.as_ref()
        {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            context.request_repaint();
        }
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| DebuggerUiError::ThreadPanicked)?;
        }
        Ok(())
    }
}

impl Drop for DebuggerUiHandle {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            warn!(error = %error, "failed to stop Intimatr debugger UI cleanly");
        }
    }
}

fn run_debugger_ui_thread(
    config: DebuggerConfig,
    toggle_key: i32,
    target_executable: String,
    persistence_path: PathBuf,
    executor: Arc<dyn CommandExecutor>,
    shutdown: Arc<AtomicBool>,
    shared_context: Arc<Mutex<Option<egui::Context>>>,
) {
    let title = format!("Intimatr Debugger — {target_executable}");
    let app_id = format!("intimatr.debugger.{}", sanitize_app_id(&target_executable));
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title.clone())
        .with_app_id(app_id)
        .with_inner_size([config.ui_width, config.ui_height])
        .with_min_inner_size(MIN_WINDOW_SIZE)
        .with_visible(config.ui_initially_visible)
        .with_taskbar(true);
    if config.ui_always_on_top {
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
            Ok(Box::new(DebuggerApp::new(
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
        Ok(()) => info!("Intimatr debugger UI thread exited"),
        Err(error) => error!(error = %error, "Intimatr debugger UI failed"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DebuggerTaskKind {
    ListThreads,
    Registers,
    Disassemble,
    Status,
    Pause,
    Resume,
    Step,
    AddBreakpoint,
    RemoveBreakpoint,
    ListBreakpoints,
    Events,
}

struct DebuggerTaskResponse {
    kind: DebuggerTaskKind,
    result: Result<CommandResult, String>,
}

struct DebuggerApp {
    config: DebuggerConfig,
    target_executable: String,
    toggle_key: i32,
    toggle_key_down: bool,
    visible: bool,
    shutdown: Arc<AtomicBool>,
    executor: Arc<dyn CommandExecutor>,
    response_tx: Sender<DebuggerTaskResponse>,
    response_rx: Receiver<DebuggerTaskResponse>,
    pending: HashSet<DebuggerTaskKind>,
    status_text: String,
    tab: DebuggerTab,

    threads: Vec<ThreadInfo>,
    selected_thread: Option<u32>,
    registers: Option<RegisterSnapshot>,
    debugger_status: Option<DebuggerStatus>,

    disassembly_address: String,
    disassembly_bitness: u32,
    disassembly_bytes: String,
    disassembly_instructions: String,
    disassembly: Vec<DisassemblyLine>,

    breakpoint_address: String,
    breakpoint_kind: HardwareBreakpointKind,
    breakpoint_size: u8,
    breakpoints: Vec<HardwareBreakpoint>,

    events: Vec<DebuggerEvent>,
    last_event_sequence: u64,
    last_event_poll: Instant,
}

impl DebuggerApp {
    fn new(
        creation_context: &eframe::CreationContext<'_>,
        config: DebuggerConfig,
        toggle_key: i32,
        target_executable: String,
        executor: Arc<dyn CommandExecutor>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        let persisted = creation_context
            .storage
            .and_then(|storage| {
                eframe::get_value::<PersistedDebuggerUiState>(storage, DEBUGGER_UI_STATE_KEY)
            })
            .unwrap_or_default();
        let (response_tx, response_rx) = mpsc::channel();
        let mut app = Self {
            visible: config.ui_initially_visible,
            disassembly_bytes: config.disassembly_default_bytes.to_string(),
            disassembly_instructions: config.disassembly_default_instructions.to_string(),
            config,
            target_executable,
            toggle_key,
            toggle_key_down: false,
            shutdown,
            executor,
            response_tx,
            response_rx,
            pending: HashSet::new(),
            status_text: "Debugger ready".to_owned(),
            tab: persisted.tab,
            threads: Vec::new(),
            selected_thread: None,
            registers: None,
            debugger_status: None,
            disassembly_address: persisted.disassembly_address,
            disassembly_bitness: persisted.disassembly_bitness,
            disassembly: Vec::new(),
            breakpoint_address: persisted.breakpoint_address,
            breakpoint_kind: persisted.breakpoint_kind,
            breakpoint_size: persisted.breakpoint_size,
            breakpoints: Vec::new(),
            events: Vec::new(),
            last_event_sequence: 0,
            last_event_poll: Instant::now(),
        };
        app.submit(DebuggerTaskKind::ListThreads, Command::ListThreads);
        app.submit(DebuggerTaskKind::Status, Command::DebuggerStatus);
        app.submit(
            DebuggerTaskKind::ListBreakpoints,
            Command::ListHardwareBreakpoints,
        );
        app
    }

    fn submit(&mut self, kind: DebuggerTaskKind, command: Command) {
        if !self.pending.insert(kind) {
            return;
        }
        let executor = Arc::clone(&self.executor);
        let sender = self.response_tx.clone();
        let command_name = command.name();
        let spawn = thread::Builder::new()
            .name(format!("intimatr-debugger-{command_name}"))
            .spawn(move || {
                let result = executor
                    .execute(command)
                    .map(|execution| execution.result)
                    .map_err(|error| error.to_string());
                let _ = sender.send(DebuggerTaskResponse { kind, result });
            });
        if let Err(error) = spawn {
            self.pending.remove(&kind);
            self.status_text = format!("Could not start {command_name}: {error}");
        }
    }

    fn drain_responses(&mut self) {
        let responses: Vec<_> = self.response_rx.try_iter().collect();
        for response in responses {
            self.pending.remove(&response.kind);
            match response.result {
                Ok(result) => self.handle_result(response.kind, result),
                Err(error) => self.status_text = error,
            }
        }
    }

    fn handle_result(&mut self, kind: DebuggerTaskKind, result: CommandResult) {
        match result {
            CommandResult::Threads { threads } => {
                self.status_text = format!("Enumerated {} threads", threads.len());
                if self
                    .selected_thread
                    .is_none_or(|selected| !threads.iter().any(|item| item.thread_id == selected))
                {
                    self.selected_thread = threads.first().map(|item| item.thread_id);
                }
                self.threads = threads;
            }
            CommandResult::ThreadRegisters { registers } => {
                self.status_text = format!(
                    "Captured thread {} at RIP 0x{:X}",
                    registers.thread_id, registers.instruction_pointer
                );
                self.registers = Some(registers);
            }
            CommandResult::Disassembly { lines, .. } => {
                self.status_text = format!("Decoded {} instructions", lines.len());
                self.disassembly = lines;
            }
            CommandResult::DebuggerStatus { status } => {
                self.breakpoints = status.breakpoints.clone();
                self.last_event_sequence = self.last_event_sequence.max(status.latest_event_sequence);
                self.debugger_status = Some(status);
            }
            CommandResult::ThreadControl { state } => {
                self.status_text = if state.paused_by_intimatr {
                    format!("Paused thread {}", state.thread_id)
                } else if kind == DebuggerTaskKind::Step {
                    format!("Single-step armed for thread {}", state.thread_id)
                } else {
                    format!("Resumed thread {}", state.thread_id)
                };
                self.submit(DebuggerTaskKind::Status, Command::DebuggerStatus);
                if kind != DebuggerTaskKind::Step {
                    self.request_registers();
                }
            }
            CommandResult::HardwareBreakpointAdded { breakpoint } => {
                self.status_text = format!(
                    "Breakpoint {} uses DR{} on thread {}",
                    breakpoint.id, breakpoint.slot, breakpoint.thread_id
                );
                self.submit(
                    DebuggerTaskKind::ListBreakpoints,
                    Command::ListHardwareBreakpoints,
                );
            }
            CommandResult::HardwareBreakpointRemoved {
                breakpoint_id,
                existed,
            } => {
                self.status_text = if existed {
                    format!("Removed breakpoint {breakpoint_id}")
                } else {
                    format!("Breakpoint {breakpoint_id} no longer existed")
                };
                self.submit(
                    DebuggerTaskKind::ListBreakpoints,
                    Command::ListHardwareBreakpoints,
                );
            }
            CommandResult::HardwareBreakpoints { breakpoints } => {
                self.breakpoints = breakpoints;
            }
            CommandResult::DebuggerEvents {
                events,
                latest_sequence,
            } => {
                self.last_event_sequence = self.last_event_sequence.max(latest_sequence);
                self.events.extend(events);
                if self.events.len() > MAX_LOCAL_EVENTS {
                    let drain = self.events.len() - MAX_LOCAL_EVENTS;
                    self.events.drain(..drain);
                }
                self.last_event_poll = Instant::now();
            }
            other => {
                self.status_text = format!("{} completed: {other:?}", task_label(kind));
            }
        }
    }

    fn request_registers(&mut self) {
        let Some(thread_id) = self.selected_thread else {
            self.status_text = "Select a thread first".to_owned();
            return;
        };
        self.submit(
            DebuggerTaskKind::Registers,
            Command::ReadThreadRegisters { thread_id },
        );
    }

    fn request_disassembly(&mut self) {
        let address = match parse_address(&self.disassembly_address) {
            Ok(address) => address,
            Err(error) => {
                self.status_text = error;
                return;
            }
        };
        let byte_count = match self.disassembly_bytes.trim().parse::<usize>() {
            Ok(value) if value > 0 => value,
            _ => {
                self.status_text = "Disassembly byte count must be positive".to_owned();
                return;
            }
        };
        let max_instructions = match self.disassembly_instructions.trim().parse::<usize>() {
            Ok(value) if value > 0 => value,
            _ => {
                self.status_text = "Instruction limit must be positive".to_owned();
                return;
            }
        };
        self.submit(
            DebuggerTaskKind::Disassemble,
            Command::Disassemble {
                address,
                byte_count,
                max_instructions,
                bitness: self.disassembly_bitness,
            },
        );
    }

    fn request_thread_control(&mut self, kind: DebuggerTaskKind) {
        let Some(thread_id) = self.selected_thread else {
            self.status_text = "Select a thread first".to_owned();
            return;
        };
        let command = match kind {
            DebuggerTaskKind::Pause => Command::PauseThread { thread_id },
            DebuggerTaskKind::Resume => Command::ResumeThread { thread_id },
            DebuggerTaskKind::Step => Command::SingleStepThread { thread_id },
            _ => return,
        };
        self.submit(kind, command);
    }

    fn request_add_breakpoint(&mut self) {
        let Some(thread_id) = self.selected_thread else {
            self.status_text = "Select a target thread first".to_owned();
            return;
        };
        let address = match parse_address(&self.breakpoint_address) {
            Ok(address) => address,
            Err(error) => {
                self.status_text = error;
                return;
            }
        };
        let size = if self.breakpoint_kind == HardwareBreakpointKind::Execute {
            1
        } else {
            self.breakpoint_size
        };
        self.submit(
            DebuggerTaskKind::AddBreakpoint,
            Command::SetHardwareBreakpoint {
                thread_id,
                address,
                kind: self.breakpoint_kind,
                size,
            },
        );
    }

    fn poll_events(&mut self) {
        if self.last_event_poll.elapsed() < Duration::from_millis(self.config.event_poll_ms) {
            return;
        }
        self.last_event_poll = Instant::now();
        self.submit(
            DebuggerTaskKind::Events,
            Command::DebuggerEvents {
                after_sequence: self.last_event_sequence,
                limit: self.config.max_events_per_poll,
            },
        );
    }

    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Intimatr Debugger");
            ui.separator();
            ui.label(&self.target_executable);
            ui.separator();
            ui.label(format!("Toggle: {}", self.config.ui_toggle_key));
        });
        ui.horizontal(|ui| {
            for (tab, label) in [
                (DebuggerTab::Threads, "Threads / Registers"),
                (DebuggerTab::Disassembly, "Disassembly"),
                (DebuggerTab::Breakpoints, "Breakpoints"),
                (DebuggerTab::Events, "Events"),
            ] {
                ui.selectable_value(&mut self.tab, tab, label);
            }
        });
        ui.separator();
    }

    fn render_threads(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Refresh threads").clicked() {
                self.submit(DebuggerTaskKind::ListThreads, Command::ListThreads);
            }
            if ui.button("Read registers").clicked() {
                self.request_registers();
            }
            if ui.button("Pause selected").clicked() {
                self.request_thread_control(DebuggerTaskKind::Pause);
            }
            if ui.button("Resume selected").clicked() {
                self.request_thread_control(DebuggerTaskKind::Resume);
            }
            if ui.button("Single step").clicked() {
                self.request_thread_control(DebuggerTaskKind::Step);
            }
        });
        ui.label("Pause/resume owns only one Intimatr suspension of the selected thread; it does not freeze the whole process.");

        ui.columns(2, |columns| {
            columns[0].heading("Process threads");
            egui::ScrollArea::vertical().max_height(500.0).show(&mut columns[0], |ui| {
                for thread in self.threads.clone() {
                    let paused = self
                        .debugger_status
                        .as_ref()
                        .is_some_and(|status| status.paused_threads.contains(&thread.thread_id));
                    let label = if paused {
                        format!("{}  [paused]", thread.thread_id)
                    } else {
                        thread.thread_id.to_string()
                    };
                    if ui
                        .selectable_label(self.selected_thread == Some(thread.thread_id), label)
                        .clicked()
                    {
                        self.selected_thread = Some(thread.thread_id);
                    }
                }
            });

            columns[1].heading("Register snapshot");
            if let Some(snapshot) = &self.registers {
                columns[1].label(format!("Thread {}", snapshot.thread_id));
                columns[1].label(format!("RIP  0x{:016X}", snapshot.instruction_pointer));
                columns[1].label(format!("RSP  0x{:016X}", snapshot.stack_pointer));
                columns[1].label(format!("EFLAGS  0x{:08X}", snapshot.flags));
                if columns[1].button("Disassemble RIP").clicked() {
                    self.disassembly_address = format!("0x{:X}", snapshot.instruction_pointer);
                    self.tab = DebuggerTab::Disassembly;
                }
                columns[1].separator();
                egui::Grid::new("debugger-register-grid")
                    .num_columns(2)
                    .striped(true)
                    .show(&mut columns[1], |ui| {
                        for register in &snapshot.registers {
                            ui.monospace(&register.name);
                            ui.monospace(format!("0x{:016X}", register.value));
                            ui.end_row();
                        }
                    });
            } else {
                columns[1].label("Select a thread and capture its context.");
            }
        });
    }

    fn render_disassembly(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Address");
            ui.text_edit_singleline(&mut self.disassembly_address);
            ui.label("Bitness");
            egui::ComboBox::from_id_salt("debugger-bitness")
                .selected_text(self.disassembly_bitness.to_string())
                .show_ui(ui, |ui| {
                    for bitness in [16_u32, 32, 64] {
                        ui.selectable_value(&mut self.disassembly_bitness, bitness, bitness.to_string());
                    }
                });
            ui.label("Bytes");
            ui.add(egui::TextEdit::singleline(&mut self.disassembly_bytes).desired_width(70.0));
            ui.label("Instructions");
            ui.add(
                egui::TextEdit::singleline(&mut self.disassembly_instructions).desired_width(70.0),
            );
            if ui.button("Disassemble").clicked() {
                self.request_disassembly();
            }
        });
        ui.label("Click an instruction to copy its address into the breakpoint form.");
        egui::ScrollArea::both().show(ui, |ui| {
            egui::Grid::new("debugger-disassembly-grid")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    ui.strong("Address");
                    ui.strong("Bytes");
                    ui.strong("Instruction");
                    ui.end_row();
                    for line in self.disassembly.clone() {
                        if ui
                            .selectable_label(false, format!("0x{:016X}", line.address))
                            .clicked()
                        {
                            self.breakpoint_address = format!("0x{:X}", line.address);
                        }
                        ui.monospace(format_hex(&line.bytes));
                        ui.monospace(line.text);
                        ui.end_row();
                    }
                });
        });
    }

    fn render_breakpoints(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(format!(
                "Thread: {}",
                self.selected_thread
                    .map_or_else(|| "none".to_owned(), |id| id.to_string())
            ));
            ui.label("Address");
            ui.text_edit_singleline(&mut self.breakpoint_address);
            ui.label("Kind");
            egui::ComboBox::from_id_salt("debugger-breakpoint-kind")
                .selected_text(breakpoint_kind_label(self.breakpoint_kind))
                .show_ui(ui, |ui| {
                    for kind in [
                        HardwareBreakpointKind::Execute,
                        HardwareBreakpointKind::Write,
                        HardwareBreakpointKind::ReadWrite,
                    ] {
                        ui.selectable_value(
                            &mut self.breakpoint_kind,
                            kind,
                            breakpoint_kind_label(kind),
                        );
                    }
                });
            if self.breakpoint_kind != HardwareBreakpointKind::Execute {
                ui.label("Size");
                egui::ComboBox::from_id_salt("debugger-breakpoint-size")
                    .selected_text(self.breakpoint_size.to_string())
                    .show_ui(ui, |ui| {
                        for size in [1_u8, 2, 4, 8] {
                            ui.selectable_value(&mut self.breakpoint_size, size, size.to_string());
                        }
                    });
            }
            if ui.button("Add").clicked() {
                self.request_add_breakpoint();
            }
            if ui.button("Refresh").clicked() {
                self.submit(
                    DebuggerTaskKind::ListBreakpoints,
                    Command::ListHardwareBreakpoints,
                );
            }
        });
        ui.label("x86/x64 exposes four DR0–DR3 address slots per thread; Intimatr never patches game code for these breakpoints.");
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("debugger-breakpoint-grid")
                .num_columns(7)
                .striped(true)
                .show(ui, |ui| {
                    for header in ["ID", "Thread", "DR", "Address", "Kind", "Size", ""] {
                        ui.strong(header);
                    }
                    ui.end_row();
                    for breakpoint in self.breakpoints.clone() {
                        ui.label(breakpoint.id.to_string());
                        ui.label(breakpoint.thread_id.to_string());
                        ui.label(format!("DR{}", breakpoint.slot));
                        ui.monospace(format!("0x{:016X}", breakpoint.address));
                        ui.label(breakpoint_kind_label(breakpoint.kind));
                        ui.label(breakpoint.size.to_string());
                        if ui.button("Remove").clicked() {
                            self.submit(
                                DebuggerTaskKind::RemoveBreakpoint,
                                Command::RemoveHardwareBreakpoint {
                                    breakpoint_id: breakpoint.id,
                                },
                            );
                        }
                        ui.end_row();
                    }
                });
        });
    }

    fn render_events(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(format!("Latest sequence: {}", self.last_event_sequence));
            if ui.button("Clear local history").clicked() {
                self.events.clear();
            }
            if ui.button("Poll now").clicked() {
                self.last_event_poll = Instant::now() - Duration::from_millis(self.config.event_poll_ms);
                self.poll_events();
            }
        });
        ui.label("Hardware-breakpoint and single-step events are recorded by a narrow VEH and auto-continue; they are also available through RPC polling.");
        egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
            egui::Grid::new("debugger-event-grid")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    ui.strong("Seq");
                    ui.strong("Thread");
                    ui.strong("Address");
                    ui.strong("Event");
                    ui.end_row();
                    for event in &self.events {
                        ui.label(event.sequence.to_string());
                        ui.label(event.thread_id.to_string());
                        ui.monospace(format!("0x{:016X}", event.address));
                        ui.label(format!("{:?}", event.kind));
                        ui.end_row();
                    }
                });
        });
    }

    fn handle_visibility_and_shutdown(&mut self, context: &egui::Context) {
        let key_down = (unsafe { GetAsyncKeyState(self.toggle_key) } as u16 & 0x8000) != 0;
        if key_down && !self.toggle_key_down {
            self.visible = !self.visible;
            context.send_viewport_cmd(egui::ViewportCommand::Visible(self.visible));
            context.request_repaint();
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
    }
}

impl eframe::App for DebuggerApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_visibility_and_shutdown(context);
        self.drain_responses();
        self.poll_events();

        egui::CentralPanel::default().show(context, |ui| {
            self.render_top_bar(ui);
            match self.tab {
                DebuggerTab::Threads => self.render_threads(ui),
                DebuggerTab::Disassembly => self.render_disassembly(ui),
                DebuggerTab::Breakpoints => self.render_breakpoints(ui),
                DebuggerTab::Events => self.render_events(ui),
            }
            ui.separator();
            ui.label(format!("Status: {}", self.status_text));
        });
        context.request_repaint_after(HOTKEY_POLL_INTERVAL);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let state = PersistedDebuggerUiState {
            tab: self.tab,
            disassembly_address: self.disassembly_address.clone(),
            disassembly_bitness: self.disassembly_bitness,
            breakpoint_address: self.breakpoint_address.clone(),
            breakpoint_kind: self.breakpoint_kind,
            breakpoint_size: self.breakpoint_size,
        };
        eframe::set_value(storage, DEBUGGER_UI_STATE_KEY, &state);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
enum DebuggerTab {
    #[default]
    Threads,
    Disassembly,
    Breakpoints,
    Events,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct PersistedDebuggerUiState {
    tab: DebuggerTab,
    disassembly_address: String,
    disassembly_bitness: u32,
    breakpoint_address: String,
    breakpoint_kind: HardwareBreakpointKind,
    breakpoint_size: u8,
}

impl Default for PersistedDebuggerUiState {
    fn default() -> Self {
        Self {
            tab: DebuggerTab::Threads,
            disassembly_address: "0x0".to_owned(),
            disassembly_bitness: 64,
            breakpoint_address: "0x0".to_owned(),
            breakpoint_kind: HardwareBreakpointKind::Execute,
            breakpoint_size: 1,
        }
    }
}

fn parse_address(input: &str) -> Result<u64, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Address must not be empty".to_owned());
    }
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).map_err(|_| format!("Invalid hexadecimal address: {input}"))
    } else {
        u64::from_str_radix(trimmed, 16)
            .or_else(|_| trimmed.parse::<u64>())
            .map_err(|_| format!("Invalid address: {input}"))
    }
}

fn parse_virtual_key(input: &str) -> Result<i32, DebuggerUiError> {
    let normalized = input.trim().to_ascii_uppercase();
    let key = match normalized.as_str() {
        "INSERT" => 0x2D,
        "DELETE" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" | "PAGE_UP" => 0x21,
        "PAGEDOWN" | "PAGE_DOWN" => 0x22,
        "PAUSE" => 0x13,
        value if value.len() == 1 => {
            let byte = value.as_bytes()[0];
            if byte.is_ascii_alphanumeric() {
                i32::from(byte)
            } else {
                return Err(DebuggerUiError::InvalidToggleKey(input.to_owned()));
            }
        }
        value if value.starts_with('F') => {
            let number = value[1..]
                .parse::<i32>()
                .map_err(|_| DebuggerUiError::InvalidToggleKey(input.to_owned()))?;
            if !(1..=24).contains(&number) {
                return Err(DebuggerUiError::InvalidToggleKey(input.to_owned()));
            }
            0x70 + number - 1
        }
        _ => return Err(DebuggerUiError::InvalidToggleKey(input.to_owned())),
    };
    Ok(key)
}

fn sanitize_app_id(input: &str) -> String {
    input
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn format_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn breakpoint_kind_label(kind: HardwareBreakpointKind) -> &'static str {
    match kind {
        HardwareBreakpointKind::Execute => "Execute",
        HardwareBreakpointKind::Write => "Write",
        HardwareBreakpointKind::ReadWrite => "Read/Write",
    }
}

fn task_label(kind: DebuggerTaskKind) -> &'static str {
    match kind {
        DebuggerTaskKind::ListThreads => "thread enumeration",
        DebuggerTaskKind::Registers => "register snapshot",
        DebuggerTaskKind::Disassemble => "disassembly",
        DebuggerTaskKind::Status => "debugger status",
        DebuggerTaskKind::Pause => "pause",
        DebuggerTaskKind::Resume => "resume",
        DebuggerTaskKind::Step => "single step",
        DebuggerTaskKind::AddBreakpoint => "add breakpoint",
        DebuggerTaskKind::RemoveBreakpoint => "remove breakpoint",
        DebuggerTaskKind::ListBreakpoints => "breakpoint list",
        DebuggerTaskKind::Events => "event poll",
    }
}

#[derive(Debug, Error)]
pub enum DebuggerUiError {
    #[error("failed to create debugger UI persistence directory {path}")]
    PersistenceDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid debugger UI toggle key {0}")]
    InvalidToggleKey(String),
    #[error("failed to spawn debugger UI thread: {0}")]
    ThreadSpawn(#[source] std::io::Error),
    #[error("debugger UI thread panicked")]
    ThreadPanicked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_debugger_hotkeys() {
        assert_eq!(parse_virtual_key("F10").unwrap(), 0x79);
        assert_eq!(parse_virtual_key("Insert").unwrap(), 0x2D);
        assert!(parse_virtual_key("F25").is_err());
    }

    #[test]
    fn parses_hex_and_decimal_addresses() {
        assert_eq!(parse_address("0x1234").unwrap(), 0x1234);
        assert_eq!(parse_address("1234").unwrap(), 0x1234);
    }
}
