#![forbid(unsafe_code)]

use anyhow::{Result, bail};
use brauser_host::{
    adapters,
    brand::{APP_NAME, NAMESPACE},
    config::ConfigStore,
    dialog, native,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{NAMESPACE}: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("serve") => serve(),
        Some("--version") => {
            // Test runners check this marker before relying on scripted dialogs.
            let variant = if cfg!(feature = "scripted-dialogs") {
                " (scripted dialogs)"
            } else {
                ""
            };
            println!("{NAMESPACE} {}{variant}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--config-path") => {
            println!("{}", ConfigStore::default_path()?.display());
            Ok(())
        }
        Some("reindex") => reindex(),
        // Chrome supplies its extension origin as a command-line argument to
        // native messaging hosts. The installed manifest, rather than argv,
        // authorizes the stable extension ID.
        // Developer and test tooling: evaluates adapter templates without
        // reading the config (adapter-templates.md §12).
        Some(adapters::COMMAND) => adapters::run(args),
        Some(command @ (dialog::PICK_FOLDER_COMMAND | dialog::CONFIRM_COMMAND)) => {
            dialog::run_child_command(command)
        }
        Some(origin) if origin.starts_with("chrome-extension://") => serve(),
        Some(other) => bail!("unknown command or browser origin: {other}"),
    }
}

fn reindex() -> Result<()> {
    let mut config = ConfigStore::load()?;
    match native::reindex(&mut config)? {
        Some(report) => println!(
            "Rebuilt index.db; wrote {}, kept {}, and left {} conflicting artifact file(s).",
            report.written, report.unchanged, report.conflicts
        ),
        None => println!(
            "Rebuilt index.db. Artifact files are written once you save {APP_NAME} settings to confirm their folder."
        ),
    }
    Ok(())
}

fn serve() -> Result<()> {
    let config = ConfigStore::load()?;
    // A host killed mid-save can leave a temporary behind; nothing else
    // removes it, so clear old ones before serving.
    config.sweep_stale_temporaries();
    native::serve(config)
}
