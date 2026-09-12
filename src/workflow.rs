use crate::Result;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const SAVE_FILE: &str = "SAVEDATA.BIN";
const SYSTEM_DIR: &str = "SYSTEMSAVEDATA00";
const CHARACTER_DIR: &str = "SAVEDATA00";

#[derive(Debug, Clone, Copy)]
pub enum SaveKind {
    Hand,
    Magic,
}

impl SaveKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hand => "手搓存档",
            Self::Magic => "魔改存档",
        }
    }

    fn directory(self) -> &'static str {
        match self {
            Self::Hand => "HandMakeSave",
            Self::Magic => "MagicMakeSave",
        }
    }
}

#[derive(Debug)]
pub struct SavePlan {
    source_dir: PathBuf,
    account_dir: PathBuf,
    tool_dir: PathBuf,
    tool_exe: PathBuf,
    mine_dir: PathBuf,
    other_dir: PathBuf,
}

impl SavePlan {
    /// Validate everything before Steam is stopped or any file is changed.
    pub fn new(asset_root: &Path, account_dir: &Path, kind: SaveKind) -> Result<Self> {
        require_directory(asset_root)?;
        require_directory(account_dir)?;
        let source_dir = asset_root.join(kind.directory());
        let tool_dir = asset_root.join("Nioh3SaveCertificateTool");
        let plan = Self {
            source_dir,
            account_dir: account_dir.to_path_buf(),
            tool_exe: tool_dir.join("双击我改签v0.5.exe"),
            mine_dir: tool_dir.join("你的存档扔里面"),
            other_dir: tool_dir.join("别人存档扔里面"),
            tool_dir,
        };
        require_directory(&plan.source_dir)?;
        require_directory(&plan.tool_dir)?;
        require_nonempty_file(&plan.tool_exe)?;
        for directory in [SYSTEM_DIR, CHARACTER_DIR] {
            require_directory(&plan.source_dir.join(directory))?;
            require_nonempty_file(&plan.source_dir.join(directory).join(SAVE_FILE))?;
        }
        require_directory(&plan.account_dir.join(SYSTEM_DIR))?;
        require_nonempty_file(&plan.account_dir.join(SYSTEM_DIR).join(SAVE_FILE))?;
        optional_directory(&plan.account_dir.join(CHARACTER_DIR))?;
        optional_file(&plan.account_dir.join(CHARACTER_DIR).join(SAVE_FILE))?;
        for directory in [&plan.mine_dir, &plan.other_dir] {
            optional_directory(directory)?;
            optional_file(&directory.join(SAVE_FILE))?;
        }
        Ok(plan)
    }

    /// Both signed results are collected and staging files restored before local
    /// saves are replaced. The caller must confirm success in each signer window.
    pub fn execute<F>(&self, backup_dir: &Path, mut sign: F) -> Result<()>
    where
        F: FnMut(&Path, &Path, &str) -> Result<()>,
    {
        let _tool_lock = ToolLock::acquire(&self.tool_dir)?;
        self.execute_inner(backup_dir, &mut sign)
            .map_err(|error| format!("{error}\n本次备份目录：{}", backup_dir.display()))
    }

