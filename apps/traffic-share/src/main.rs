//! `traffic-share [--base /blog/] FILE...`: prints the route-share table for `CloudFront` logs.

use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = io::stdout().lock();
    match traffic_share::run(&args, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Nothing else can report the failure; a broken stderr leaves only the exit code.
            let _ = writeln!(io::stderr(), "traffic-share: {error}");
            ExitCode::FAILURE
        }
    }
}
