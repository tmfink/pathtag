use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::Duration,
};

use clap::Parser;
use eframe::egui;
use pathtag_core::{ChangeInfo, tag_dir_images};
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    layer::{Context, Layer},
    prelude::*,
};

const ICON_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon.png"));

// Args are optional and pre-populate fields if set

/// Apply EXIF keyword tags based on directory structure
///
/// Makes a copy of the input dir to the output dir with the EXIF tags added
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Input directory with images
    #[arg(short, long)]
    input_dir: Option<PathBuf>,

    /// Output directory for images with tags added
    #[arg(short, long)]
    output_dir: Option<PathBuf>,
}

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

enum PreflightMessage {
    Sizes(Result<DiskSpace, String>),
}

struct DiskSpace {
    input_dir_bytes: u64,
    output_dir_bytes: Option<u64>,
}

impl DiskSpace {
    fn additional_space(&self) -> u64 {
        self.input_dir_bytes
            .saturating_sub(self.output_dir_bytes.unwrap_or_default())
    }
}

struct Preflight {
    input: PathBuf,
    output: PathBuf,
    rx: Receiver<PreflightMessage>,
    sizes: Option<DiskSpace>,
    error: Option<String>,
}

fn directory_size(path: &std::path::Path) -> Result<u64, String> {
    let mut total = 0u64;

    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry.map_err(|e| e.to_string())?;

        if entry.file_type().is_file() {
            let metadata = entry.metadata().map_err(|e| e.to_string())?;
            total = total.saturating_add(metadata.len());
        }
    }

    Ok(total)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];

    let mut value = bytes as f64;
    let mut unit = 0;

    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    format!("{value:.2} {}", UNITS[unit])
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
                        changes.success_files.len(),
                        changes.error_files.len(),
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

struct PathTagApp {
    input_dir: String,
    output_dir: String,
    validation_error: Option<String>,

    preflight: Option<Preflight>,
    preflight_open: bool,

    result: Option<RunResult>,
    results_open: bool,
}

impl Default for PathTagApp {
    fn default() -> Self {
        Self {
            input_dir: String::new(),
            output_dir: String::new(),
            validation_error: None,
            preflight: None,
            preflight_open: false,
            result: None,
            results_open: false,
        }
    }
}

impl PathTagApp {
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

    fn begin_preflight(&mut self, ctx: &egui::Context) {
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
                input.display()
            ));
            return;
        }

        // Avoid accidentally treating the input directory as output.
        let canonical_input = input.canonicalize().ok();
        let canonical_output = output.canonicalize().ok();

        if canonical_input.is_some() && canonical_input == canonical_output {
            self.validation_error = Some("Input and output directories must be different.".into());
            return;
        }

        let (tx, rx) = mpsc::channel();

        self.preflight = Some(Preflight {
            input: input.clone(),
            output: output.clone(),
            rx,
            sizes: None,
            error: None,
        });

        self.preflight_open = true;

        let gui_ctx = ctx.clone();

        thread::spawn(move || {
            let result = (|| {
                let input_dir_bytes = directory_size(&input)?;

                let output_dir_bytes = if output.exists() {
                    Some(directory_size(&output)?)
                } else {
                    None
                };

                Ok(DiskSpace {
                    input_dir_bytes,
                    output_dir_bytes,
                })
            })();

            let _ = tx.send(PreflightMessage::Sizes(result));
            gui_ctx.request_repaint();
        });
    }

    fn launch_worker(&mut self, ctx: &egui::Context) {
        let Some(preflight) = self.preflight.take() else {
            return;
        };

        let input = preflight.input;
        let output = preflight.output;

        let (tx, rx) = mpsc::channel::<WorkerMessage>();

        self.result = Some(RunResult::new(rx));
        self.results_open = true;
        self.preflight_open = false;

        let gui_ctx = ctx.clone();

        thread::spawn(move || {
            let layer = GuiLogLayer {
                tx: tx.clone(),
                ctx: gui_ctx.clone(),
            };

            let subscriber = tracing_subscriber::registry().with(layer);

            let result = tracing::subscriber::with_default(subscriber, || {
                tracing::info!(
                    input_dir = %input.display(),
                    output_dir = %output.display(),
                    "Starting image tagging"
                );

                tag_dir_images(&input, &output, true).map_err(|error| format!("{error:#}"))
            });

            let _ = tx.send(WorkerMessage::Finished(result));
            gui_ctx.request_repaint();
        });
    }
}

// ------------------------------------------------------------
// Main UI
// ------------------------------------------------------------

