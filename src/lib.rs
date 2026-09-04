#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(not(target_os = "linux"))]
compile_error!("rmu currently supports Linux only");
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("rmu currently supports only x86_64 and AArch64 Linux");

mod args;
mod control;
mod ctl_args;
mod directory_sizes;
mod display;
mod engine;
mod freedesktop;
mod linux;
mod mode;
mod rmu_cli;
mod rmuctl_cli;
mod scanner;
#[cfg(test)]
mod test_support;

/// Run the `rmu` command-line interface.
#[must_use]
pub fn run_rmu() -> std::process::ExitCode {
    rmu_cli::entrypoint()
}

/// Run the `rmuctl` command-line interface.
#[must_use]
pub fn run_rmuctl() -> std::process::ExitCode {
    rmuctl_cli::entrypoint()
}