    fn execute_inner<F>(&self, backup_dir: &Path, sign: &mut F) -> Result<()>
    where
        F: FnMut(&Path, &Path, &str) -> Result<()>,
    {
        // Do not overwrite earlier backups when a caller reuses a directory.
        let local_system = Snapshot::capture(
            &self.account_dir.join(SYSTEM_DIR).join(SAVE_FILE),
            &backup_dir.join("local").join(SYSTEM_DIR).join(SAVE_FILE),
            true,
        )?;
        let local_character = Snapshot::capture(
            &self.account_dir.join(CHARACTER_DIR).join(SAVE_FILE),
            &backup_dir.join("local").join(CHARACTER_DIR).join(SAVE_FILE),
            false,
        )?;
        let original_mine = Snapshot::capture(
            &self.mine_dir.join(SAVE_FILE),
            &backup_dir.join("staging").join("mine").join(SAVE_FILE),
            false,
        )?;
        let original_other = Snapshot::capture(
            &self.other_dir.join(SAVE_FILE),
            &backup_dir.join("staging").join("other").join(SAVE_FILE),
            false,
        )?;
        let mine_existed = self.mine_dir.is_dir();
        let other_existed = self.other_dir.is_dir();
        let signed_system = backup_dir.join("signed").join(SYSTEM_DIR).join(SAVE_FILE);
        let signed_character = backup_dir
            .join("signed")
            .join(CHARACTER_DIR)
            .join(SAVE_FILE);
        let sign_result: Result<()> = (|| {
            for (directory, stage, output) in [
                (
                    SYSTEM_DIR,
                    "系统设置存档 (SYSTEMSAVEDATA00)",
                    &signed_system,
                ),
                (
                    CHARACTER_DIR,
                    "角色数据存档 (SAVEDATA00)",
                    &signed_character,
                ),
            ] {
                // The original workflow uses the user's SYSTEM save as the
                // signing identity in BOTH rounds. Restore it each round even
                // if the external program has modified its staging copy.
                replace_file(
                    local_system
                        .backup
                        .as_ref()
                        .expect("required system backup"),
                    &self.mine_dir.join(SAVE_FILE),
                )?;
                replace_file(
                    &self.source_dir.join(directory).join(SAVE_FILE),
                    &self.other_dir.join(SAVE_FILE),
                )?;
                sign(&self.tool_exe, &self.tool_dir, stage)?;
                require_nonempty_file(&self.other_dir.join(SAVE_FILE))?;
                copy_new(&self.other_dir.join(SAVE_FILE), output)?;
            }
            Ok(())
        })();

        // Restore the bundled staging files on cancellation and on success.
        // If this fails, do not proceed to replace the user's local saves.
        let staging_errors = restore_all(&[&original_mine, &original_other]);
        for (directory, existed) in [
            (&self.mine_dir, mine_existed),
            (&self.other_dir, other_existed),
        ] {
            if !existed {
                // Remove only an empty directory created by this operation;
                // preserve any other files created by the signing program.
                let _ = fs::remove_dir(directory);
            }
        }
        if let Err(error) = sign_result {
            return Err(append_errors(
                format!("改签已停止，本地存档尚未写入：{error}"),
                &staging_errors,
            ));
        }
        if !staging_errors.is_empty() {
            return Err(append_errors(
                "工具暂存文件恢复失败，本地存档尚未写入。".to_owned(),
                &staging_errors,
            ));
        }

        // The game or another program may have saved during the manual signing
        // steps. Stop before any write; rolling back here would destroy that save.
        local_system.ensure_unchanged()?;
        local_character.ensure_unchanged()?;
        commit_local_saves(
            &local_system,
            &local_character,
            &signed_system,
            &signed_character,
        )
    }
}

struct ToolLock {
    _file: File,
    #[cfg(not(windows))]
    path: PathBuf,
}

impl ToolLock {
    fn acquire(tool_dir: &Path) -> Result<Self> {
        let path = tool_dir.join(".nioh3-save-manager.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // The file may remain after a crash; exclusivity belongs to the
            // live Windows handle, so a stale file never prevents a later run.
            const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
            options
                .create(true)
                .truncate(false)
                .share_mode(0)
                .attributes(FILE_ATTRIBUTE_HIDDEN);
        }
        #[cfg(not(windows))]
        options.create_new(true);
        let file = options.open(&path).map_err(|error| {
            format!(
                "无法独占锁定改签工具，可能已有另一个操作正在运行。请等待其结束后重试。\n锁文件：{}\n{error}",
                path.display()
            )
        })?;
        Ok(Self {
            _file: file,
            #[cfg(not(windows))]
            path,
        })
    }
}

#[cfg(not(windows))]
impl Drop for ToolLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn commit_local_saves(
    system: &Snapshot,
    character: &Snapshot,
    signed_system: &Path,
    signed_character: &Path,
) -> Result<()> {
    let character_directory = character.original.parent().expect("save parent");
    let character_directory_existed = character_directory.is_dir();
    let commit_result: Result<()> = (|| {
        replace_file(signed_system, &system.original)?;
        replace_file(signed_character, &character.original)?;
        Ok(())
    })();
    if let Err(error) = commit_result {
        let rollback_errors = restore_all(&[system, character]);
        if !character_directory_existed {
            let _ = fs::remove_dir(character_directory);
        }
        let status = if rollback_errors.is_empty() {
            "原始本地存档已恢复。"
        } else {
            "部分原始本地存档未能恢复，请从备份目录恢复。"
        };
        return Err(append_errors(
            format!("写回存档失败：{error}\n{status}"),
            &rollback_errors,
        ));
    }
    Ok(())
}

struct Snapshot {
    original: PathBuf,
    backup: Option<PathBuf>,
}