impl eframe::App for PathTagApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(result) = &mut self.result {
            result.poll(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("PathTag");
            ui.label("Add metadata tags to images in a directory.");

            ui.add_space(12.0);

            Self::directory_input(ui, "Input directory:", &mut self.input_dir);

            Self::directory_input(ui, "Output directory:", &mut self.output_dir);

            ui.add_space(12.0);

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.is_running(), egui::Button::new("Go"))
                    .clicked()
                {
                    self.begin_preflight(ctx);
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
                    .default_size([820.0, 800.0])
                    .min_size([480.0, 400.0])
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
                            if !changes.success_files.is_empty() {
                                egui::CollapsingHeader::new(format!(
                                    "Success files ({})",
                                    changes.success_files.len(),
                                ))
                                .show(ui, |ui| {
                                    Self::file_list(ui, &changes.success_files);
                                });
                            }

                            if !changes.error_files.is_empty() {
                                egui::CollapsingHeader::new(format!(
                                    "Error files ({})",
                                    changes.error_files.len(),
                                ))
                                .show(ui, |ui| {
                                    Self::file_list(ui, &changes.error_files);
                                });
                            }
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

        // Show the preflight confirmation dialog, if open.
        self.show_preflight(ctx);

        // Keep the UI responsive while directory sizes are calculated.
        if self.preflight_open
            && self
                .preflight
                .as_ref()
                .is_some_and(|p| p.sizes.is_none() && p.error.is_none())
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        // Continue refreshing while the tagging operation is running.
        if self.is_running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

impl PathTagApp {
    fn file_list(ui: &mut egui::Ui, files: &BTreeSet<String>) {
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .show(ui, |ui| {
                for file in files {
                    ui.label(egui::RichText::new(file).monospace());
                }
            });
    }

    fn show_preflight(&mut self, ctx: &egui::Context) {
        // Collect completed size calculations without blocking the UI.
        if let Some(preflight) = &mut self.preflight {
            loop {
                match preflight.rx.try_recv() {
                    Ok(PreflightMessage::Sizes(Ok(sizes))) => {
                        preflight.sizes = Some(sizes);
                    }
                    Ok(PreflightMessage::Sizes(Err(error))) => {
                        preflight.error = Some(error);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if preflight.sizes.is_none() && preflight.error.is_none() {
                            preflight.error = Some("Directory scan stopped unexpectedly.".into());
                        }
                        break;
                    }
                }
            }
        }

        if !self.preflight_open {
            return;
        }

        let mut confirm = false;
        let mut cancel = false;

        if let Some(preflight) = &self.preflight {
            let output_exists = preflight.output.exists();
            let scanning = preflight.sizes.is_none() && preflight.error.is_none();

            egui::Window::new("Confirm image tagging")
                .collapsible(false)
                .resizable(false)
                .default_width(520.0)
                .show(ctx, |ui| {
                    ui.label("Review the operation before continuing.");
                    ui.add_space(8.0);

                    ui.label("Input directory:");
                    ui.monospace(preflight.input.display().to_string());

                    ui.label("Output directory:");
                    ui.monospace(preflight.output.display().to_string());

                    ui.add_space(12.0);

                    if output_exists {
                        ui.colored_label(
                            egui::Color32::RED,
                            egui::RichText::new(
                                "WARNING: \
                             Output dir already exists, all existing contents will be deleted.",
                            )
                            .strong(),
                        );
                    } else {
                        ui.label("The output path does not currently exist.");
                    }

                    ui.separator();
                    ui.heading("Disk space estimate");

                    if scanning {
                        ui.spinner();
                        ui.label("Calculating directory sizes...");
                    }

                    if let Some(error) = &preflight.error {
                        ui.colored_label(
                            egui::Color32::RED,
                            format!("Could not calculate directory sizes: {error}"),
                        );
                    }

                    if let Some(disk_space) = &preflight.sizes {
                        ui.label(format!(
                            "Input directory size: {}",
                            format_bytes(disk_space.input_dir_bytes),
                        ));

                        if let Some(output_dir_bytes) = disk_space.output_dir_bytes {
                            ui.label(format!(
                                "Existing output size: {}",
                                format_bytes(output_dir_bytes),
                            ));
                        }

                        ui.label(format!(
                            "Estimated additional space required: approximately {}",
                            format_bytes(disk_space.additional_space()),
                        ));

                        ui.small(
                            "Estimate based on the total size of input files. \
                         Actual space use may differ.",
                        );
                    }

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Back").clicked() {
                            cancel = true;
                        }

                        let can_confirm = !scanning && preflight.error.is_none();

                        if ui
                            .add_enabled(can_confirm, egui::Button::new("Continue"))
                            .clicked()
                        {
                            confirm = true;
                        }
                    });
                });
        }

        if cancel {
            self.preflight_open = false;
            self.preflight = None;
        }

        if confirm {
            self.launch_worker(ctx);
        }
    }
}

fn main() -> eframe::Result {
    let args = Args::parse();

    let input_dir = args
        .input_dir
        .map(|p| {
            p.canonicalize()
                .expect("Failed to canonicalize input_dir")
                .into_string()
                .expect("Failed to convert input_dir to UTF-8")
        })
        .unwrap_or_default();
    let output_dir = args
        .output_dir
        .map(|p| {
            p.canonicalize()
                .expect("Failed to canonicalize output_dir")
                .into_string()
                .expect("Failed to convert output_dir to UTF-8")
        })
        .unwrap_or_default();

    let icon =
        eframe::icon_data::from_png_bytes(ICON_BYTES).expect("Failed to parse runtime icon PNG");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 500.0])
            .with_min_inner_size([580.0, 220.0])
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "PathTag",
        options,
        Box::new(|_cc| {
            Ok(Box::new(PathTagApp {
                input_dir,
                output_dir,
                ..PathTagApp::default()
            }))
        }),
    )
}
