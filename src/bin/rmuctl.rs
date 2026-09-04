use std::process::ExitCode;

include!(concat!(env!("OUT_DIR"), "/rmu_build_identity.rs"));

fn main() -> ExitCode {
    rmu::run_rmuctl()
}
