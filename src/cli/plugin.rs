use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
};

use clap::Parser;
use memofs::{InMemoryFs, Vfs, VfsSnapshot};
use roblox_install::RobloxStudio;

use crate::serve_session::ServeSession;

static PLUGIN_BINCODE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/plugin.bincode"));
static PLUGIN_FILE_NAME: &str = "RojoManagedPlugin.rbxm";

/// Install Rojo's plugin.
#[derive(Debug, Parser)]
pub struct PluginCommand {
    #[clap(subcommand)]
    subcommand: PluginSubcommand,
}

/// Manages Rojo's Roblox Studio plugin.
#[derive(Debug, Parser)]
pub enum PluginSubcommand {
    /// Install the plugin in Roblox Studio's plugins folder. If the plugin is
    /// already installed, installing it again will overwrite the current plugin
    /// file.
    Install,

    /// Removes the plugin if it is installed.
    Uninstall,
}

impl PluginCommand {
    pub fn run(self) -> anyhow::Result<()> {
        self.subcommand.run()
    }
}

impl PluginSubcommand {
    pub fn run(self) -> anyhow::Result<()> {
        match self {
            PluginSubcommand::Install => install_plugin(),
            PluginSubcommand::Uninstall => uninstall_plugin(),
        }
    }
}

fn initialize_plugin() -> anyhow::Result<ServeSession> {
    let plugin_snapshot: VfsSnapshot = bincode::deserialize(PLUGIN_BINCODE)
        .expect("Rojo's plugin was not properly packed into Rojo's binary");

    let mut in_memory_fs = InMemoryFs::new();
    in_memory_fs.load_snapshot("/plugin", plugin_snapshot)?;

    let vfs = Vfs::new(in_memory_fs);
    Ok(ServeSession::new(vfs, "/plugin")?)
}

fn install_plugin() -> anyhow::Result<()> {
    let studio = RobloxStudio::locate()?;

    let plugins_folder_path = studio.plugins_path();

    if !plugins_folder_path.exists() {
        log::debug!("Creating Roblox Studio plugins folder");
        // `create_dir_all` rather than `create_dir`, so a missing parent (a
        // machine where Studio has never written to Documents) is created
        // rather than reported as a confusing failure.
        fs::create_dir_all(plugins_folder_path)?;
    }

    let plugin_path = plugins_folder_path.join(PLUGIN_FILE_NAME);
    log::debug!("Writing plugin to {}", plugin_path.display());

    write_plugin(&plugin_path)
}

/// Writes the packed plugin to `plugin_path`, replacing it atomically.
///
/// The obvious implementation truncates the destination and then does the work
/// that can fail, which means a bad pack or a full disk leaves the developer
/// with a zero-byte plugin and no working install — strictly worse than the
/// version they started with. Building the tree first and swapping the file in
/// at the end means the old plugin survives every failure.
fn write_plugin(plugin_path: &Path) -> anyhow::Result<()> {
    let session = initialize_plugin()?;
    let tree = session.tree();
    let root_id = tree.get_root_id();

    // Written beside the destination rather than in a temporary directory, so
    // the rename below stays on one filesystem and is therefore atomic.
    let staging_path = plugin_path.with_extension("rbxm.new");

    {
        let mut file = BufWriter::new(File::create(&staging_path)?);
        rbx_binary::to_writer(&mut file, tree.inner(), &[root_id])?;

        // `BufWriter`'s `Drop` flushes but discards the error, so a failure on
        // the final write would otherwise be reported as success.
        file.flush()?;
    }

    match fs::rename(&staging_path, plugin_path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&staging_path);
            Err(err.into())
        }
    }
}

fn uninstall_plugin() -> anyhow::Result<()> {
    let studio = RobloxStudio::locate()?;

    let plugin_path = studio.plugins_path().join(PLUGIN_FILE_NAME);

    if plugin_path.exists() {
        log::debug!("Removing existing plugin from {}", plugin_path.display());
        fs::remove_file(plugin_path)?;
    } else {
        log::debug!("Plugin not installed at {}", plugin_path.display());
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn plugin_initialize() {
        let _ = initialize_plugin().unwrap();
    }

    #[test]
    fn writing_the_plugin_produces_a_complete_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PLUGIN_FILE_NAME);

        write_plugin(&path).unwrap();

        let written = fs::read(&path).unwrap();

        assert!(
            written.starts_with(b"<roblox!"),
            "the written plugin is not a Roblox binary model"
        );

        // rbx_binary writes this terminator uncompressed for exactly this
        // purpose, so it is the cheapest proof that the write ran to
        // completion rather than being cut short.
        assert!(
            written.ends_with(b"</roblox>"),
            "the written plugin is truncated"
        );

        // No staging file should survive a successful install.
        assert!(!path.with_extension("rbxm.new").exists());
    }

    #[test]
    fn a_failed_write_leaves_the_existing_plugin_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PLUGIN_FILE_NAME);

        fs::write(&path, b"previously installed plugin").unwrap();

        // A directory where the staging file needs to go makes `File::create`
        // fail after the point where the old implementation had already
        // truncated the destination.
        fs::create_dir(path.with_extension("rbxm.new")).unwrap();

        assert!(write_plugin(&path).is_err());

        assert_eq!(
            fs::read(&path).unwrap(),
            b"previously installed plugin",
            "a failed install must not damage the plugin already in place"
        );
    }
}
