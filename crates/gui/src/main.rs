use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::Duration,
};

use eframe::egui;
use pathtag_core::{ChangeInfo, tag_dir_images};
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    layer::{Context, Layer},
    prelude::*,
};

// ------------------------------------------------------------
// Logging
// ------------------------------------------------------------

enum WorkerMessage {
    Log(String),
    Finished(Result<ChangeInfo, String>),
}

struct GuiLogLayer {
    tx: Sender<WorkerMessage>,
    ctx: egui::Context,
}

impl<S: Subscriber> Layer<S> for GuiLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        let line = format!(
            "[{}] {}: {}",
            metadata.level(),
            metadata.target(),
            visitor.message,
        );

        // The channel is unbounded so logging never blocks the UI.
        let _ = self.tx.send(WorkerMessage::Log(line));

        // Ask egui to repaint when a new event arrives.
        self.ctx.request_repaint();
    }
}

#[derive(Default)]
struct FieldVisitor {
    message: String,
}

impl tracing::field::Visit for FieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.push_field(field.name(), value);
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.push_field(field.name(), &format!("{value:?}"));
    }
}

impl FieldVisitor {
    fn push_field(&mut self, name: &str, value: &str) {
        use std::fmt::Write as _;

        if name == "message" {
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            self.message.push_str(value);
        } else {
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            let _ = write!(&mut self.message, "{name}={value}");
        }
    }
}

// ------------------------------------------------------------
// State for a single run
// ------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunStatus {
    Running,
    Success,
    Failed,
}

struct RunResult {
    rx: Receiver<WorkerMessage>,
    logs: Vec<String>,
    status: RunStatus,
    changes: Option<ChangeInfo>,
    error: Option<String>,
}

impl RunResult {
    fn new(rx: Receiver<WorkerMessage>) -> Self {
        Self {
            rx,
            logs: Vec::new(),
            status: RunStatus::Running,
            changes: None,
            error: None,
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        loop {
            match self.rx.try_recv() {
                Ok(WorkerMessage::Log(line)) => {
                    self.logs.push(line);
                    ctx.request_repaint();
                }

                Ok(WorkerMessage::Finished(Ok(changes))) => {
                    self.status = RunStatus::Success;

                    self.logs.push(format!(
                        "Completed successfully: {} changed, {} unchanged.",
                        changes.changed_files.len(),
                        changes.unchanged_files.len(),
                    ));

                    self.changes = Some(changes);
                    ctx.request_repaint();
                }

                Ok(WorkerMessage::Finished(Err(error))) => {
                    self.status = RunStatus::Failed;
                    self.logs.push(format!("ERROR: {error}"));
                    self.error = Some(error);
                    ctx.request_repaint();
                }

                Err(TryRecvError::Empty) => break,

                Err(TryRecvError::Disconnected) => {
                    if self.status == RunStatus::Running {
                        self.status = RunStatus::Failed;
                        self.error = Some("Worker stopped without returning a result.".into());
                    }
                    break;
                }
            }
        }
    }
}

// ------------------------------------------------------------
// Application
// ------------------------------------------------------------

struct ImageTaggerApp {
    input_dir: String,
    output_dir: String,
    force_output_dir: bool,
    validation_error: Option<String>,

    result: Option<RunResult>,
    results_open: bool,
}

impl Default for ImageTaggerApp {
    fn default() -> Self {
        Self {
            input_dir: String::new(),
            output_dir: String::new(),
            force_output_dir: false,
            validation_error: None,
            result: None,
            results_open: false,
        }
    }
}

impl ImageTaggerApp {
    fn browse_directory(path: &mut String) {
        if let Some(selected) = rfd::FileDialog::new().pick_folder() {
            *path = selected.display().to_string();
        }
    }

    fn directory_input(ui: &mut egui::Ui, label: &str, path: &mut String) {
        ui.horizontal(|ui| {
            ui.label(label);

            ui.add(
                egui::TextEdit::singleline(path)
                    .desired_width(390.0)
                    .hint_text("Choose a directory..."),
            );

            if ui.button("Browse...").clicked() {
                Self::browse_directory(path);
            }
        });
    }

    fn is_running(&self) -> bool {
        self.result
            .as_ref()
            .is_some_and(|r| r.status == RunStatus::Running)
    }

