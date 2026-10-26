use std::path::PathBuf;

use clap::Parser;
use tracing::{info, level_filters::LevelFilter};

/// Apply EXIF keyword tags based on directory structure
///
/// Makes a copy of the input dir to the output dir with the EXIF tags added
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Input directory with images
    #[arg(short, long)]
    input_dir: PathBuf,

    /// Output directory for images with tags added
    #[arg(short, long)]
    output_dir: PathBuf,

    /// Overwrite output dir if it exists
    #[arg(short, long)]
    force: bool,

    /// Increase verbosity
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

fn set_logging(verbosity: u8) {
    let levels = [
        LevelFilter::OFF,
        LevelFilter::ERROR,
        LevelFilter::WARN,
        LevelFilter::INFO,
        LevelFilter::DEBUG,
        LevelFilter::TRACE,
    ];

    let log_level = levels[(2 + verbosity as usize).min(levels.len() - 1)];

    tracing_subscriber::fmt().with_max_level(log_level).init();
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    set_logging(args.verbose);
    info!("Args: {args:?}");

    let change_info = pathtag_core::tag_dir_images(&args.input_dir, &args.output_dir, args.force)?;

    if !change_info.changed_files.is_empty() {
        println!("Succeeded adding tags for files:");
        for file in change_info.changed_files {
            println!("    {:?}", file);
        }
    }

    if !change_info.unchanged_files.is_empty() {
        println!("\nFailed to add tags to file:");
        for file in change_info.unchanged_files {
            println!("    {:?}", file);
        }
    }

    Ok(())
}
