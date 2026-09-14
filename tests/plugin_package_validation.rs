use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[path = "../build_support/plugin_packages.rs"]
mod plugin_packages;

fn temporary_plugin_dir(test_name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "roxo-plugin-package-validation-{}-{test_name}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn accepts_initialized_plugin_packages() {
    let plugin_dir = temporary_plugin_dir("complete");

    for package in plugin_packages::REQUIRED_PLUGIN_PACKAGES {
        let package_dir = plugin_dir.join("Packages").join(package);
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(package_dir.join("default.project.json"), "{}").unwrap();
    }

    assert_eq!(plugin_packages::validate(&plugin_dir), Ok(()));
    fs::remove_dir_all(plugin_dir).unwrap();
}

#[test]
fn reports_every_missing_plugin_package_with_recovery_command() {
    let plugin_dir = temporary_plugin_dir("missing");
    let roact_dir = plugin_dir.join("Packages").join("Roact");
    fs::create_dir_all(&roact_dir).unwrap();
    fs::write(roact_dir.join("default.project.json"), "{}").unwrap();

    let error = plugin_packages::validate(&plugin_dir).unwrap_err();

    assert!(!error.contains("Roact"));
    for package in plugin_packages::REQUIRED_PLUGIN_PACKAGES {
        if *package != "Roact" {
            assert!(error.contains(package));
        }
    }
    assert!(error.contains("git submodule update --init --recursive"));

    fs::remove_dir_all(plugin_dir).unwrap();
}
