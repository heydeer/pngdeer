//! Running a command: each file read, compressed, written and reported.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use super::{
    args::Command,
    encode::{encode, is_jpeg, named_by, Format, PNG_SIGNATURE},
    files::{destination, file_id, images_under, Destination, FileId, Input},
    report::{json_line, plain, verbose_line},
    tools::{score, upscale},
    usage_error, Error,
};

impl Command {
    /// Runs it; the exit code.
    pub fn run(&self) -> i32 {
        let helper = if let Some(other) = &self.score_against {
            Some(score(&self.files[0], other))
        } else {
            self.upscale.map(|k| upscale(&self.files[0], self.output.as_deref().unwrap_or_default(), k))
        };
        if let Some(result) = helper {
            return match result {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("slimdeer: {e}");
                    1
                }
            };
        }
        if let Some(dir) = &self.dir {
            // -d's copy of a tree keeps the inputs' names, so in the tree's own folder it would
            // write over them (each file is checked again before it is written)
            if self.recursive && self.files.iter().any(|file| same_folder(Path::new(file), dir)) {
                return usage_error(&format!("-d {} is a folder -r reads: its images would be replaced", dir.display()));
            }
            if let Err(e) = fs::create_dir_all(dir) {
                eprintln!("slimdeer: {}: {}", dir.display(), plain(e));
                return 1;
            }
        }
        let (inputs, mut failed) = self.inputs();
        let mut files =
            Files { inputs: inputs.iter().filter(|input| !input.is_stdin()).filter_map(|input| file_id(&input.path)).collect(), outputs: HashMap::new() };
        for input in &inputs {
            if let Err(e) = self.compress(input, &mut files) {
                eprintln!("slimdeer: {}: {e}", input.name());
                failed = true;
            }
        }
        failed as i32
    }

    /// The files to compress: each FILE, and with -r the images in each FILE that is a folder; and
    /// whether any FILE, or folder in one, failed (said).
    fn inputs(&self) -> (Vec<Input>, bool) {
        let (mut inputs, mut failed) = (Vec::new(), false);
        // -d's folder inside a tree holds outputs under the inputs' own names: not inputs again
        let skip = self.dir.as_deref().and_then(file_id);
        for file in &self.files {
            let path = PathBuf::from(file);
            if file == "-" || !path.is_dir() {
                inputs.push(Input { path, root: None });
                continue;
            }
            if !self.recursive {
                eprintln!("slimdeer: {file}: a folder; -r compresses the images in it");
                failed = true;
                continue;
            }
            let (found, unread) = images_under(&path, skip.as_ref());
            for (folder, e) in unread {
                eprintln!("slimdeer: {}: {}", folder.display(), plain(e));
                failed = true;
            }
            inputs.extend(found.into_iter().map(|image| Input { path: image, root: Some(path.clone()) }));
        }
        (inputs, failed)
    }

    /// One file: compressed, written, and reported as asked.
    fn compress(&self, input: &Input, files: &mut Files) -> Result<(), Error> {
        let start = Instant::now();
        let bytes = if input.is_stdin() {
            let mut bytes = Vec::new();
            io::stdin().read_to_end(&mut bytes).map_err(plain)?;
            bytes
        } else {
            fs::read(&input.path).map_err(plain)?
        };
        let jpeg = is_jpeg(&bytes);
        if !jpeg && !bytes.starts_with(PNG_SIGNATURE) {
            return Err("not a PNG or a JPEG".into());
        }
        let own = if jpeg { Format::Jpeg } else { Format::Png };
        let format = self.format.or(named_by(self.output.as_deref())).unwrap_or(own);
        let destination = destination(input, self.output.as_deref(), self.dir.as_deref(), format, format == own);
        match &destination {
            Destination::Stdout if io::stdout().is_terminal() => {
                return Err("an image can't go to the terminal: redirect stdout, or use -o".into());
            }
            Destination::File(path, checked) => {
                if let Some(id) = file_id(path) {
                    if let Some(owner) = files.outputs.get(&id) {
                        return Err(format!("{} is already {owner}'s output; give one of them another name or folder", path.display()).into());
                    }
                    if *checked && files.inputs.contains(&id) {
                        if file_id(&input.path).as_ref() == Some(&id) {
                            return Err(format!("{} would replace the input; name another with -o", path.display()).into());
                        }
                        return Err(format!("{} is another input, which this would replace", path.display()).into());
                    }
                }
            }
            _ => {}
        }
        let (output, report, unchanged) = encode(&bytes, jpeg, format, &self.settings)?;
        match &destination {
            Destination::Stdout => io::stdout().lock().write_all(&output).map_err(plain)?,
            Destination::File(path, _) => {
                if input.root.is_some() {
                    if let Some(folder) = path.parent() {
                        fs::create_dir_all(folder).map_err(|e| format!("{}: {}", folder.display(), plain(e)))?;
                    }
                }
                fs::write(path, &output).map_err(|e| format!("{}: {}", path.display(), plain(e)))?;
                if let Some(id) = file_id(path) {
                    files.outputs.insert(id, input.name());
                }
            }
        }
        if self.verbose {
            let seconds = start.elapsed().as_secs_f64();
            eprintln!("{}", verbose_line(&input.name(), &destination.shown(), bytes.len(), output.len(), unchanged, seconds));
        }
        if self.json {
            let name = if input.is_stdin() { "-".into() } else { input.path.display().to_string() };
            println!("{}", json_line(&report, &name, &destination.name(), bytes.len()));
        }
        Ok(())
    }
}

/// The files a run reads, and those it has written and whose they are: a name slimdeer makes up
/// mustn't replace an input, nor any output another input's (x.png from two folders -r reads into one -d, X.png and
/// x.png where a volume ignores case, logo.png and logo.jpg as WebP).
struct Files {
    inputs: HashSet<FileId>,
    outputs: HashMap<FileId, String>,
}

/// Whether `a` and `b` are the same folder, however each is spelt; not if either doesn't exist.
fn same_folder(a: &Path, b: &Path) -> bool {
    file_id(a).is_some_and(|a| file_id(b) == Some(a))
}
