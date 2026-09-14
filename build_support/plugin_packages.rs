use std::path::Path;

pub const REQUIRED_PLUGIN_PACKAGES: &[&str] = &[
    "Flipper",
    "Highlighter",
    "Promise",
    "Roact",
    "TestEZ",
    "msgpack-luau",
    "t",
];

pub fn validate(plugin_dir: &Path) -> Result<(), String> {
    let packages_dir = plugin_dir.join("Packages");
    let missing = REQUIRED_PLUGIN_PACKAGES
        .iter()
        .filter(|package| {
            !packages_dir
                .join(package)
                .join("default.project.json")
                .is_file()
        })
        .copied()
        .collect::<Vec<_>>();

    if missing.is_empty() {
        return Ok(());
    }

    Err(format!(
        "plugin dependency submodules are not initialized: {}. Run `git submodule update --init --recursive` from the repository root and rebuild",
        missing.join(", ")
    ))
}
