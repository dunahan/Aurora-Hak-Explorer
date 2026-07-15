fn main() {
    println!("cargo:rerun-if-changed=assets/aheicon.ico");
    println!("cargo:rerun-if-changed=packaging/aurora-hak-explorer.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("packaging/aurora-hak-explorer.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
