//! Steam account mapping and a deliberately conservative, format-preserving VDF editor.

use crate::Result;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const STEAM_ID_BASE: u64 = 76_561_197_960_265_728;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn find_steam_dir() -> Result<Option<PathBuf>> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        let output = Command::new("reg.exe")
            .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
            .creation_flags(0x08000000)
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                // Avoid guessing a path when reg.exe emits a legacy code page.
                if let Ok(text) = std::str::from_utf8(&output.stdout) {
                    for line in text.lines() {
                        if let Some((name, value)) = line.split_once("REG_SZ") {
                            if name.trim().eq_ignore_ascii_case("SteamPath") {
                                let path = PathBuf::from(value.trim());
                                if path.is_dir() {
                                    return Ok(Some(path));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let program_files = env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    Ok([
        program_files.join("Steam"),
        PathBuf::from(r"D:\Steam"),
        PathBuf::from(r"E:\Steam"),
    ]
    .into_iter()
    .find(|path| path.is_dir()))
}

/// Map the selected SteamID64 to exactly that user's Steam configuration.
pub fn config_path(steam_dir: &Path, steam_id: &str) -> Result<PathBuf> {
    if steam_id.is_empty() || !steam_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("无效的 Steam 账号目录名：{steam_id}"));
    }
    let account_id = steam_id
        .parse::<u64>()
        .ok()
        .and_then(|id| id.checked_sub(STEAM_ID_BASE))
        .filter(|id| *id > 0 && *id <= u32::MAX as u64)
        .ok_or_else(|| format!("存档目录名不是有效的个人 SteamID64：{steam_id}"))?;
    let path = steam_dir
        .join("userdata")
        .join(account_id.to_string())
        .join("7")
        .join("remote")
        .join("sharedconfig.vdf");
    if !path.is_file() {
        return Err(format!(
            "找不到所选账号 {steam_id} 的 Steam 配置：{}。请登录该账号后手动关闭仁王 3 的 Steam 云存档。",
            path.display()
        ));
    }
    Ok(path)
}

pub fn disable_cloud_in_config(config: &Path, app_id: &str, backup_dir: &Path) -> Result<()> {
    let original = fs::read(config)
        .map_err(|error| format!("读取 Steam 配置失败（{}）：{error}", config.display()))?;
    let source = std::str::from_utf8(&original)
        .map_err(|_| "Steam 配置不是有效的 UTF-8，未修改；请手动关闭云存档。".to_string())?;
    let updated = update_cloud(source, app_id)
        .map_err(|error| format!("无法安全修改 Steam 云存档配置：{error}。请手动关闭云存档。"))?;

    fs::create_dir_all(backup_dir)
        .map_err(|error| format!("创建配置备份目录失败（{}）：{error}", backup_dir.display()))?;
    let backup = backup_dir.join("sharedconfig.vdf");
    let mut backup_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .map_err(|error| {
            format!(
                "创建 Steam 配置备份失败（{}）：{error}；原配置未修改",
                backup.display()
            )
        })?;
    backup_file
        .write_all(&original)
        .and_then(|()| backup_file.sync_all())
        .map_err(|error| format!("保存 Steam 配置备份失败：{error}；原配置未修改"))?;
    drop(backup_file);

    if updated.as_bytes() == original {
        return Ok(());
    }

    let parent = config
        .parent()
        .ok_or_else(|| "Steam 配置路径没有父目录".to_string())?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("系统时间无效：{error}"))?
        .as_nanos();
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".nioh3-sharedconfig-{}-{timestamp}-{sequence}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("创建配置临时文件失败：{error}；原配置未修改"))?;
    let result = (|| -> Result<()> {
        file.write_all(updated.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("写入配置临时文件失败：{error}"))?;
        drop(file);
        let current = fs::read(config).map_err(|error| format!("复核 Steam 配置失败：{error}"))?;
        if current != original {
            return Err("Steam 配置在操作期间被其他程序修改，已停止替换".to_string());
        }
        // Replacement is atomic on the local Windows filesystem. Failed writes never truncate
        // the original, and the complete original is also retained in the backup directory.
        fs::rename(&temporary, config).map_err(|error| format!("替换 Steam 配置失败：{error}"))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "{error}；原配置未被本程序替换，备份位于 {}",
            backup.display()
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    span: Range<usize>,
}

#[derive(Debug, Clone)]
enum TokenKind {
    Word { text: String, quoted: bool },
    Open,
    Close,
}

#[derive(Debug)]
struct Entry {
    key: String,
    key_start: usize,
    value: Value,
}

#[derive(Debug)]
enum Value {
    Text { span: Range<usize>, quoted: bool },
    Object { entries: Vec<Entry>, close: usize },
}

fn tokenize(source: &str) -> Result<Vec<Token>> {
    let bytes = source.as_bytes();
    let mut index = if source.starts_with('\u{feff}') { 3 } else { 0 };
    let mut tokens = Vec::new();
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index..].starts_with(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index..].starts_with(b"/*") {
            let end = source[index + 2..]
                .find("*/")
                .ok_or_else(|| "VDF 注释没有结束".to_string())?;
            index += end + 4;
            continue;
        }
        let start = index;
        let kind = match bytes[index] {
            b'{' => {
                index += 1;
                TokenKind::Open
            }
            b'}' => {
                index += 1;
                TokenKind::Close
            }
            b'"' => {
                index += 1;
                let content_start = index;
                while index < bytes.len() && bytes[index] != b'"' {
                    if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
                if index >= bytes.len() {
                    return Err("VDF 字符串没有结束".to_string());
                }
                let mut text = String::new();
                let mut characters = source[content_start..index].chars();
                while let Some(character) = characters.next() {
                    if character == '\\' {
                        let escaped = characters
                            .next()
                            .ok_or_else(|| "VDF 转义不完整".to_string())?;
                        match escaped {
                            'n' => text.push('\n'),
                            'r' => text.push('\r'),
                            't' => text.push('\t'),
                            '"' | '\\' => text.push(escaped),
                            _ => {
                                text.push('\\');
                                text.push(escaped);
                            }
                        }
                    } else {
                        text.push(character);
                    }
                }
                index += 1;
                TokenKind::Word { text, quoted: true }
            }
            _ => {
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'{' | b'}' | b'"')
                    && !bytes[index..].starts_with(b"//")
                    && !bytes[index..].starts_with(b"/*")
                {
                    index += 1;
                }
                TokenKind::Word {
                    text: source[start..index].to_string(),
                    quoted: false,
                }
            }
        };
        tokens.push(Token {
            kind,
            span: start..index,
        });
    }
    Ok(tokens)
}

fn parse_entries(tokens: &[Token], cursor: &mut usize, depth: usize) -> Result<Vec<Entry>> {
    if depth > 128 {
        return Err("VDF 嵌套层数过多".to_string());
    }
    let mut entries = Vec::new();
    while let Some(token) = tokens.get(*cursor) {
        if matches!(token.kind, TokenKind::Close) {
            if depth == 0 {
                return Err("VDF 存在多余的右花括号".to_string());
            }
            return Ok(entries);
        }
        let TokenKind::Word { text: key, .. } = &token.kind else {
            return Err(format!("VDF 在字节 {} 处缺少键名", token.span.start));
        };
        let key = key.clone();
        let key_start = token.span.start;
        *cursor += 1;
        let value_token = tokens
            .get(*cursor)
            .ok_or_else(|| "VDF 键缺少值".to_string())?;
        let value = match &value_token.kind {
            TokenKind::Word { quoted, .. } => {
                *cursor += 1;
                Value::Text {
                    span: value_token.span.clone(),
                    quoted: *quoted,
                }
            }
            TokenKind::Open => {
                *cursor += 1;
                let children = parse_entries(tokens, cursor, depth + 1)?;
                let close = tokens
                    .get(*cursor)
                    .ok_or_else(|| "VDF 对象没有结束".to_string())?;
                if !matches!(close.kind, TokenKind::Close) {
                    return Err("VDF 对象没有右花括号".to_string());
                }
                *cursor += 1;
                Value::Object {
                    entries: children,
                    close: close.span.start,
                }
            }
            TokenKind::Close => return Err(format!("VDF 键 {key} 缺少值")),
        };
        entries.push(Entry {
            key,
            key_start,
            value,
        });
    }
    if depth > 0 {
        return Err("VDF 对象没有结束".to_string());
    }
    Ok(entries)
}

fn find_unique<'a>(entries: &'a [Entry], key: &str) -> Result<Option<&'a Entry>> {
    let mut matches = entries
        .iter()
        .filter(|entry| entry.key.eq_ignore_ascii_case(key));
    let first = matches.next();
    if matches.next().is_some() {
        return Err(format!("VDF 含有重复的 {key} 项，无法确定应修改哪一项"));
    }
    Ok(first)
}

