use crate::Result;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub path: PathBuf,
}

impl Account {
    pub fn has_system_save(&self) -> bool {
        self.path
            .join("SYSTEMSAVEDATA00")
            .join("SAVEDATA.BIN")
            .is_file()
    }
}

pub fn discover_accounts(savedata_root: &Path) -> Result<Vec<Account>> {
    let entries = fs::read_dir(savedata_root).map_err(|error| {
        format!(
            "无法读取游戏存档目录 {}：{}。请先运行游戏并创建存档，或指定正确的存档目录。",
            savedata_root.display(),
            error
        )
    })?;
    let mut accounts = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "无法读取存档目录 {} 中的项目：{}",
                savedata_root.display(),
                error
            )
        })?;
        let name = entry.file_name();
        let Some(id) = name.to_str() else {
            continue;
        };
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法检查存档路径 {}：{}", entry.path().display(), error))?;
        if file_type.is_dir() {
            accounts.push(Account {
                id: id.to_owned(),
                path: entry.path(),
            });
        }
    }
    accounts.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(accounts)
}

pub fn default_savedata_root() -> Result<PathBuf> {
    savedata_root_from_env(env::var_os("LOCALAPPDATA"), env::var_os("USERPROFILE"))
}

fn savedata_root_from_env(
    local_appdata: Option<OsString>,
    user_profile: Option<OsString>,
) -> Result<PathBuf> {
    let local_root = if let Some(path) = local_appdata.filter(|path| !path.is_empty()) {
        PathBuf::from(path)
    } else if let Some(path) = user_profile.filter(|path| !path.is_empty()) {
        PathBuf::from(path).join("AppData").join("Local")
    } else {
        return Err(
            "无法确定游戏存档目录：LOCALAPPDATA 和 USERPROFILE 均未设置，请指定存档目录。"
                .to_owned(),
        );
    };
    Ok(local_root.join("KoeiTecmo").join("NIOH3").join("Savedata"))
}

pub fn find_asset_root() -> Result<PathBuf> {
    let executable = env::current_exe().ok();
    let current_dir = env::current_dir().ok();
    find_asset_root_in(&asset_candidates(
        executable.as_deref(),
        current_dir.as_deref(),
    ))
}

fn asset_candidates(executable: Option<&Path>, current_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(directory) = executable.and_then(Path::parent) {
        // Covers an adjacent resource bundle, dist/, and Cargo's target/debug,
        // target/release, target/<triple>/release and test executables in deps/.
        for ancestor in directory.ancestors().take(5) {
            candidates.push(ancestor.to_path_buf());
        }
    }
    if let Some(directory) = current_dir {
        if !candidates.iter().any(|candidate| candidate == directory) {
            candidates.push(directory.to_path_buf());
        }
    }
    candidates
}

