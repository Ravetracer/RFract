// SPDX-License-Identifier: LGPL-3.0-or-later

//! rfr-cli: RFract on the command line. Stage 0: version and system information; rendering,
//! animation, batch and formula commands follow with the stages in PLAN.md.

use std::process::ExitCode;

const USAGE: &str = "\
Usage: rfr-cli <command>

Commands:
  info         Program version, GPU, denoiser and settings folders
  help         This text

Options:
  -V, --version  Print the version";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Version,
    Info,
    Help,
}

fn parse(args: &[String]) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => Ok(Command::Help),
        Some("-V" | "--version" | "version") => Ok(Command::Version),
        Some("info") => Ok(Command::Info),
        Some(other) => Err(format!("unknown command '{other}'")),
    }
}

fn info() {
    println!("{}", rfr_core::name_version());
    match rfr_gpu::detect_adapter() {
        Ok(a) => println!("GPU:       {a}"),
        Err(e) => println!("GPU:       none ({e}); rendering will use the CPU"),
    }
    match rfr_core::render::oidn::library() {
        Ok(p) => println!("Denoiser:  {}", p.display()),
        Err(e) => println!("Denoiser:  not available ({e})"),
    }
    println!("Settings:  {}", rfr_core::paths::config_dir().display());
    println!("Caches:    {}", rfr_core::paths::cache_dir().display());
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(Command::Version) => println!("{}", rfr_core::name_version()),
        Ok(Command::Info) => info(),
        Ok(Command::Help) => println!("{}\n\n{USAGE}", rfr_core::name_version()),
        Err(e) => {
            eprintln!("rfr-cli: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse(&args(&[])), Ok(Command::Help));
        assert_eq!(parse(&args(&["--version"])), Ok(Command::Version));
        assert_eq!(parse(&args(&["-V"])), Ok(Command::Version));
        assert_eq!(parse(&args(&["info"])), Ok(Command::Info));
        assert!(parse(&args(&["render"])).is_err());
    }
}
