use crate::state::State;
use anyhow::{Context, Result};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub struct Store {
    directory: PathBuf,
    _lock: File,
}

fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    Ok(options.open(path)?)
}

impl Store {
    pub fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let lock = options.open(directory.join("lock"))?;
        lock.try_lock()
            .context("Another geniewish process is using this state directory.")?;
        Ok(Self {
            directory: directory.into(),
            _lock: lock,
        })
    }

    pub fn load(&self) -> Result<State> {
        match fs::read(self.directory.join("state.json")) {
            Ok(bytes) => {
                let state: State = serde_json::from_slice(&bytes)
                    .context("Saved state is damaged; refusing to reset lifetime conversations.")?;
                state.validate()?;
                Ok(state)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self, state: &State) -> Result<()> {
        state.validate()?;
        let temporary = self
            .directory
            .join(format!("state-{}.tmp", std::process::id()));
        // The directory lock makes a stale temporary file safe to remove. It is never used as a replacement for a valid saved state.
        if temporary.exists() {
            fs::remove_file(&temporary)?;
        }
        let result = (|| -> Result<()> {
            let mut file = private_file(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(state)?)?;
            file.sync_all()?;
            fs::rename(&temporary, self.directory.join("state.json"))?;
            File::open(&self.directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}
