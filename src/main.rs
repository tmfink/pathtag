use std::{
    collections::BTreeSet,
    env::set_current_dir,
    io::BufRead,
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::Context;
use clap::Parser;
use tracing::{debug, info, level_filters::LevelFilter, trace, warn};
use walkdir::WalkDir;

enum ImageCreationResult {
    NewTagsAdded,
    ImageNotChanged,
}

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

fn sanity_check_inout_paths(input_dir: &Path, output_dir: &Path) -> anyhow::Result<()> {
    if !input_dir.is_dir() {
        anyhow::bail!("Input dir must be a directory");
    }

    if input_dir == output_dir {
        anyhow::bail!("Input and output dirs must be different");
    }

    if input_dir.starts_with(output_dir) || output_dir.starts_with(input_dir) {
        anyhow::bail!("Neither input dir nor output dir can be as subdir of the other");
    }

    Ok(())
}

fn cmd_output_to_indented(output: &[u8]) -> String {
    let mut output_indented = String::with_capacity(output.len());
    for line in output.lines() {
        let line = match line {
            Err(err) => {
                warn!(?err, "Failed to convert output to UTF-8");
                break;
            }
            Ok(line) => line,
        };
        output_indented.push_str(&format!(" > {}\n", line));
    }
    output_indented
}

fn dir_path_to_tags(path: &Path) -> anyhow::Result<BTreeSet<String>> {
    let mut tags = BTreeSet::new();
    for comp in path.components() {
        let path_str = comp
            .as_os_str()
            .to_str()
            .with_context(|| {
                format!(
                    "Failed to convert path to string for path {} component {:?}",
                    path.display(),
                    comp
                )
            })?
            .to_lowercase();
        if path_str == "." {
            continue;
        }
        let comp_tags_iter = path_str.split(",").filter_map(|s| {
            let s = s.trim();
            if !s.is_empty() {
                Some(s.to_string())
            } else {
                None
            }
        });
        tags.extend(comp_tags_iter);
    }
    Ok(tags)
}

fn add_image_file_tags(
    original_image_path: &Path,
    new_image_path: &Path,
    new_tags: &BTreeSet<String>,
) -> anyhow::Result<ImageCreationResult> {
    let mut res = ImageCreationResult::NewTagsAdded;

    let new_image_path_dir = new_image_path.parent().with_context(|| {
        format!(
            "Failed to get parent dir for {:?}",
            new_image_path.display()
        )
    })?;
    std::fs::create_dir_all(new_image_path_dir).with_context(|| {
        format!(
            "Failed to create new image dir {:?}",
            new_image_path_dir.display()
        )
    })?;
    std::fs::copy(original_image_path, new_image_path).with_context(|| {
        format!(
            "Failed to copy original image {:?} to new path {:?}",
            original_image_path.display(),
            new_image_path.display()
        )
    })?;

    // exiv2 -M'set Xmp.digiKam.TagsList XmpSeq tag1' -M'set Xmp.digiKam.TagsList XmpSeq tag2' output.jpg
    let mut cmd = Command::new("exiv2");
    for tag in new_tags {
        cmd.arg(format!("-Mset Xmp.digiKam.TagsList XmpSeq {tag}"));
    }
    cmd.arg(new_image_path);
    let output = cmd.output().with_context(|| {
        format!(
            "failed to execute exiv2 to add tags image {:?}",
            new_image_path.display()
        )
    })?;
    if !output.status.success() {
        // This can fail if the file does not have EXIF tags, so this is not fatal
        res = ImageCreationResult::ImageNotChanged;
        warn!(
            "exiv2 command to add tags image {:?}, exited with status {}\
            \nstdout:\
            \n{}\
            \nstderr:\
            \n{}",
            new_image_path.display(),
            output.status,
            cmd_output_to_indented(&output.stdout),
            cmd_output_to_indented(&output.stderr),
        );
    }

    Ok(res)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut new_path = PathBuf::new();

    for comp in path.components() {
        match comp {
            Component::CurDir => continue,
            Component::ParentDir => {
                new_path.pop();
            }
            _ => {
                new_path.push(comp);
            }
        }
    }
    new_path
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    set_logging(args.verbose);
    info!("Args: {args:?}");

    // Sanity check neither input nor output dir are subdirs of each other
    let input_dir = args
        .input_dir
        .canonicalize()
        .context("Unable to canonicalize input dir")?;
    let output_dir = args
        .output_dir
        .canonicalize()
        .context("Unable to canonicalize input dir")?;
    drop(args.input_dir);
    drop(args.output_dir);

    sanity_check_inout_paths(&input_dir, &output_dir)?;

    if args.force {
        std::fs::remove_dir_all(&output_dir).context("Failed to remove output dir")?;
    }
    std::fs::create_dir(&output_dir).context("Failed to create output directory")?;

    let mut changed_files: BTreeSet<String> = Default::default();
    let mut unchanged_files: BTreeSet<String> = Default::default();

    set_current_dir(input_dir).context("Failed to change directory to input directory")?;
    for relative_entry in WalkDir::new(".") {
        let relative_entry = relative_entry.context("Failed to read input file")?;
        debug!("Found entry: {:?}", &relative_entry.path().display());

        if !relative_entry.file_type().is_file() {
            debug!("skipping; not a file");
            continue;
        }

        let relative_path = normalize_path(relative_entry.path());
        drop(relative_entry);

        let new_file = {
            let mut new_file = output_dir.clone();
            new_file.push(&relative_path);
            new_file
        };
        info!(
            "Copying {} to {}",
            relative_path.canonicalize().unwrap().display(),
            new_file.display()
        );
        let parent_dir = relative_path.parent().with_context(|| {
            format!(
                "No parent dir found for file {}",
                relative_path.canonicalize().unwrap().display()
            )
        })?;
        let tags = dir_path_to_tags(parent_dir).with_context(|| {
            format!("Failed to get tags for directory {}", parent_dir.display())
        })?;
        trace!("tags: {tags:?}");

        // XXX: get image current tags
        // XXX: merge tags
        // XXX: ignore non-image file

        let file_set = match add_image_file_tags(&relative_path, &new_file, &tags)? {
            ImageCreationResult::ImageNotChanged => &mut unchanged_files,
            ImageCreationResult::NewTagsAdded => &mut changed_files,
        };
        let path_str = format!("{}", relative_path.display());
        file_set.insert(path_str);
    }

    println!("Succeeded adding tags for files:");
    for file in changed_files {
        println!("    {:?}", file);
    }

    println!("\nFailed to add tags to file:");
    for file in unchanged_files {
        println!("    {:?}", file);
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_normalize_path() {
        fn t(input: &str, expected_output: &str) {
            assert_eq!(
                normalize_path(&PathBuf::from(input)).to_string_lossy(),
                expected_output
            );
        }
        t("./foo", "foo");
        t("foo", "foo");
        t("/a/b/c", "/a/b/c");
        t(".bar/././foo", ".bar/foo");
        t(".bar/../foo", "foo");
    }

    // XXX: add path -> tag tests
}
