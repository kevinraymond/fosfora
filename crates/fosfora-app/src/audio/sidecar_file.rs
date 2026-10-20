//! The one way a dev sidecar (`FOSFORA_*_SIDECAR`) opens its output file.
//!
//! A sidecar is evidence: an hour of a listener's live ticks can't be re-recorded. Opening
//! the path with `File::create` truncated whatever was there, and a launch that reused an
//! old env line wiped a session that way (#4174). So a sidecar never overwrites data: a
//! path that already holds bytes is refused loudly, and the caller picks a new one or
//! deletes the old file on purpose. An existing *empty* file is taken over, since the
//! dumpers pre-create nothing and a `touch`ed placeholder loses nothing.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;

/// Open `path` for writing without ever truncating existing data. Panics, naming `var`,
/// when the file already holds bytes or cannot be created: a sweep silently missing a
/// track, or a session silently overwriting one, would corrupt the comparison.
pub fn create_sidecar(var: &str, path: &str) -> File {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            assert!(
                len == 0,
                "{var}: {path} already holds {len} bytes; a sidecar is never overwritten. \
                 Point the variable at a new file, or delete this one first."
            );
            File::create(path).unwrap_or_else(|e| panic!("{var}: cannot open {path}: {e}"))
        }
        Err(e) => panic!("{var}: cannot create {path}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sidecar_file_{tag}_{}.jsonl", std::process::id()))
    }

    #[test]
    fn a_fresh_path_is_created() {
        let p = temp("fresh");
        let _ = std::fs::remove_file(&p);
        let mut f = create_sidecar("T", p.to_str().unwrap());
        f.write_all(b"x\n").unwrap();
        drop(f);
        assert_eq!(std::fs::read(&p).unwrap(), b"x\n");
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn an_empty_existing_file_is_taken_over() {
        let p = temp("empty");
        File::create(&p).unwrap();
        let mut f = create_sidecar("T", p.to_str().unwrap());
        f.write_all(b"y\n").unwrap();
        drop(f);
        assert_eq!(std::fs::read(&p).unwrap(), b"y\n");
        std::fs::remove_file(&p).unwrap();
    }

    /// The guard itself: a file with data in it is refused and left untouched.
    #[test]
    fn a_file_with_data_is_never_truncated() {
        let p = temp("data");
        std::fs::write(&p, b"an hour of ticks\n").unwrap();
        let path = p.to_str().unwrap().to_owned();
        let r = std::panic::catch_unwind(|| create_sidecar("FOSFORA_T_SIDECAR", &path));
        let msg = match r {
            Ok(_) => panic!("a non-empty sidecar was opened for writing"),
            Err(e) => e.downcast_ref::<String>().cloned().unwrap_or_else(|| {
                e.downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .unwrap_or_default()
            }),
        };
        assert!(
            msg.contains("FOSFORA_T_SIDECAR") && msg.contains("never overwritten"),
            "{msg}"
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"an hour of ticks\n");
        std::fs::remove_file(&p).unwrap();
    }
}
