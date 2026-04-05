//! Helpers for cleaning stale agent artifacts.

use std::fs;
use std::path::Path;

/// Clean up background process log files older than `max_age_days`.
pub fn cleanup_old_logs(log_dir: &Path, max_age_days: u64) -> Result<usize, anyhow::Error> {
    if !log_dir.exists() {
        return Ok(0);
    }

    let mut cleaned = 0;
    let now = std::time::SystemTime::now();
    let max_age = std::time::Duration::from_secs(max_age_days * 24 * 60 * 60);

    for entry in fs::read_dir(log_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "log") {
            if let Ok(metadata) = fs::metadata(&path) {
                if let Ok(modified) = metadata.modified() {
                    if now.duration_since(modified).unwrap_or(max_age) > max_age {
                        fs::remove_file(&path)?;
                        cleaned += 1;
                    }
                }
            }
        }
    }

    Ok(cleaned)
}

/// Clean up stale background process tracking files.
pub fn cleanup_background_processes(process_dir: &Path) -> Result<usize, anyhow::Error> {
    if !process_dir.exists() {
        return Ok(0);
    }

    let mut cleaned = 0;
    let now = std::time::SystemTime::now();
    let max_age = std::time::Duration::from_secs(24 * 60 * 60);

    for entry in fs::read_dir(process_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "pid") {
            if let Ok(metadata) = fs::metadata(&path) {
                if let Ok(modified) = metadata.modified() {
                    if now.duration_since(modified).unwrap_or(max_age) > max_age {
                        fs::remove_file(&path)?;
                        cleaned += 1;
                    }
                }
            } else {
                fs::remove_file(&path)?;
                cleaned += 1;
            }
        }
    }

    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_cleanup_old_logs() {
        let temp_dir = TempDir::new().unwrap();
        let log_path = temp_dir.path().join("old.log");
        fs::write(&log_path, "content").unwrap();

        let two_days_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 60 * 60);
        let file_time = filetime::FileTime::from_system_time(two_days_ago);
        filetime::set_file_mtime(&log_path, file_time).unwrap();

        let cleaned = cleanup_old_logs(temp_dir.path(), 1).unwrap();
        assert_eq!(cleaned, 1);
        assert!(!log_path.exists());
    }

    #[test]
    fn test_cleanup_old_logs_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let cleaned = cleanup_old_logs(temp_dir.path(), 1).unwrap();
        assert_eq!(cleaned, 0);
    }

    #[test]
    fn test_cleanup_background_processes() {
        let temp_dir = TempDir::new().unwrap();
        let pid_path = temp_dir.path().join("old.pid");
        fs::write(&pid_path, "12345").unwrap();

        let two_days_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 60 * 60);
        let file_time = filetime::FileTime::from_system_time(two_days_ago);
        filetime::set_file_mtime(&pid_path, file_time).unwrap();

        let cleaned = cleanup_background_processes(temp_dir.path()).unwrap();
        assert_eq!(cleaned, 1);
        assert!(!pid_path.exists());
    }
}