impl Snapshot {
    fn capture(original: &Path, backup: &Path, required: bool) -> Result<Self> {
        let exists = optional_file(original)?;
        if required && !exists {
            return Err(format!("备份前发现文件已不存在：{}", original.display()));
        }
        if exists {
            copy_new(original, backup)?;
        }
        Ok(Self {
            original: original.to_path_buf(),
            backup: exists.then(|| backup.to_path_buf()),
        })
    }

    fn ensure_unchanged(&self) -> Result<()> {
        let changed_message = || {
            format!(
                "改签期间本地存档已被其他程序更改，已停止写回并保留其最新内容：{}",
                self.original.display()
            )
        };
        let exists = optional_file(&self.original)
            .map_err(|error| format!("无法核对本地存档，已停止写回：{error}"))?;
        match &self.backup {
            None if !exists => Ok(()),
            None => Err(changed_message()),
            Some(_) if !exists => Err(changed_message()),
            Some(backup) => match files_equal(backup, &self.original) {
                Ok(true) => Ok(()),
                Ok(false) => Err(changed_message()),
                Err(error) => Err(format!(
                    "无法核对本地存档，已停止写回：{}：{error}",
                    self.original.display()
                )),
            },
        }
    }

    fn restore(&self) -> Result<()> {
        match &self.backup {
            Some(backup) => replace_file(backup, &self.original),
            None => match fs::remove_file(&self.original) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!(
                    "无法删除本次新建的文件 {}：{error}",
                    self.original.display()
                )),
            },
        }
    }
}

fn files_equal(left: &Path, right: &Path) -> io::Result<bool> {
    let mut left = File::open(left)?;
    let mut right = File::open(right)?;
    if left.metadata()?.len() != right.metadata()?.len() {
        return Ok(false);
    }
    let mut left_buffer = [0_u8; 64 * 1024];
    let mut right_buffer = [0_u8; 64 * 1024];
    loop {
        let count = left.read(&mut left_buffer)?;
        if count == 0 {
            return right.read(&mut right_buffer[..1]).map(|count| count == 0);
        }
        right.read_exact(&mut right_buffer[..count])?;
        if left_buffer[..count] != right_buffer[..count] {
            return Ok(false);
        }
    }
}

fn restore_all(snapshots: &[&Snapshot]) -> Vec<String> {
    snapshots
        .iter()
        .filter_map(|snapshot| snapshot.restore().err())
        .collect()
}

fn append_errors(mut message: String, errors: &[String]) -> String {
    for error in errors {
        message.push_str("\n恢复错误：");
        message.push_str(error);
    }
    message
}

fn require_directory(path: &Path) -> Result<()> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("无法访问目录 {}：{error}", path.display()))?;
    if !metadata.is_dir() {
        return Err(format!("此路径不是目录：{}", path.display()));
    }
    Ok(())
}

fn optional_directory(path: &Path) -> Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!("此路径不是目录：{}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法访问目录 {}：{error}", path.display())),
    }
}

fn optional_file(path: &Path) -> Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(format!("此路径不是文件：{}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("无法访问文件 {}：{error}", path.display())),
    }
}

fn require_nonempty_file(path: &Path) -> Result<()> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("无法访问文件 {}：{error}", path.display()))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(format!("文件不是有效的非空文件：{}", path.display()));
    }
    Ok(())
}

fn copy_new(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("文件缺少父目录：{}", destination.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("无法创建目录 {}：{error}", parent.display()))?;
    let mut input =
        File::open(source).map_err(|error| format!("无法读取 {}：{error}", source.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            format!(
                "无法创建文件（不会覆盖旧备份）{}：{error}",
                destination.display()
            )
        })?;
    let result = io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    drop(output);
    if let Err(error) = result {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "无法复制 {} 到 {}：{error}",
            source.display(),
            destination.display()
        ));
    }
    Ok(())
}

