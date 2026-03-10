use std::{
    io::{Error, ErrorKind, Result},
    path::PathBuf,
};

use directories::ProjectDirs;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct AppPaths {
    pub db_path: PathBuf,
    pub config_path: PathBuf,
    pub hooks_dir: PathBuf,
}

#[allow(dead_code)]
impl AppPaths {
    pub fn new() -> Result<Self> {
        let project_dirs = ProjectDirs::from("", "", "diddo").ok_or_else(|| {
            Error::new(
                ErrorKind::NotFound,
                "could not determine platform-specific diddo directories",
            )
        })?;
        Ok(Self::from_roots(
            project_dirs.data_local_dir().to_path_buf(),
            project_dirs.config_dir().to_path_buf(),
        ))
    }

    fn from_roots(db_root: PathBuf, config_root: PathBuf) -> Self {
        Self {
            db_path: db_root.join("commits.db"),
            config_path: config_root.join("config.toml"),
            hooks_dir: config_root.join("hooks"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, path::PathBuf};

    use super::{AppPaths, Error, ErrorKind, ProjectDirs};

    #[test]
    fn from_roots_keeps_db_in_local_data_root_and_config_files_in_config_root() {
        let db_root = PathBuf::from("/tmp/diddo-local-data");
        let config_root = PathBuf::from("/tmp/diddo-config");
        let paths = AppPaths::from_roots(db_root.clone(), config_root.clone());

        assert_eq!(paths.db_path, db_root.join("commits.db"));
        assert_eq!(paths.config_path, config_root.join("config.toml"));
        assert_eq!(paths.hooks_dir, config_root.join("hooks"));
    }

    #[test]
    fn new_uses_local_data_directory_for_database_file() {
        let project_dirs = project_dirs();
        let paths = AppPaths::new().unwrap();

        assert_eq!(
            paths.db_path,
            project_dirs.data_local_dir().join("commits.db")
        );
    }

    #[test]
    fn new_places_config_and_hooks_under_the_config_directory() {
        let project_dirs = project_dirs();
        let paths = AppPaths::new().unwrap();
        let config_dir = project_dirs.config_dir();

        assert_eq!(paths.config_path, config_dir.join("config.toml"));
        assert_eq!(paths.hooks_dir, config_dir.join("hooks"));
        assert_eq!(paths.hooks_dir.file_name(), Some(OsStr::new("hooks")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_local_data_for_db_and_config_dir_for_config_files() {
        let project_dirs = project_dirs();
        let paths = AppPaths::new().unwrap();

        assert_ne!(project_dirs.data_local_dir(), project_dirs.data_dir());
        assert_ne!(project_dirs.data_local_dir(), project_dirs.config_dir());
        assert_eq!(paths.db_path.parent(), Some(project_dirs.data_local_dir()));
        assert_eq!(paths.config_path.parent(), Some(project_dirs.config_dir()));
        assert_eq!(paths.hooks_dir.parent(), Some(project_dirs.config_dir()));
    }

    fn project_dirs() -> ProjectDirs {
        ProjectDirs::from("", "", "diddo")
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::NotFound,
                    "could not determine platform-specific diddo directories",
                )
            })
            .unwrap()
    }
}
