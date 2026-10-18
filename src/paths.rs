//! Home and config directories, `~` expansion (doc/0002).
//!
//! Nothing here reads the process environment except [`Dirs::from_env`], so
//! tests can build a [`Dirs`] pointing at a temporary directory.

use std::path::{Path, PathBuf};

/// The directories tower works with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// The user's home directory, used for `~` expansion.
    pub home: PathBuf,
    /// Tower's own directory, holding `config.toml` and `state.json`.
    pub config_dir: PathBuf,
}

impl Dirs {
    /// Builds the directories from a home directory and an optional
    /// `$XDG_CONFIG_HOME`. Per the XDG spec an empty or relative
    /// `XDG_CONFIG_HOME` is ignored; the default is `~/.config`.
    pub fn new(home: impl Into<PathBuf>, xdg_config_home: Option<&Path>) -> Dirs {
        let home = home.into();
        let base = match xdg_config_home {
            Some(dir) if dir.is_absolute() => dir.to_path_buf(),
            _ => home.join(".config"),
        };
        Dirs {
            home,
            config_dir: base.join("watchtower"),
        }
    }

    /// Reads `$HOME` and `$XDG_CONFIG_HOME` from the process environment.
    /// Returns `None` when no home directory can be determined.
    pub fn from_env() -> Option<Dirs> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .or_else(std::env::home_dir)?;
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        Some(Dirs::new(home, xdg.as_deref()))
    }

    /// `<config_dir>/config.toml`.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// `<config_dir>/state.json`, the sidecar file.
    pub fn state_file(&self) -> PathBuf {
        self.config_dir.join("state.json")
    }

    /// Expands a leading `~` against this home directory.
    pub fn expand(&self, path: &str) -> PathBuf {
        expand_tilde(path, &self.home)
    }
}

/// Expands a leading `~` or `~/` to `home`. Other paths, including the
/// unsupported `~user` form, are returned unchanged.
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Shortens a path under `home` to the `~/…` form for display (doc/0006),
/// e.g. `/home/user/workspace/api` becomes `~/workspace/api`.
pub fn shorten_home(path: &str, home: &Path) -> String {
    let Some(home) = home.to_str().map(|h| h.trim_end_matches('/')) else {
        return path.to_owned();
    };
    if home.is_empty() {
        return path.to_owned();
    }
    match path.strip_prefix(home) {
        Some("") => "~".to_owned(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expansion() {
        let home = Path::new("/home/user");
        assert_eq!(expand_tilde("~", home), PathBuf::from("/home/user"));
        assert_eq!(
            expand_tilde("~/.claude", home),
            PathBuf::from("/home/user/.claude")
        );
        assert_eq!(expand_tilde("/opt/x", home), PathBuf::from("/opt/x"));
        assert_eq!(expand_tilde("~other/x", home), PathBuf::from("~other/x"));
        assert_eq!(expand_tilde("rel/~", home), PathBuf::from("rel/~"));
    }

    #[test]
    fn xdg_and_default_dirs() {
        let d = Dirs::new("/home/user", None);
        assert_eq!(
            d.config_file(),
            PathBuf::from("/home/user/.config/watchtower/config.toml")
        );
        assert_eq!(
            d.state_file(),
            PathBuf::from("/home/user/.config/watchtower/state.json")
        );
        let d = Dirs::new("/home/user", Some(Path::new("/xdg")));
        assert_eq!(d.config_dir, PathBuf::from("/xdg/watchtower"));
        // Relative XDG_CONFIG_HOME is ignored per the XDG spec.
        let d = Dirs::new("/home/user", Some(Path::new("rel")));
        assert_eq!(d.config_dir, PathBuf::from("/home/user/.config/watchtower"));
    }

    #[test]
    fn shorten_home_paths() {
        let home = Path::new("/home/user");
        assert_eq!(
            shorten_home("/home/user/workspace/api", home),
            "~/workspace/api"
        );
        assert_eq!(shorten_home("/home/user", home), "~");
        assert_eq!(shorten_home("/home/username/x", home), "/home/username/x");
        assert_eq!(shorten_home("/srv/x", home), "/srv/x");
    }
}
