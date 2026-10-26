use std::{collections::BTreeSet, path::Path};

use anyhow::Context;
use tracing::{debug, info, trace, warn};
use walkdir::WalkDir;
const XMP_TAGSLIST: &str = "Xmp.digiKam.TagsList";

enum ImageCreationResult {
    MetadataAdded,
    ImageNotChanged,
}

pub struct ChangeInfo {
    pub changed_files: BTreeSet<String>,
    pub unchanged_files: BTreeSet<String>,
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
            // always lowercase
            .to_lowercase();
        if path_str == "." || path_str == "/" {
            continue;
        }
        let comp_tags_iter = path_str.split(",").filter_map(|s| {
            // avoid leading/trailing spaces
            let s = s.trim();
            // ignore empty tags
            if s.is_empty() {
                return None;
            }
            // avoid duplicate spaces between words of a tag
            let tag_words: Vec<&str> = s.split_whitespace().collect();
            Some(tag_words.join(" "))
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
    rexiv2::initialize().context("failed to initialize rexiv2 library")?;

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

    let meta = rexiv2::Metadata::new_from_path(original_image_path)?;
    let mut final_tags_list = new_tags.clone();
    let mut existing_tags_list = meta
        .get_tag_multiple_strings(XMP_TAGSLIST)
        .unwrap_or_default();
    trace!("existing tags: {existing_tags_list:?}");
    final_tags_list.extend(existing_tags_list.drain(0..));
    let final_tags_str_vec: Vec<&str> = final_tags_list.iter().map(|s| s.as_str()).collect();

    if let Err(err) = meta.set_tag_multiple_strings(XMP_TAGSLIST, &final_tags_str_vec) {
        warn!("Failed to add tags to image metadata: {}", err);
        return Ok(ImageCreationResult::ImageNotChanged);
    }

    if let Err(err) = meta.save_to_file(new_image_path) {
        warn!("Failed to save metadata to file: {}", err);
        return Ok(ImageCreationResult::ImageNotChanged);
    }

    Ok(ImageCreationResult::MetadataAdded)
}

pub fn tag_dir_images(
    input_dir: &Path,
    output_dir: &Path,
    force_output_dir: bool,
) -> Result<ChangeInfo, anyhow::Error> {
    let input_abs_dir = input_dir
        .canonicalize()
        .with_context(|| format!("Unable to canonicalize input dir {:?}", input_dir.display()))?;
    let output_abs_dir = output_dir.canonicalize().with_context(|| {
        format!(
            "Unable to canonicalize output dir {:?}",
            output_dir.display()
        )
    })?;

    sanity_check_inout_paths(&input_abs_dir, &output_abs_dir)?;
    if force_output_dir {
        std::fs::remove_dir_all(&output_abs_dir).context("Failed to remove output dir")?;
    }
    std::fs::create_dir(&output_abs_dir).context("Failed to create output directory")?;
    let mut changed_files: BTreeSet<String> = Default::default();
    let mut unchanged_files: BTreeSet<String> = Default::default();
    for input_abs_entry in WalkDir::new(&input_abs_dir) {
        let input_abs_entry = input_abs_entry.context("Failed to read input file")?;
        debug!("Found entry: {:?}", &input_abs_entry.path().display());

        if !input_abs_entry.file_type().is_file() {
            debug!("skipping; not a file");
            continue;
        }

        // XXX: ignore non-image file?

        let relative_path = input_abs_entry
            .path()
            .strip_prefix(&input_abs_dir)
            .context("Failed to find common path")?;
        let path_str = format!("{}", relative_path.display());

        let new_file_path = {
            let mut new_file = output_abs_dir.clone();
            new_file.push(relative_path);
            new_file
        };
        info!(
            "Copying {} to {}",
            input_abs_entry.path().display(),
            new_file_path.display()
        );
        let new_file_parent_dir = &new_file_path.parent().with_context(|| {
            format!(
                "No parent dir found for file {}",
                relative_path.canonicalize().unwrap().display()
            )
        })?;
        let new_tags = dir_path_to_tags(new_file_parent_dir).with_context(|| {
            format!(
                "Failed to get tags for directory {}",
                new_file_parent_dir.display()
            )
        })?;
        trace!("new tags: {new_tags:?}");

        let file_set = match add_image_file_tags(input_abs_entry.path(), &new_file_path, &new_tags)?
        {
            ImageCreationResult::ImageNotChanged => &mut unchanged_files,
            ImageCreationResult::MetadataAdded => &mut changed_files,
        };
        file_set.insert(path_str);
    }
    Ok(ChangeInfo {
        changed_files,
        unchanged_files,
    })
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_dir_path_to_tags() {
        fn t(input: &str, expected_output: &[&str]) {
            let expected_output: BTreeSet<String> = expected_output
                .iter()
                .copied()
                .map(|s| s.to_string())
                .collect();
            assert_eq!(
                dir_path_to_tags(&PathBuf::from(input)).unwrap(),
                expected_output
            );
        }
        t("", &[]);
        t("foo", &["foo"]);
        t("zzz, aaa", &["aaa", "zzz"]);
        t("zzz,aaa", &["aaa", "zzz"]);
        t("zzz ,aaa", &["aaa", "zzz"]);
        t("zzz ,  aaa", &["aaa", "zzz"]);
        t(
            "tag1-word1 tag1-word2,tag2",
            &["tag1-word1 tag1-word2", "tag2"],
        );
        t("foo/bar", &["bar", "foo"]);
        t("foo/bar/baz", &["bar", "baz", "foo"]);
        t("foo,stuff/bar/baz", &["bar", "baz", "foo", "stuff"]);
        t(",,, ,   , ,,,,   ,, hi,, ,, ,, ", &["hi"]);
        t("ALL CAPS", &["all caps"]);
        t("  Too    Many      Spaces ", &["too many spaces"]);
        t("/ignore/abs/dir/", &["ignore", "abs", "dir"]);
    }
}
