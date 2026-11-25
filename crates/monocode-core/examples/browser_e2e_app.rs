//! App side of `npm run ext:e2e`: see `monocode_core::mono::browser::e2e`. Debug builds only.
#[cfg(all(unix, debug_assertions))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(monocode_core::mono::browser::e2e::main(&args));
}

#[cfg(not(all(unix, debug_assertions)))]
fn main() {
    eprintln!("browser_e2e_app only exists in unix debug builds");
    std::process::exit(2);
}
