use std::path::PathBuf;

pub fn root() -> PathBuf {
    std::env::var_os("ASHEN_ASSET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets"))
}
