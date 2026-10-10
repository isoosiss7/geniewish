use anyhow::Result;
use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

pub struct Log {
    path: PathBuf,
    file: File,
    bytes: u64,
    limit: u64,
    transcripts: bool,
}

fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    Ok(options.open(path)?)
}

impl Log {
    pub fn open(directory: &Path, transcripts: bool) -> Result<Self> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(directory)?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let path = directory.join(format!("core-{stamp}-{}.jsonl", std::process::id()));
        let file = private_file(&path)?;
        let mut old: Vec<_> = fs::read_dir(directory)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("core-") && n.ends_with(".jsonl"))
            })
            .collect();
        old.sort();
        for old in old.iter().take(old.len().saturating_sub(10)) {
            for suffix in ["", ".1", ".2"] {
                let _ = fs::remove_file(format!("{}{suffix}", old.display()));
            }
        }
        Ok(Self {
            path,
            file,
            bytes: 0,
            limit: 1_048_576,
            transcripts,
        })
    }

    pub fn write(&mut self, event: &str, fields: Value) -> Result<()> {
        let mut record = serde_json::to_vec(
            &json!({"time_ms":SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),"pid":std::process::id(),"component":"core","event":event,"fields":fields}),
        )?;
        record.push(b'\n');
        if self.bytes > 0 && self.bytes + record.len() as u64 > self.limit {
            let first = format!("{}.1", self.path.display());
            let second = format!("{}.2", self.path.display());
            if Path::new(&second).exists() {
                fs::remove_file(&second)?;
            }
            if Path::new(&first).exists() {
                fs::rename(&first, &second)?;
            }
            fs::rename(&self.path, &first)?;
            self.file = private_file(&self.path)?;
            self.bytes = 0;
        }
        self.file.write_all(&record)?;
        self.file.flush()?;
        self.bytes += record.len() as u64;
        Ok(())
    }

    pub fn text(&mut self, event: &str, text: &str) -> Result<()> {
        let mut fields = json!({"chars":text.chars().count(),"transcript_logged":self.transcripts});
        if self.transcripts {
            fields["text"] = text.chars().take(4096).collect::<String>().into();
        }
        self.write(event, fields)
    }
}

pub fn init(directory: &Path, transcripts: bool) -> Result<()> {
    let log = Log::open(directory, transcripts)?;
    let _ = LOG.set(Mutex::new(log));
    Ok(())
}

pub fn record(event: &str, fields: Value) {
    if let Some(log) = LOG.get()
        && let Ok(mut log) = log.lock()
        && let Err(error) = log.write(event, fields)
    {
        eprintln!("geniewish diagnostics could not be written: {error}");
    }
}

pub fn text(event: &str, value: &str) {
    if let Some(log) = LOG.get()
        && let Ok(mut log) = log.lock()
        && let Err(error) = log.text(event, value)
    {
        eprintln!("geniewish diagnostics could not be written: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logs_are_private_rotated_and_transcripts_are_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = Log::open(dir.path(), false).unwrap();
        log.text("input", "private-phrase").unwrap();
        assert!(
            !fs::read_to_string(&log.path)
                .unwrap()
                .contains("private-phrase")
        );
        log.transcripts = true;
        log.text("input", "visible-debug-phrase").unwrap();
        assert!(
            fs::read_to_string(&log.path)
                .unwrap()
                .contains("visible-debug-phrase")
        );
        log.limit = 1;
        for _ in 0..4 {
            log.write("rotation", json!({"ok":true})).unwrap();
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
        for file in fs::read_dir(dir.path()).unwrap() {
            let file = file.unwrap();
            for line in fs::read_to_string(file.path()).unwrap().lines() {
                let _: Value = serde_json::from_str(line).unwrap();
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
            }
        }
    }
}
