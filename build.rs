fn main() {
    hbb_common::gen_version();
    // Preview builds carry the full version (e.g. "45.1") so the
    // updater can tell an installed preview from its formal base
    // (PRO_VERSION stays the formal number). Set PRO_FULL_VERSION
    // when building a preview release; formal builds leave it unset
    // and api.rs falls back to PRO_VERSION.
    println!("cargo:rerun-if-env-changed=PRO_FULL_VERSION");
    if let Ok(v) = std::env::var("PRO_FULL_VERSION") {
        let v = v.trim();
        if !v.is_empty() {
            println!("cargo:rustc-env=PRO_FULL_VERSION={v}");
        }
    }
}
