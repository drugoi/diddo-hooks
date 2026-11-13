use std::{
    error::Error,
    io::{self, IsTerminal, Write},
};

use chrono::{DateTime, Days, Utc};

use crate::{PruneArgs, config::AppConfig, db::Database, paths::AppPaths};

pub fn run(args: PruneArgs) -> Result<(), Box<dyn Error>> {
    let paths = AppPaths::new()?;
    let days = args
        .days
        .unwrap_or(AppConfig::load(&paths.config_path)?.retention.days);
    let now = DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap();
    let cutoff = now
        .checked_sub_days(Days::new(u64::from(days.get())))
        .ok_or("Retention period is too large")?;
    // Stored timestamps are UTC RFC3339 and may include fractional seconds.
    // A cutoff without a suffix keeps every timestamp in the cutoff second.
    let cutoff_key = cutoff.format("%Y-%m-%dT%H:%M:%S").to_string();

    if !paths.db_path.try_exists()? {
        println!("0 commit records older than {cutoff}.");
        return Ok(());
    }
    let count = Database::count_commits_before(&paths.db_path, &cutoff_key)?;
    println!("{count} commit records older than {cutoff}.");
    if args.dry_run || count == 0 {
        return Ok(());
    }
    if !args.yes {
        let stdin = io::stdin();
        if !stdin.is_terminal() {
            return Err("Use --yes to confirm deletion in a non-interactive session".into());
        }
        print!("Delete these records and clear cached summaries? [y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        stdin.read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Cancelled.");
            return Ok(());
        }
    }
    let mut database = Database::open(&paths.db_path)?;
    let removed = database.prune_commits(&cutoff_key, count)?;
    println!("Deleted {removed} commit records and cleared cached summaries.");
    Ok(())
}
