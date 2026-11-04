//! Which files a command compresses, and where each one's output goes.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::encode::Format;

/// A file to compress: its path ("-" for stdin), and with -r the folder it was found in, whose
/// layout -d's folder copies.
pub struct Input {
    pub path: PathBuf,
    pub root: Option<PathBuf>,
}

impl Input {
    pub fn is_stdin(&self) -> bool {
        self.path.as_os_str() == "-"
    }

    /// As slimdeer names it in what it says.
    pub fn name(&self) -> String {
        if self.is_stdin() {
            "stdin".into()
        } else {
            self.path.display().to_string()
        }
    }
}

/// Where a file's output goes.
pub enum Destination {
    Stdout,
    /// A path, and whether to check it isn't the input before writing over it.
    File(PathBuf, bool),
}

impl Destination {
    /// As --json names it.
    pub fn name(&self) -> String {
        match self {
            Destination::Stdout => "-".into(),
            Destination::File(path, _) => path.display().to_string(),
        }
    }

    /// As -v names it.
    pub fn shown(&self) -> String {
        match self {
            Destination::Stdout => "stdout".into(),
            Destination::File(path, _) => path.display().to_string(),
        }
    }
}

/// The PNG and JPEG files under `root`, by their extension, in order: every folder below it, but
/// not hidden files or folders, nor slimdeer's own NAME.min.EXT outputs, so that running it on a
/// folder again doesn't compress those too.
pub fn images_under(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let path = entry?.path();
            if path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')) {
                continue;
            }
            if path.is_dir() {
                folders.push(path);
            } else if is_image(&path) && !is_output(&path) {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn is_image(path: &Path) -> bool {
    path.extension().is_some_and(|e| ["png", "jpg", "jpeg"].iter().any(|ext| e.eq_ignore_ascii_case(ext)))
}

/// slimdeer's output beside an input: NAME.min.EXT.
fn is_output(path: &Path) -> bool {
    path.file_stem().is_some_and(|stem| stem.to_string_lossy().ends_with(".min"))
}

/// Where `input`'s output goes: -o's, else stdout for stdin, else NAME.EXT in -d's folder (for a file
/// -r found, in the same folder there as under its root) or NAME.min.EXT beside the input. EXT is
/// the input's own, as spelt, where the format stays.
pub fn destination(input: &Input, output: Option<&str>, dir: Option<&Path>, format: Format, same: bool) -> Destination {
    match output {
        Some("-") => return Destination::Stdout,
        Some(output) => return Destination::File(output.into(), false),
        None if input.is_stdin() => return Destination::Stdout,
        None => {}
    }
    let path = &input.path;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = match path.extension() {
        Some(own) if same => own.to_string_lossy(),
        _ => format.extension().into(),
    };
    match (dir, &input.root) {
        // Command::run has checked that -d's folder isn't the one -r reads, so a file from it can't
        // land on itself, and needn't be checked again one at a time
        (Some(dir), Some(root)) => {
            let folder = path.parent().and_then(|parent| parent.strip_prefix(root).ok()).unwrap_or(Path::new(""));
            Destination::File(dir.join(folder).join(format!("{stem}.{extension}")), false)
        }
        (Some(dir), None) => Destination::File(dir.join(format!("{stem}.{extension}")), true),
        (None, _) => Destination::File(path.with_file_name(format!("{stem}.min.{extension}")), true),
    }
}
