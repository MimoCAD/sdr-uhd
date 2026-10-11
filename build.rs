// Generates Rust bindings for libuhd's C API (Application Programming
// Interface) at build time and emits linker directives for the prefix the
// UHD (USRP Hardware Driver; USRP = Universal Software Radio Peripheral)
// stack was installed into. The prefix comes from $SDRPREFIX (the same
// variable README.md's build steps use), falling back to ~/.sdr/prefix.
fn main() {
    // the README's default (Step 0, Option A)
    let default = format!("{}/.sdr/prefix", std::env::var("HOME").unwrap_or_default());
    let prefix = std::env::var("SDRPREFIX").unwrap_or(default);
    // rebuild if the choice changes
    println!("cargo:rerun-if-env-changed=SDRPREFIX");
    // ...or if libuhd itself was rebuilt with a new header
    let header = format!("{prefix}/include/uhd.h");
    println!("cargo:rerun-if-changed={header}");
    // Checked before anything else is emitted: bindgen's own failure on a
    // missing header is `NotExist("<path>")` — true, and useless.
    if !std::path::Path::new(&header).exists() {
        panic!(
            "\n\
             UHD not found: {header} does not exist.\n\
             \n\
             sdr-uhd links libuhd (UHD's C API). Build it from source into a\n\
             prefix (README.md, Steps 0-8; the B206mini needs UHD >= 4.9),\n\
             or name an existing install:\n\
             \x20   SDRPREFIX=/usr/local cargo build --release\n"
        );
    }
    // where libuhd lives
    println!("cargo:rustc-link-search=native={prefix}/lib");
    // link -luhd
    println!("cargo:rustc-link-lib=dylib=uhd");
    // Linux: macOS records a dylib's install name by absolute path; ld.so
    // has no such thing. -rpath records the prefix in the binary so it runs
    // from any shell with no LD_LIBRARY_PATH, and -rpath-link lets the
    // LINKER resolve libuhd's own Boost libraries beside it (without it:
    // "libboost_thread.so…, needed by libuhd.so, not found" and undefined
    // boost::thread references). --disable-new-dtags makes it a DT_RPATH,
    // not a DT_RUNPATH: only the former is consulted for libuhd's OWN
    // dependencies (its Boost libraries in the same prefix); with a RUNPATH
    // they read "not found" at run time even though libuhd itself resolves.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath-link,{prefix}/lib");
    }
    let bindings = bindgen::Builder::default()
        // the whole C API in one header
        .header(&header)
        // so nested includes resolve
        .clang_arg(format!("-I{prefix}/include"))
        // only UHD's own API, not everything its includes drag in
        .allowlist_item("uhd_.*")
        .generate()
        .unwrap_or_else(|e| panic!("bindgen failed on {header}: {e}"));
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings.write_to_file(out.join("uhd.rs")).unwrap();
}