/// Write and flush alongside the target first, so a copy failure cannot leave
/// the target truncated. Backups cover failure between the two final renames.
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = destination
        .parent()
        .ok_or_else(|| format!("文件缺少父目录：{}", destination.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        ".nioh3-{}-{timestamp}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    copy_new(source, &temporary)?;
    if let Err(error) = fs::rename(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("无法替换 {}：{error}", destination.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        account: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "nioh3-workflow-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            let account = root.join("accounts").join("76561198000000099");
            for (kind, data) in [(SaveKind::Hand, b"hand"), (SaveKind::Magic, b"mods")] {
                for directory in [SYSTEM_DIR, CHARACTER_DIR] {
                    write(
                        &root.join(kind.directory()).join(directory).join(SAVE_FILE),
                        data,
                    );
                }
            }
            write(&account.join(SYSTEM_DIR).join(SAVE_FILE), b"my-system");
            write(
                &account.join(CHARACTER_DIR).join(SAVE_FILE),
                b"my-character",
            );
            write(
                &root
                    .join("Nioh3SaveCertificateTool")
                    .join("双击我改签v0.5.exe"),
                b"fake executable, never launched",
            );
            Self { root, account }
        }

        fn plan(&self, kind: SaveKind) -> SavePlan {
            SavePlan::new(&self.root, &self.account, kind).unwrap()
        }

        fn backup(&self) -> PathBuf {
            self.root.join("backup")
        }

        fn assert_local_unchanged(&self) {
            assert_eq!(
                read(&self.account.join(SYSTEM_DIR).join(SAVE_FILE)),
                b"my-system"
            );
            assert_eq!(
                read(&self.account.join(CHARACTER_DIR).join(SAVE_FILE)),
                b"my-character"
            );
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only this unique fixture directory, never real game saves.
            if let (Ok(root), Ok(temp)) = (
                self.root.canonicalize(),
                std::env::temp_dir().canonicalize(),
            ) {
                if root.is_absolute() && root != temp && root.starts_with(&temp) {
                    let _ = fs::remove_dir_all(root);
                }
            }
        }
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn read(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap()
    }

    #[test]
    fn both_save_choices_use_selected_account_and_restore_staging() {
        for (kind, expected) in [(SaveKind::Hand, b"hand"), (SaveKind::Magic, b"mods")] {
            let fixture = Fixture::new();
            let plan = fixture.plan(kind);
            write(&plan.mine_dir.join(SAVE_FILE), b"bundled-mine");
            write(&plan.other_dir.join(SAVE_FILE), b"bundled-other");
            let other_account = fixture.root.join("accounts").join("76561198000000123");
            write(
                &other_account.join(SYSTEM_DIR).join(SAVE_FILE),
                b"other-account",
            );
            let mut calls = 0;
            plan.execute(&fixture.backup(), |_, tool, stage| {
                calls += 1;
                assert_eq!(tool, plan.tool_dir);
                assert!(stage.contains(if calls == 1 {
                    SYSTEM_DIR
                } else {
                    CHARACTER_DIR
                }));
                assert_eq!(read(&plan.mine_dir.join(SAVE_FILE)), b"my-system");
                assert_eq!(read(&plan.other_dir.join(SAVE_FILE)), expected);
                fixture.assert_local_unchanged();
                write(
                    &plan.other_dir.join(SAVE_FILE),
                    format!("signed-{calls}").as_bytes(),
                );
                write(
                    &plan.mine_dir.join(SAVE_FILE),
                    b"signer-changed-identity-copy",
                );
                Ok(())
            })
            .unwrap();
            assert_eq!(calls, 2);
            assert_eq!(
                read(&fixture.account.join(SYSTEM_DIR).join(SAVE_FILE)),
                b"signed-1"
            );
            assert_eq!(
                read(&fixture.account.join(CHARACTER_DIR).join(SAVE_FILE)),
                b"signed-2"
            );
            assert_eq!(
                read(&other_account.join(SYSTEM_DIR).join(SAVE_FILE)),
                b"other-account"
            );
            assert_eq!(read(&plan.mine_dir.join(SAVE_FILE)), b"bundled-mine");
            assert_eq!(read(&plan.other_dir.join(SAVE_FILE)), b"bundled-other");
            assert_eq!(
                read(
                    &fixture
                        .backup()
                        .join("local")
                        .join(SYSTEM_DIR)
                        .join(SAVE_FILE)
                ),
                b"my-system"
            );
            assert_eq!(
                read(
                    &fixture
                        .backup()
                        .join("local")
                        .join(CHARACTER_DIR)
                        .join(SAVE_FILE)
                ),
                b"my-character"
            );
        }
    }

    #[test]
    fn either_signing_failure_keeps_local_saves_and_restores_staging() {
        for fail_on in [1, 2] {
            let fixture = Fixture::new();
            let plan = fixture.plan(SaveKind::Hand);
            write(&plan.mine_dir.join(SAVE_FILE), b"bundled-mine");
            write(&plan.other_dir.join(SAVE_FILE), b"bundled-other");
            let mut calls = 0;
            let error = plan
                .execute(&fixture.backup(), |_, _, _| {
                    calls += 1;
                    write(&plan.other_dir.join(SAVE_FILE), b"partially-signed");
                    if calls == fail_on {
                        Err("模拟用户取消".to_owned())
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
            assert_eq!(calls, fail_on);
            assert!(error.contains("本地存档尚未写入"));
            fixture.assert_local_unchanged();
            assert_eq!(read(&plan.mine_dir.join(SAVE_FILE)), b"bundled-mine");
            assert_eq!(read(&plan.other_dir.join(SAVE_FILE)), b"bundled-other");
        }
    }

    #[test]
    fn absent_staging_and_character_directories_are_created_when_needed() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.account.join(CHARACTER_DIR).join(SAVE_FILE)).unwrap();
        fs::remove_dir(fixture.account.join(CHARACTER_DIR)).unwrap();
        let plan = fixture.plan(SaveKind::Hand);
        assert!(!plan.mine_dir.exists());
        assert!(!plan.other_dir.exists());
        plan.execute(&fixture.backup(), |_, _, _| Ok(())).unwrap();
        assert_eq!(
            read(&fixture.account.join(CHARACTER_DIR).join(SAVE_FILE)),
            b"hand"
        );
        assert!(!plan.mine_dir.exists());
        assert!(!plan.other_dir.exists());
    }

    #[test]
    fn empty_or_missing_signed_output_never_replaces_local_saves() {
        for delete in [false, true] {
            let fixture = Fixture::new();
            let plan = fixture.plan(SaveKind::Hand);
            let error = plan
                .execute(&fixture.backup(), |_, _, _| {
                    if delete {
                        fs::remove_file(plan.other_dir.join(SAVE_FILE)).unwrap();
                    } else {
                        write(&plan.other_dir.join(SAVE_FILE), b"");
                    }
                    Ok(())
                })
                .unwrap_err();
            assert!(error.contains("本地存档尚未写入"));
            fixture.assert_local_unchanged();
            assert!(!plan.mine_dir.exists());
            assert!(!plan.other_dir.exists());
        }
    }

    #[test]
    fn preflight_reports_missing_files_and_does_not_create_staging() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.account.join(SYSTEM_DIR).join(SAVE_FILE)).unwrap();
        let error = SavePlan::new(&fixture.root, &fixture.account, SaveKind::Hand).unwrap_err();
        assert!(error.contains(&fixture.account.display().to_string()));
        assert!(!fixture
            .root
            .join("Nioh3SaveCertificateTool")
            .join("你的存档扔里面")
            .exists());
    }

    #[test]
    fn preflight_rejects_a_file_in_place_of_a_staging_directory() {
        let fixture = Fixture::new();
        let wrong_path = fixture
            .root
            .join("Nioh3SaveCertificateTool")
            .join("别人存档扔里面");
        write(&wrong_path, b"not a directory");
        let error = SavePlan::new(&fixture.root, &fixture.account, SaveKind::Hand).unwrap_err();
        assert!(error.contains("不是目录"));
        fixture.assert_local_unchanged();
    }

    #[test]
    fn existing_backup_is_never_overwritten() {
        let fixture = Fixture::new();
        let plan = fixture.plan(SaveKind::Hand);
        let old_backup = fixture
            .backup()
            .join("local")
            .join(SYSTEM_DIR)
            .join(SAVE_FILE);
        write(&old_backup, b"previous-backup");
        let mut called = false;
        assert!(plan
            .execute(&fixture.backup(), |_, _, _| {
                called = true;
                Ok(())
            })
            .is_err());
        assert!(!called);
        assert_eq!(read(&old_backup), b"previous-backup");
        fixture.assert_local_unchanged();
    }

    #[test]
    fn failure_writing_second_save_restores_the_first_save() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.account.join(CHARACTER_DIR).join(SAVE_FILE)).unwrap();
        fs::remove_dir(fixture.account.join(CHARACTER_DIR)).unwrap();
        let system = Snapshot::capture(
            &fixture.account.join(SYSTEM_DIR).join(SAVE_FILE),
            &fixture.backup().join("system"),
            true,
        )
        .unwrap();
        let character = Snapshot::capture(
            &fixture.account.join(CHARACTER_DIR).join(SAVE_FILE),
            &fixture.backup().join("character"),
            false,
        )
        .unwrap();
        let signed_system = fixture.backup().join("signed-system");
        let signed_character = fixture.backup().join("signed-character");
        write(&signed_system, b"signed-system");
        write(&signed_character, b"signed-character");
        // Exercise the commit rollback directly: the parent becomes unavailable
        // after validation, and the system save is written before this fails.
        write(&fixture.account.join(CHARACTER_DIR), b"blocking-file");
        let error =
            commit_local_saves(&system, &character, &signed_system, &signed_character).unwrap_err();
        assert!(error.contains("写回存档失败"));
        assert_eq!(
            read(&fixture.account.join(SYSTEM_DIR).join(SAVE_FILE)),
            b"my-system"
        );
    }

    #[test]
    fn overlapping_runs_cannot_share_the_tool_and_failure_releases_the_lock() {
        let fixture = Fixture::new();
        let plan = fixture.plan(SaveKind::Hand);
        let competing_plan = fixture.plan(SaveKind::Magic);
        let competing_backup = fixture.root.join("competing-backup");
        let mut competing_sign_called = false;
        let error = plan
            .execute(&fixture.backup(), |_, _, _| {
                let competing_error = competing_plan
                    .execute(&competing_backup, |_, _, _| {
                        competing_sign_called = true;
                        Ok(())
                    })
                    .unwrap_err();
                assert!(competing_error.contains("已有另一个操作正在运行"));
                assert!(!competing_backup.exists());
                Err("模拟取消".to_owned())
            })
            .unwrap_err();
        assert!(error.contains("模拟取消"));
        assert!(!competing_sign_called);
        fixture.assert_local_unchanged();
        // Windows keeps the lock file, but the closed handle releases ownership.
        competing_plan
            .execute(&competing_backup, |_, _, _| Ok(()))
            .unwrap();
    }

    #[test]
    fn local_changes_during_signing_are_preserved_and_stop_all_writeback() {
        for (changed_directory, new_data, unchanged_directory, original_data) in [
            (
                SYSTEM_DIR,
                b"changed!!".as_slice(),
                CHARACTER_DIR,
                b"my-character".as_slice(),
            ),
            (
                CHARACTER_DIR,
                b"changed-data".as_slice(),
                SYSTEM_DIR,
                b"my-system".as_slice(),
            ),
        ] {
            let fixture = Fixture::new();
            let plan = fixture.plan(SaveKind::Hand);
            let changed_file = fixture.account.join(changed_directory).join(SAVE_FILE);
            let mut calls = 0;
            let error = plan
                .execute(&fixture.backup(), |_, _, _| {
                    calls += 1;
                    write(&plan.other_dir.join(SAVE_FILE), b"signed");
                    if calls == 2 {
                        // Same-length data also has to be compared by content.
                        write(&changed_file, new_data);
                    }
                    Ok(())
                })
                .unwrap_err();
            assert_eq!(calls, 2);
            assert!(error.contains("已停止写回并保留其最新内容"));
            assert!(error.contains(&changed_file.display().to_string()));
            assert!(error.contains(&fixture.backup().display().to_string()));
            assert_eq!(read(&changed_file), new_data);
            assert_eq!(
                read(&fixture.account.join(unchanged_directory).join(SAVE_FILE)),
                original_data
            );
            assert!(!plan.mine_dir.exists());
            assert!(!plan.other_dir.exists());
        }
    }

    #[test]
    fn a_new_character_save_created_during_signing_is_preserved() {
        let fixture = Fixture::new();
        let character = fixture.account.join(CHARACTER_DIR).join(SAVE_FILE);
        fs::remove_file(&character).unwrap();
        let plan = fixture.plan(SaveKind::Hand);
        let error = plan
            .execute(&fixture.backup(), |_, _, _| {
                write(&character, b"new-game-save");
                Ok(())
            })
            .unwrap_err();
        assert!(error.contains("已停止写回并保留其最新内容"));
        assert_eq!(read(&character), b"new-game-save");
        assert_eq!(
            read(&fixture.account.join(SYSTEM_DIR).join(SAVE_FILE)),
            b"my-system"
        );
    }

    #[test]
    fn rollback_removes_a_file_that_did_not_exist_before_the_run() {
        let fixture = Fixture::new();
        let original = fixture.account.join("new-save").join(SAVE_FILE);
        let snapshot =
            Snapshot::capture(&original, &fixture.backup().join("missing"), false).unwrap();
        write(&original, b"newly-created");
        snapshot.restore().unwrap();
        assert!(!original.exists());
    }
}
