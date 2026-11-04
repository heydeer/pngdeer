//! The command line: where outputs go, what it says, and how it fails.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use slimdeer::{codec, RGBA};

/// A 96×80 PNG of flat blocks with fine noise, which a palette makes much smaller.
fn sample() -> Vec<u8> {
    let (w, h) = (96, 80);
    let colors = [(200, 60, 40), (30, 120, 220), (240, 230, 210), (20, 20, 30), (90, 180, 90)];
    let mut seed = 7u32;
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let noise = ((seed >> 16) % 9) as u8;
            let (r, g, b) = colors[(x / 16 + y / 20) % colors.len()];
            pixels.push(RGBA::new(r + noise, g + noise, b - noise, 255));
        }
    }
    codec::encode_rgba(&pixels, w, h)
}

/// A fresh folder holding sample PNGs under `names`.
fn folder(test: &str, names: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("slimdeer-cli-{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for name in names {
        fs::write(dir.join(name), sample()).unwrap();
    }
    dir
}

fn slimdeer(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_slimdeer")).current_dir(dir).args(args).output().unwrap()
}

fn is_png(path: &Path) -> bool {
    fs::read(path).is_ok_and(|b| b.starts_with(b"\x89PNG"))
}

#[test]
fn writes_each_file_beside_it_and_says_nothing() {
    let dir = folder("beside", &["a.png", "b.png"]);
    let run = slimdeer(&dir, &["a.png", "b.png"]);
    assert_eq!(run.status.code(), Some(0));
    assert!(run.stdout.is_empty() && run.stderr.is_empty(), "{}", String::from_utf8_lossy(&run.stderr));
    for name in ["a.min.png", "b.min.png"] {
        assert!(is_png(&dir.join(name)), "{name}");
        assert!(fs::metadata(dir.join(name)).unwrap().len() < sample().len() as u64);
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn writes_into_a_folder_but_never_over_an_input() {
    let dir = folder("dir", &["a.png", "b.png"]);
    let run = slimdeer(&dir, &["a.png", "b.png", "-d", "out/web"]);
    assert_eq!(run.status.code(), Some(0), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(is_png(&dir.join("out/web/a.png")) && is_png(&dir.join("out/web/b.png")));
    // the inputs' own folder: that name is the input's
    let run = slimdeer(&dir, &["a.png", "-d", "."]);
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&run.stderr).contains("would replace the input"));
    assert_eq!(fs::read(dir.join("a.png")).unwrap(), sample());
    fs::remove_dir_all(&dir).unwrap();
}

#[cfg(feature = "webp")]
#[test]
fn names_one_output_with_o_in_the_format_its_name_says() {
    let dir = folder("output", &["a.png"]);
    let run = slimdeer(&dir, &["a.png", "-o", "small.webp", "-v"]);
    assert_eq!(run.status.code(), Some(0), "{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(&fs::read(dir.join("small.webp")).unwrap()[8..12], b"WEBP");
    // -v: one line, on stderr
    let said = String::from_utf8(run.stderr).unwrap();
    assert!(run.stdout.is_empty() && said.starts_with("a.png \u{2192} small.webp: ") && said.lines().count() == 1, "{said}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn reads_stdin_and_writes_stdout() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_slimdeer")).arg("-").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(&sample()).unwrap();
    let run = child.wait_with_output().unwrap();
    assert_eq!(run.status.code(), Some(0));
    assert!(run.stdout.starts_with(b"\x89PNG") && run.stdout.len() < sample().len());
}

#[test]
fn json_is_a_line_per_file() {
    let dir = folder("json", &["a.png", "b.png"]);
    let run = slimdeer(&dir, &["a.png", "b.png", "--json"]);
    assert_eq!(run.status.code(), Some(0));
    let lines: Vec<String> = String::from_utf8(run.stdout).unwrap().lines().map(String::from).collect();
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].starts_with('{') && lines[0].ends_with(&format!(r#""input": "a.png", "output": "a.min.png", "input_bytes": {}}}"#, sample().len())),
        "{}",
        lines[0]
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_bad_file_doesnt_stop_the_others() {
    let dir = folder("bad", &["good.png"]);
    fs::write(dir.join("notes.txt"), "not an image").unwrap();
    let run = slimdeer(&dir, &["notes.txt", "missing.png", "good.png"]);
    assert_eq!(run.status.code(), Some(1));
    let said = String::from_utf8(run.stderr).unwrap();
    assert!(said.contains("notes.txt: not a PNG or a JPEG") && said.contains("missing.png: no such file"), "{said}");
    assert!(is_png(&dir.join("good.min.png")));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_command_it_cant_run_exits_2() {
    let dir = folder("usage", &["a.png", "b.png"]);
    for args in [&[][..], &["a.png", "b.png", "-o", "x.png"], &["--quality", "80", "a.png"], &["a.png", "--target", "101"], &["-", "--json"]] {
        let run = slimdeer(&dir, args);
        assert_eq!(run.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&run.stderr).contains("slimdeer --help"), "{args:?}");
    }
    assert!(!dir.join("a.min.png").exists() && !dir.join("x.png").exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn recurses_into_folders_with_r() {
    let dir = folder("recursive", &[]);
    for name in ["assets/a.png", "assets/icons/b.PNG", "assets/icons/small/c.png", "assets/.cache/d.png"] {
        fs::create_dir_all(dir.join(name).parent().unwrap()).unwrap();
        fs::write(dir.join(name), sample()).unwrap();
    }
    fs::write(dir.join("assets/icons/notes.txt"), "not an image").unwrap();
    // a folder needs -r
    let run = slimdeer(&dir, &["assets"]);
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&run.stderr).contains("assets: a folder; -r"));
    // beside each image, hidden folders and other files left alone, in the folders' order
    let run = slimdeer(&dir, &["-r", "assets", "--json"]);
    assert_eq!(run.status.code(), Some(0), "{}", String::from_utf8_lossy(&run.stderr));
    let outputs: Vec<String> = String::from_utf8(run.stdout)
        .unwrap()
        .lines()
        .map(|line| line.split(r#""output": ""#).nth(1).unwrap().split('"').next().unwrap().to_string())
        .collect();
    assert_eq!(outputs, ["assets/a.min.png", "assets/icons/b.min.PNG", "assets/icons/small/c.min.png"]);
    assert!(!dir.join("assets/.cache/d.min.png").exists());
    // again: the outputs aren't compressed a second time
    let run = slimdeer(&dir, &["-r", "assets", "--json"]);
    assert_eq!(String::from_utf8(run.stdout).unwrap().lines().count(), 3);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn copies_the_tree_into_d_and_keeps_its_images() {
    let dir = folder("tree", &[]);
    for name in ["assets/a.png", "assets/icons/b.png"] {
        fs::create_dir_all(dir.join(name).parent().unwrap()).unwrap();
        fs::write(dir.join(name), sample()).unwrap();
    }
    let run = slimdeer(&dir, &["-r", "assets", "-d", "dist"]);
    assert_eq!(run.status.code(), Some(0), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(is_png(&dir.join("dist/a.png")) && is_png(&dir.join("dist/icons/b.png")));
    // -d can't be the folder -r reads, whose images it would replace
    let run = slimdeer(&dir, &["-r", "assets", "-d", "assets"]);
    assert_eq!(run.status.code(), Some(2));
    assert_eq!(fs::read(dir.join("assets/icons/b.png")).unwrap(), sample());
    // nor does -r go with -o's one output
    assert_eq!(slimdeer(&dir, &["-r", "assets", "-o", "x.png"]).status.code(), Some(2));
    fs::remove_dir_all(&dir).unwrap();
}
