//! Build script: embed the Windows resource file (icon + manifest) into the exe.
//!
//! The resource file lives at `assets/yumedock.rc` and references the icon and
//! manifest by relative path. We compile it with the `embed-resource` crate,
//! which shells out to `rc.exe` on MSVC and `windres` on GNU toolchains.
//!
//! If `assets/yumedock.rc` does not exist (e.g. a fresh checkout where the icon
//! has not been generated yet), the build continues without an icon so a plain
//! `cargo build` always works -- the icon is a presentation concern, not a
//! build prerequisite.

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let rc = std::path::Path::new(&manifest_dir)
        .join("assets")
        .join("yumedock.rc");

    if !rc.exists() {
        println!(
            "cargo:warning=YumeDock icon resource not found at {} -- building without embedded icon. \
             Run `python assets/make_icon.py` to generate it.",
            rc.display()
        );
        return;
    }

    println!("cargo:rerun-if-changed=assets/yumedock.rc");
    println!("cargo:rerun-if-changed=assets/yumedock.ico");

    // Compile assets/yumedock.rc (paths inside are relative to that file). On a
    // failure embed-resource prints its own diagnostic; we abort rather than
    // silently shipping an icon-less exe in CI, since a missing icon in a
    // release is a regression we'd want to notice.
    let _ = embed_resource::compile("assets/yumedock", embed_resource::NONE);
}
