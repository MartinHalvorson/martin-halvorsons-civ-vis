fn main() {
    // `/status` embeds the exact revision promoted by the spectator
    // supervisor. Make Cargo notice when only that identity changes, even if
    // the compiled source bytes are otherwise unchanged.
    println!("cargo:rerun-if-env-changed=CIVVIS_COMMIT");
}
