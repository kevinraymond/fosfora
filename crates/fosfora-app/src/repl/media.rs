//! Media natives: browse a folder of clips and open one as a media layer.
//!
//! `(list-media "dir")` returns the playable files in `dir` as EDN text, and
//! `(add-media! "path")` queues [`WsInMessage::AddMedia`], which the render
//! thread hands to `App::start_media_layer`: the same background decode the
//! "add media layer" file dialog uses.

use std::path::{Path, PathBuf};

use cljrs_runtime::tiered::GlobalEnv;
use cljrs_value::{Arity, ValueError};

use super::{def_native, push_edn_str, str_arg, str_value};
use crate::inbound::DropOldestSender;
use crate::web::types::WsInMessage;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp"];

/// `~/x` and `~` against `$HOME`; anything else as given.
fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix('~'), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(path),
    }
}

fn is_media(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| {
            crate::media::decoder::VIDEO_EXTENSIONS.contains(&e.as_str())
                || IMAGE_EXTENSIONS.contains(&e.as_str())
        })
}

fn other(msg: String) -> ValueError {
    ValueError::Other(msg)
}

pub(super) fn register(globals: &GlobalEnv, commands: DropOldestSender<WsInMessage>) {
    def_native(globals, "list-media", Arity::Fixed(1), |args| {
        let dir = expand(&str_arg(args, 0)?);
        let entries =
            std::fs::read_dir(&dir).map_err(|e| other(format!("{}: {e}", dir.display())))?;
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_media(p))
            .collect();
        files.sort();
        let mut edn = String::from("[");
        for (i, f) in files.iter().enumerate() {
            if i > 0 {
                edn.push(' ');
            }
            push_edn_str(&f.display().to_string(), &mut edn);
        }
        edn.push(']');
        Ok(str_value(edn))
    });

    def_native(globals, "add-media!", Arity::Fixed(1), move |args| {
        let path = expand(&str_arg(args, 0)?);
        let path = path
            .canonicalize()
            .map_err(|e| other(format!("{}: {e}", path.display())))?;
        if !path.is_file() || !is_media(&path) {
            return Err(other(format!(
                "not an image or video fosfora can open: {}",
                path.display()
            )));
        }
        let shown = path.display().to_string();
        commands.send(WsInMessage::AddMedia { path });
        Ok(str_value(shown))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_is_recognised_by_extension_case_insensitively() {
        assert!(is_media(Path::new("/x/loop.MP4")));
        assert!(is_media(Path::new("/x/still.webp")));
        assert!(!is_media(Path::new("/x/notes.txt")));
        assert!(!is_media(Path::new("/x/noext")));
    }

    #[test]
    fn tilde_expands_against_home_only_at_the_start() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand("~/clips"), PathBuf::from(&home).join("clips"));
        assert_eq!(expand("~"), PathBuf::from(&home));
        assert_eq!(expand("/a/~b"), PathBuf::from("/a/~b"));
        assert_eq!(expand("~other/x"), PathBuf::from("~other/x"));
    }
}
