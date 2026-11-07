use std::env;
use std::process::ExitCode;

use rustic_engine::Viewport;

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.is_empty() || (args.len() == 1 && args[0] == "--help") {
        println!(
            "Rustic browser engine development scaffold\n\nUsage: rustic [info|--help|--version]"
        );
        return ExitCode::SUCCESS;
    }
    if args.len() == 1 && args[0] == "--version" {
        println!("rustic {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.len() == 1 && args[0] == "info" {
        let viewport = Viewport::new(1280, 720).expect("built-in viewport is within limits");
        println!("Rustic {} — foundation stage", env!("CARGO_PKG_VERSION"));
        println!(
            "Default viewport: {}x{}",
            viewport.width(),
            viewport.height()
        );
        println!("HTML, CSS, JavaScript, networking, and rendering: not implemented");
        return ExitCode::SUCCESS;
    }
    eprintln!(
        "Unsupported command. Rendering and navigation are not implemented; see docs/ROADMAP.md."
    );
    ExitCode::from(2)
}
