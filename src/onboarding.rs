use std::error::Error;
use std::path::Path;

use crate::config;
use crate::db;

/// Full onboarding flow (git scan, identity selection, import) is implemented incrementally.
pub fn run(
    _database: &db::Database,
    _config_path: &Path,
    _config: config::AppConfig,
) -> Result<(), Box<dyn Error>> {
    Ok(())
}