fn find_asset_root_in(candidates: &[PathBuf]) -> Result<PathBuf> {
    for candidate in candidates {
        if candidate.join("HandMakeSave").is_dir()
            && candidate.join("MagicMakeSave").is_dir()
            && candidate.join("Nioh3SaveCertificateTool").is_dir()
            && candidate
                .join("Nioh3SaveCertificateTool")
                .join("双击我改签v0.5.exe")
                .is_file()
        {
            return Ok(candidate.clone());
        }
    }
    let checked = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("；");
    Err(format!(
        "未找到资源目录：需要 HandMakeSave、MagicMakeSave、Nioh3SaveCertificateTool 三个目录及其中的 双击我改签v0.5.exe。已检查：{}。请使用 --assets <资源目录> 指定。",
        if checked.is_empty() { "无法取得程序目录及当前目录" } else { &checked }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDir {
        path: PathBuf,
        temp_root: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let temp_root = fs::canonicalize(env::temp_dir()).unwrap();
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = temp_root.join(format!(
                "nioh3-paths-test-{}-{}-{}",
                std::process::id(),
                timestamp,
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self { path, temp_root }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            // Only remove this test's resolved, directly contained temp directory.
            if let Ok(resolved) = fs::canonicalize(&self.path) {
                if resolved.parent() == Some(self.temp_root.as_path())
                    && resolved
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("nioh3-paths-test-"))
                {
                    let _ = fs::remove_dir_all(resolved);
                }
            }
        }
    }

    fn create_assets(path: &Path, with_executable: bool) {
        for name in ["HandMakeSave", "MagicMakeSave", "Nioh3SaveCertificateTool"] {
            fs::create_dir_all(path.join(name)).unwrap();
        }
        if with_executable {
            fs::write(
                path.join("Nioh3SaveCertificateTool")
                    .join("双击我改签v0.5.exe"),
                b"",
            )
            .unwrap();
        }
    }

    #[test]
    fn discovers_sorted_accounts_including_missing_system_save() {
        let temp = TestDir::new();
        fs::create_dir_all(temp.path.join("76561199999999999").join("SYSTEMSAVEDATA00")).unwrap();
        fs::write(
            temp.path
                .join("76561199999999999/SYSTEMSAVEDATA00/SAVEDATA.BIN"),
            b"save",
        )
        .unwrap();
        fs::create_dir(temp.path.join("76561190000000000")).unwrap();
        let accounts = discover_accounts(&temp.path).unwrap();
        assert_eq!(
            accounts
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            vec!["76561190000000000", "76561199999999999"]
        );
        assert_eq!(accounts[0].path, temp.path.join("76561190000000000"));
        assert!(!accounts[0].has_system_save());
        assert!(accounts[1].has_system_save());
    }

    #[test]
    fn ignores_non_numeric_directories_and_numeric_files() {
        let temp = TestDir::new();
        for name in ["backup", "123abc", "１２３", "-123", "12 3"] {
            fs::create_dir(temp.path.join(name)).unwrap();
        }
        fs::write(temp.path.join("456"), b"not a directory").unwrap();
        assert!(discover_accounts(&temp.path).unwrap().is_empty());
    }

    #[test]
    fn empty_directory_has_no_accounts() {
        let temp = TestDir::new();
        assert!(discover_accounts(&temp.path).unwrap().is_empty());
    }

    #[test]
    fn missing_root_error_includes_path() {
        let temp = TestDir::new();
        let missing = temp.path.join("missing");
        let error = discover_accounts(&missing).unwrap_err();
        assert!(error.contains(missing.to_str().unwrap()));
    }

    #[test]
    fn file_root_error_includes_path() {
        let temp = TestDir::new();
        let file = temp.path.join("file");
        fs::write(&file, b"").unwrap();
        assert!(discover_accounts(&file)
            .unwrap_err()
            .contains(file.to_str().unwrap()));
    }

    #[test]
    fn prefers_localappdata_and_falls_back_to_userprofile() {
        let suffix = Path::new("KoeiTecmo").join("NIOH3").join("Savedata");
        assert_eq!(
            savedata_root_from_env(Some("local".into()), Some("profile".into())).unwrap(),
            Path::new("local").join(&suffix)
        );
        for local in [None, Some(OsString::new())] {
            assert_eq!(
                savedata_root_from_env(local, Some("profile".into())).unwrap(),
                Path::new("profile")
                    .join("AppData")
                    .join("Local")
                    .join(&suffix)
            );
        }
        assert!(savedata_root_from_env(None, None).is_err());
        assert!(savedata_root_from_env(Some(OsString::new()), Some(OsString::new())).is_err());
    }

    #[test]
    fn skips_incomplete_asset_copy_and_allows_absent_staging_directories() {
        let temp = TestDir::new();
        let incomplete = temp.path.join("incomplete");
        let complete = temp.path.join("complete");
        create_assets(&incomplete, false);
        create_assets(&complete, true);
        assert_eq!(
            find_asset_root_in(&[incomplete, complete.clone()]).unwrap(),
            complete
        );
    }

    #[test]
    fn missing_asset_directories_produce_actionable_error() {
        let temp = TestDir::new();
        let error = find_asset_root_in(&[temp.path.clone()]).unwrap_err();
        assert!(error.contains(temp.path.to_str().unwrap()));
        assert!(error.contains("--assets"));
    }

    #[test]
    fn finds_assets_beside_executable_in_build_parent_and_current_directory() {
        let temp = TestDir::new();
        let project = temp.path.join("project");
        create_assets(&project, true);
        for executable in [
            project.join("app.exe"),
            project.join("target/debug/app.exe"),
            project.join("target/release/app.exe"),
            project.join("target/debug/deps/app-test.exe"),
            project.join("target/x86_64-pc-windows-msvc/release/app.exe"),
            project.join("dist/app.exe"),
        ] {
            let candidates = asset_candidates(Some(&executable), None);
            assert_eq!(find_asset_root_in(&candidates).unwrap(), project);
        }
        let elsewhere = temp.path.join("elsewhere/app.exe");
        let candidates = asset_candidates(Some(&elsewhere), Some(&project));
        assert_eq!(find_asset_root_in(&candidates).unwrap(), project);
    }
}
