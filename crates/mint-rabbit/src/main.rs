//! mint-rabbit: a 3D model and vector texture painter.

mod app;
mod camera;
mod editor;
mod icons;
mod model_view;
mod panels;
mod picker;
mod previews;
mod render;
mod scene;
mod stamps;
mod surface;
mod texture_view;
mod watch;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: mint-rabbit [--decomp PATH] [--game-version GAFE01_00] [--project DIR]

--decomp defaults to the MINT_AC_DECOMP environment variable.";

/// Parsed command line: options to run with, or `None` after `--help`.
fn parse_args() -> Result<Option<app::Options>, String> {
    let mut options = app::Options {
        decomp: std::env::var_os("MINT_AC_DECOMP")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
        game_version: "GAFE01_00".into(),
        project: None,
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
            "--project" => options.project = Some(PathBuf::from(value()?)),
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
