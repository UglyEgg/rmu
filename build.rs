use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=RMU_SOURCE_MANIFEST_SHA256");
    println!("cargo:rerun-if-env-changed=RMU_RUNTIME_MANIFEST_SHA256");
    let source = env::var("RMU_SOURCE_MANIFEST_SHA256").unwrap_or_else(|_| "unbound".into());
    let runtime = env::var("RMU_RUNTIME_MANIFEST_SHA256").unwrap_or_else(|_| "unbound".into());
    validate_digest("RMU_SOURCE_MANIFEST_SHA256", &source);
    validate_digest("RMU_RUNTIME_MANIFEST_SHA256", &runtime);
    let architecture = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo target architecture");
    let identity = format!(
        "RMU-BUILD-IDENTITY-V2\nsource-manifest-sha256={source}\nruntime-manifest-sha256={runtime}\narchitecture={architecture}\n"
    );
    let generated = format!(
        "#[used]\n#[unsafe(link_section = \".rmu.build\")]\nstatic RMU_BUILD_IDENTITY: [u8; {}] = *b{:?};\n",
        identity.len(),
        identity
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    fs::write(output.join("rmu_build_identity.rs"), generated)
        .expect("write embedded RMU build identity");
}

fn validate_digest(name: &str, value: &str) {
    if value != "unbound"
        && (value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    {
        panic!("{name} must be 64 hexadecimal characters");
    }
}