fn object_entries(entry: &Entry) -> Result<&[Entry]> {
    match &entry.value {
        Value::Object { entries, .. } => Ok(entries),
        Value::Text { .. } => Err(format!("VDF 中的 {} 应为对象", entry.key)),
    }
}

fn line_indent(source: &str, position: usize) -> Option<&str> {
    let start = source[..position].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &source[start..position];
    prefix
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
        .then_some(prefix)
}

fn update_cloud(source: &str, app_id: &str) -> Result<String> {
    if app_id.is_empty() || !app_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("游戏 AppID 必须全部为数字".to_string());
    }
    let tokens = tokenize(source)?;
    let root_entries = parse_entries(&tokens, &mut 0, 0)?;
    let mut apps = Vec::new();
    // sharedconfig.vdf commonly uses the roaming root; older/alternative files can use local.
    for root_key in ["UserLocalConfigStore", "UserRoamingConfigStore"] {
        let Some(mut entry) = find_unique(&root_entries, root_key)? else {
            continue;
        };
        let mut complete = true;
        for key in ["Software", "Valve", "Steam", "apps", app_id] {
            let Some(child) = find_unique(object_entries(entry)?, key)? else {
                complete = false;
                break;
            };
            entry = child;
        }
        if complete {
            apps.push(entry);
        }
    }
    let app = match apps.as_slice() {
        [] => return Err(format!("未找到所选账号的游戏 {app_id} 配置对象")),
        [app] => *app,
        _ => return Err("多个配置根对象含有同一游戏，无法确定目标".to_string()),
    };
    let entries = object_entries(app)?;
    let mut updated = source.to_string();
    if let Some(cloud) = find_unique(entries, "CloudEnabled")? {
        let Value::Text { span, quoted } = &cloud.value else {
            return Err("CloudEnabled 的值不是字符串".to_string());
        };
        updated.replace_range(span.clone(), if *quoted { "\"0\"" } else { "0" });
    } else {
        let Value::Object { close, .. } = &app.value else {
            unreachable!()
        };
        let newline = if source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let parent_indent = line_indent(source, *close)
            .or_else(|| line_indent(source, app.key_start))
            .unwrap_or("");
        let child_indent = entries
            .iter()
            .find_map(|entry| line_indent(source, entry.key_start))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{parent_indent}\t"));
        let text = format!("{child_indent}\"CloudEnabled\"\t\t\"0\"{newline}");
        if let Some(close_indent) = line_indent(source, *close) {
            updated.insert_str(*close - close_indent.len(), &text);
        } else {
            updated.insert_str(*close, &format!("{newline}{text}{parent_indent}"));
        }
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new() -> Self {
            let id = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = env::temp_dir().join(format!(
                "nioh3-steam-test-{}-{timestamp}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&directory).unwrap();
            Self(directory)
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            if let (Ok(path), Ok(temp_root)) =
                (fs::canonicalize(&self.0), fs::canonicalize(env::temp_dir()))
            {
                if path.parent() == Some(temp_root.as_path())
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("nioh3-steam-test-"))
                {
                    let _ = fs::remove_dir_all(path);
                }
            }
        }
    }

    fn wrap(body: &str) -> String {
        format!("\"UserLocalConfigStore\" {{ \"Software\" {{ \"Valve\" {{ \"Steam\" {{ \"apps\" {{ {body} }} }} }} }} }}")
    }

    #[test]
    fn changes_only_direct_target_amid_nested_objects_and_comments() {
        let source = wrap(
            r#"
            // "4198760" { "CloudEnabled" "1" }
            "4198760" {
                "nested" { "CloudEnabled" "1" }
                /* braces } { and "quotes" are not syntax */
                "cloudenabled"   "1" // keep this comment
                "description" "quote: \" }"
            }
            "123" { "CloudEnabled" "1" }
        "#,
        );
        assert_eq!(
            update_cloud(&source, "4198760").unwrap(),
            source.replace("\"cloudenabled\"   \"1\"", "\"cloudenabled\"   \"0\"")
        );
    }

    #[test]
    fn inserts_direct_cloud_key_preserving_crlf_bom_and_nested_values() {
        let source = format!(
            "\u{feff}{}",
            wrap("\"4198760\" {\r\n\t\"nested\" { \"CloudEnabled\" \"1\" }\r\n}")
        );
        let updated = update_cloud(&source, "4198760").unwrap();
        assert!(updated.starts_with('\u{feff}'));
        assert!(updated.contains("\t\"CloudEnabled\"\t\t\"0\"\r\n}"));
        assert!(updated.contains("\"nested\" { \"CloudEnabled\" \"1\" }"));
        assert_eq!(update_cloud(&updated, "4198760").unwrap(), updated);
    }

    #[test]
    fn supports_roaming_root_case_insensitive_and_unquoted_values() {
        let source = "userroamingconfigstore { software { VALVE { Steam { APPS { 4198760 { CloudEnabled 1 } } } } } }";
        assert_eq!(
            update_cloud(source, "4198760").unwrap(),
            source.replace("CloudEnabled 1", "CloudEnabled 0")
        );
    }

    #[test]
    fn rejects_missing_wrong_duplicate_and_malformed_targets() {
        for source in [
            wrap("\"123\" { \"CloudEnabled\" \"1\" }"),
            "\"4198760\" { \"CloudEnabled\" \"1\" }".to_string(),
            wrap("\"4198760\" { CloudEnabled 1 cloudenabled 0 }"),
            wrap("\"4198760\" {} \"4198760\" {}"),
            wrap("\"4198760\" \"1\""),
            "\"UserLocalConfigStore\" {".to_string(),
            "\"UserLocalConfigStore\" { /* unfinished".to_string(),
        ] {
            assert!(update_cloud(&source, "4198760").is_err(), "{source}");
        }
    }

    #[test]
    fn selects_exact_account_and_never_another_available_account() {
        let directory = TemporaryDirectory::new();
        for account in [123_u64, 456] {
            let path = directory
                .0
                .join("userdata")
                .join(account.to_string())
                .join("7/remote/sharedconfig.vdf");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"test").unwrap();
            assert_eq!(
                config_path(&directory.0, &(STEAM_ID_BASE + account).to_string()).unwrap(),
                path
            );
        }
        assert!(config_path(&directory.0, &(STEAM_ID_BASE + 789).to_string()).is_err());
        for id in ["../123", "123", "76561197960265728", "99999999999999999999"] {
            assert!(config_path(&directory.0, id).is_err());
        }
    }

    #[test]
    fn backs_up_exact_original_before_writing_and_preserves_existing_backup() {
        let directory = TemporaryDirectory::new();
        let config = directory.0.join("sharedconfig.vdf");
        let backup = directory.0.join("backup");
        let source = wrap("\"4198760\" { \"CloudEnabled\" \"1\" }");
        fs::write(&config, &source).unwrap();
        disable_cloud_in_config(&config, "4198760", &backup).unwrap();
        assert_eq!(
            fs::read_to_string(backup.join("sharedconfig.vdf")).unwrap(),
            source
        );
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            update_cloud(&source, "4198760").unwrap()
        );
        assert!(disable_cloud_in_config(&config, "4198760", &backup).is_err());
        assert_eq!(
            fs::read_to_string(backup.join("sharedconfig.vdf")).unwrap(),
            source
        );
    }

    #[test]
    fn failed_lookup_or_backup_never_modifies_config() {
        let directory = TemporaryDirectory::new();
        let config = directory.0.join("sharedconfig.vdf");
        let source = wrap("\"4198760\" { \"CloudEnabled\" \"1\" }");
        fs::write(&config, &source).unwrap();
        let backup = directory.0.join("backup");
        assert!(disable_cloud_in_config(&config, "123", &backup).is_err());
        assert!(!backup.exists());
        fs::write(&backup, b"not a directory").unwrap();
        assert!(disable_cloud_in_config(&config, "4198760", &backup).is_err());
        assert_eq!(fs::read_to_string(&config).unwrap(), source);
    }
}
