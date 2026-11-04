//! What slimdeer says about a file: -v's line, --json's report, and errors as people say them.

use std::io;

/// -v's line: what `input_bytes` came to, and how long it took.
pub fn verbose_line(from: &str, to: &str, input_bytes: usize, output_bytes: usize, unchanged: bool, seconds: f64) -> String {
    let what = if unchanged {
        format!("{}, unchanged: nothing smaller keeps its quality", size(input_bytes))
    } else {
        let change = (output_bytes as f64 / input_bytes as f64 - 1.0) * 100.0;
        let sign = if change < 0.0 { "\u{2212}" } else { "+" };
        format!("{} \u{2192} {} ({sign}{:.0}%)", size(input_bytes), size(output_bytes), change.abs())
    };
    format!("{from} \u{2192} {to}: {what}, {seconds:.1} s")
}

/// --json's line: the library's report with the input, the output and the input's size added.
pub fn json_line(report: &str, input: &str, output: &str, input_bytes: usize) -> String {
    format!(r#"{}, "input": {}, "output": {}, "input_bytes": {}}}"#, &report[..report.len() - 1], json_string(input), json_string(output), input_bytes)
}

/// An I/O error as people say it, without the OS's error number.
pub fn plain(e: io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "no such file".into(),
        io::ErrorKind::PermissionDenied => "permission denied".into(),
        io::ErrorKind::IsADirectory => "a folder, not a file".into(),
        _ => e.to_string(),
    }
}

/// A size in bytes for people: B, KB or MB, in thousands as file managers count.
fn size(bytes: usize) -> String {
    match bytes {
        0..1000 => format!("{bytes} B"),
        1000..1_000_000 => format!("{:.1} KB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