    fn start(&mut self, ctx: &egui::Context) {
        self.validation_error = None;

        let input = PathBuf::from(self.input_dir.trim());
        let output = PathBuf::from(self.output_dir.trim());

        if self.input_dir.trim().is_empty() {
            self.validation_error = Some("Please select an input directory.".into());
            return;
        }

        if self.output_dir.trim().is_empty() {
            self.validation_error = Some("Please select an output directory.".into());
            return;
        }

        if !input.is_dir() {
            self.validation_error = Some(format!(
                "Input directory does not exist: {}",
                input.display(),
            ));
            return;
        }

        let force = self.force_output_dir;

        // One channel carries both log entries and the final result.
        let (tx, rx) = mpsc::channel::<WorkerMessage>();

        self.result = Some(RunResult::new(rx));
        self.results_open = true;

        let gui_ctx = ctx.clone();

        thread::spawn(move || {
            let layer = GuiLogLayer {
                tx: tx.clone(),
                ctx: gui_ctx.clone(),
            };

            // Install a subscriber for this worker thread, without
            // claiming the process-wide global subscriber.
            let subscriber = tracing_subscriber::registry().with(layer);

            let result = tracing::subscriber::with_default(subscriber, || {
                tracing::info!(
                    input_dir = %input.display(),
                    output_dir = %output.display(),
                    force_output_dir = force,
                    "Starting image tagging"
                );

                tag_dir_images(&input, &output, force).map_err(|error| format!("{error:#}"))
            });

            let _ = tx.send(WorkerMessage::Finished(result));
            gui_ctx.request_repaint();
        });
    }
}

// ------------------------------------------------------------
// Main UI
// ------------------------------------------------------------

impl eframe::App for ImageTaggerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(result) = &mut self.result {
            result.poll(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Image Tagger");
            ui.label("Add metadata tags to images in a directory.");

            ui.add_space(12.0);

            Self::directory_input(ui, "Input directory:", &mut self.input_dir);

            Self::directory_input(ui, "Output directory:", &mut self.output_dir);

            ui.add_space(8.0);

            ui.checkbox(
                &mut self.force_output_dir,
                "Allow overwriting the output directory",
            );

            ui.add_space(12.0);

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.is_running(), egui::Button::new("Go"))
                    .clicked()
                {
                    self.start(ctx);
                }

                if self.result.is_some() && ui.button("Show results").clicked() {
                    self.results_open = true;
                }

                if self.is_running() {
                    ui.spinner();
                    ui.label("Processing images...");
                }
            });

            if let Some(error) = &self.validation_error {
                ui.colored_label(egui::Color32::RED, error);
            }
        });

        // Results window is independent of the main window.
        if self.results_open {
            if let Some(result) = &mut self.result {
                egui::Window::new("Image Tagging Results")
                    .open(&mut self.results_open)
                    .default_size([820.0, 600.0])
                    .min_size([480.0, 300.0])
                    .resizable(true)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| match result.status {
                            RunStatus::Running => {
                                ui.spinner();
                                ui.label("Processing...");
                            }

                            RunStatus::Success => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(40, 180, 70),
                                    egui::RichText::new("✓ Success").strong().size(18.0),
                                );
                            }

                            RunStatus::Failed => {
                                ui.colored_label(
                                    egui::Color32::RED,
                                    egui::RichText::new("✗ Failed").strong().size(18.0),
                                );
                            }
                        });

                        if let Some(error) = &result.error {
                            ui.colored_label(egui::Color32::RED, error);
                        }

                        if let Some(changes) = &result.changes {
                            ui.label(format!("Changed files: {}", changes.changed_files.len(),));

                            ui.label(format!(
                                "Unchanged files: {}",
                                changes.unchanged_files.len(),
                            ));

                            egui::CollapsingHeader::new(format!(
                                "Changed files ({})",
                                changes.changed_files.len(),
                            ))
                            .show(ui, |ui| {
                                Self::file_list(ui, &changes.changed_files);
                            });

                            egui::CollapsingHeader::new(format!(
                                "Unchanged files ({})",
                                changes.unchanged_files.len(),
                            ))
                            .show(ui, |ui| {
                                Self::file_list(ui, &changes.unchanged_files);
                            });
                        }

                        ui.separator();

                        ui.horizontal(|ui| {
                            ui.heading("Logs");
                            ui.label(format!("{} entries", result.logs.len(),));

                            if ui.button("Copy logs").clicked() {
                                ctx.copy_text(result.logs.join("\n"));
                            }

                            if ui.button("Clear logs").clicked() {
                                result.logs.clear();
                            }
                        });

                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                for (index, line) in result.logs.iter().enumerate() {
                                    ui.push_id(index, |ui| {
                                        ui.add(
                                            egui::Label::new(egui::RichText::new(line).monospace())
                                                .selectable(true),
                                        );
                                    });
                                }
                            });
                    });
            }
        }

        // Continue refreshing while work is in progress.
        if self.is_running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

impl ImageTaggerApp {
    fn file_list(ui: &mut egui::Ui, files: &BTreeSet<String>) {
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .show(ui, |ui| {
                for file in files {
                    ui.label(egui::RichText::new(file).monospace());
                }
            });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 280.0])
            .with_min_inner_size([580.0, 220.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Image Tagger",
        options,
        Box::new(|_cc| Ok(Box::new(ImageTaggerApp::default()))),
    )
}
