//! mint-rabbit: a 3D model and vector texture painter.

mod app;
mod camera;
mod render;
mod scene;
mod texture_view;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: mint-rabbit [--decomp PATH] [--game-version GAFE01_00]";

/// Parsed command line: options to run with, or `None` after `--help`.
fn parse_args() -> Result<Option<app::Options>, String> {
    let mut options = app::Options {
        decomp: None,
        game_version: "GAFE01_00".into(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--decomp" => options.decomp = Some(PathBuf::from(value()?)),
            "--game-version" => options.game_version = value()?,
            "-h" | "--help" => return Ok(None),
            _ => return Err(format!("unknown argument {arg:?}\n{USAGE}")),
        }
    }
    Ok(Some(options))
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(Some(o)) => o,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let native = eframe::NativeOptions {
        depth_buffer: render::DEPTH_BITS,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("mint-rabbit")
            .with_inner_size([1400.0, 820.0]),
        ..eframe::NativeOptions::default()
    };
    match eframe::run_native(
        "mint-rabbit",
        native,
        Box::new(|cc| Ok(Box::new(app::App::new(cc, options)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mint-rabbit: {e}");
            ExitCode::FAILURE
        }
    }
}
