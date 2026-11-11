use std::path::Path;

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root_manifest = manifest_dir.join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", root_manifest.display());
    let version = workspace_package_version(&root_manifest);
    println!("cargo:rustc-env=V2RAYR_VERSION={version}");
    tauri_build::build()
}

/// The shell is its own Cargo workspace, so its package version cannot inherit
/// the product version. The root manifest is the only source.
fn workspace_package_version(path: &Path) -> String {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", path.display()));
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.package]";
            continue;
        }
        if !in_section {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("version") else {
            continue;
        };
        let value = rest
            .trim()
            .trim_start_matches('=')
            .trim()
            .trim_matches('"');
        if value.is_empty()
            || !value
                .split(['.', '-', '+'])
                .next()
                .is_some_and(|major| major.chars().all(|c| c.is_ascii_digit()))
        {
            panic!("{} 的 workspace.package.version 无效：{value}", path.display());
        }
        return value.to_owned();
    }
    panic!("{} 没有 [workspace.package] version", path.display());
}
