//! The command line's parts; main.rs says what it does.

use std::{sync::OnceLock, time::Instant};

pub mod args;
mod encode;
mod files;
mod report;
mod run;
mod tools;

pub type Error = Box<dyn std::error::Error>;

/// Says what's wrong with a command it can't run; the exit code.
pub fn usage_error(message: &str) -> i32 {
    eprintln!("slimdeer: {message}\nTry 'slimdeer --help' for more.");
    2
}

pub fn now_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}
