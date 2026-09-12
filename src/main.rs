use nioh3_save_manager::confirmation;
use nioh3_save_manager::paths::{self, Account};
use nioh3_save_manager::signer;
use nioh3_save_manager::steam;
use nioh3_save_manager::workflow::{SaveKind, SavePlan};
use nioh3_save_manager::Result;
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Default)]
struct Options {
    assets: Option<PathBuf>,
    savedata_root: Option<PathBuf>,
    steam_dir: Option<PathBuf>,
    source_dir: Option<PathBuf>,
    account: Option<String>,
    check: bool,
    preview_confirmation: bool,
    help: bool,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--check") => options.check = true,
                Some("--preview-confirmation") => options.preview_confirmation = true,
                Some("--help" | "-h") => options.help = true,
                Some(
                    "--assets" | "--savedata-root" | "--steam-dir" | "--source-dir" | "--account",
                ) => {
                    let value = args
                        .next()
                        .ok_or_else(|| format!("参数 {} 缺少值。", arg.to_string_lossy()))?;
                    if value.is_empty() {
                        return Err(format!("参数 {} 不能为空。", arg.to_string_lossy()));
                    }
                    match arg.to_str().unwrap() {
                        "--assets" => options.assets = Some(value.into()),
                        "--savedata-root" => options.savedata_root = Some(value.into()),
                        "--steam-dir" => options.steam_dir = Some(value.into()),
                        "--source-dir" => options.source_dir = Some(value.into()),
                        _ => {
                            let id = value.into_string().map_err(|_| "账号必须是数字。")?;
                            if !id.bytes().all(|b| b.is_ascii_digit()) {
                                return Err("账号必须是存档目录的纯数字名称。".into());
                            }
                            options.account = Some(id);
                        }
                    }
                }
                _ => {
                    return Err(format!(
                        "未知参数：{}。使用 --help 查看说明。",
                        arg.to_string_lossy()
                    ))
                }
            }
        }
        Ok(options)
    }
}

fn main() {
    enable_utf8_console();
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let interactive = !args
        .iter()
        .any(|a| a == "--check" || a == "--preview-confirmation" || a == "--help" || a == "-h");
    let result = Options::parse(args).and_then(run);
    if let Err(error) = &result {
        eprintln!("\n操作已停止：{error}");
    }
    if interactive && io::stdin().is_terminal() {
        let _ = read_line("\n按回车键退出程序...");
    }
    if result.is_err() {
        std::process::exit(1);
    }
}

fn run(options: Options) -> Result<()> {
    if options.help {
        println!(
            "仁王 3 存档改签助手\n\
             用法：nioh3-save-manager.exe [选项]\n\n\
             --check                 只检查路径和资源，不关闭 Steam、不修改任何文件\n\
             --preview-confirmation  仅预览确认弹窗，关闭后退出，不执行存档操作\n\
             --assets <目录>         image.png 和存档资源目录所在的位置\n\
             --savedata-root <目录>  包含各账号数字子目录的 Savedata 根目录\n\
             --account <数字目录名>  明确指定已有账号；否则自动发现并在多账号时选择\n\
             --source-dir <目录>     使用自定义存档；目录内需包含 SYSTEMSAVEDATA00 和 SAVEDATA00\n\
             --steam-dir <目录>      Steam 安装目录（自动查找失败时使用）\n\
             --help, -h              显示帮助\n\n\
             默认存档位置：%LOCALAPPDATA%\\KoeiTecmo\\NIOH3\\Savedata\n\
             程序使用外部改签工具；每轮改签后须确认成功，才会继续。"
        );
        return Ok(());
    }
    let asset_root = options.assets.map_or_else(paths::find_asset_root, Ok)?;
    let asset_root = std::path::absolute(&asset_root)
        .map_err(|e| format!("无法解析资源路径 {}：{e}", asset_root.display()))?;
    if options.preview_confirmation {
        let ready = confirmation::show(&asset_root.join("image.png"))?;
        println!(
            "确认弹窗预览结束：{}。未执行存档操作。",
            if ready { "已确认" } else { "已取消" }
        );
        return Ok(());
    }
    let savedata_root = options
        .savedata_root
        .map_or_else(paths::default_savedata_root, Ok)?;
    let savedata_root = std::path::absolute(&savedata_root)
        .map_err(|e| format!("无法解析存档路径 {}：{e}", savedata_root.display()))?;
    println!("仁王 3 存档改签助手");
    println!("资源目录：{}", asset_root.display());
    println!("存档根目录：{}", savedata_root.display());
    if !options.check && !confirmation::show(&asset_root.join("image.png"))? {
        println!("已取消，未修改存档。");
        return Ok(());
    }
    let accounts = paths::discover_accounts(&savedata_root)?;
    if accounts.is_empty() {
        return Err(format!(
            "在 {} 中没有发现数字账号目录。请先用目标账号启动游戏并创建存档，或使用 --savedata-root 指定实际位置。",
            savedata_root.display()
        ));
    }
    if options.check {
        return check_paths(
            &asset_root,
            &accounts,
            options.account.as_deref(),
            options.source_dir.as_deref(),
        );
    }
    if !cfg!(windows) {
        return Err("交互改签流程仅支持 Windows。".into());
    }
    let Some(account) = select_account(&accounts, options.account.as_deref())? else {
        println!("已取消，未修改存档。");
        return Ok(());
    };
    let kind = match &options.source_dir {
        Some(directory) => custom_save_kind(directory)?,
        None => {
            let Some(kind) = select_save_kind(&asset_root)? else {
                println!("已取消，未修改存档。");
                return Ok(());
            };
            kind
        }
    };
    let plan = SavePlan::new(&asset_root, &account.path, kind.clone())?;
    println!("\n选中账号：{}", account.id);
    println!("目标目录：{}", account.path.display());
    println!("存档类型：{}", kind.label());
    if !confirm("将关闭 Steam、备份原存档并进行两轮改签。继续？[y/N] ")? {
        println!("已取消，未修改存档。");
        return Ok(());
    }
    let backup_dir = create_backup_dir(&asset_root, &account.id)?;
    println!("本次备份目录：{}", backup_dir.display());
    if !close_steam()? {
        println!("已取消，未修改存档。");
        return Ok(());
    }
    let cloud_result = options
        .steam_dir
        .map_or_else(steam::find_steam_dir, |p| Ok(Some(p)))
        .and_then(|p| p.ok_or_else(|| "未找到 Steam 安装目录。".to_string()))
        .and_then(|p| steam::config_path(&p, &account.id))
        .and_then(|p| steam::disable_cloud_in_config(&p, "4198760", &backup_dir));
    match cloud_result {
        Ok(()) => println!("已备份并关闭所选账号的游戏云存档配置。"),
        Err(error) => {
            println!("无法自动关闭云存档：{error}");
            if !confirm("请手动关闭该游戏的 Steam 云同步，并退出 Steam。已完成？[y/N] ")?
            {
                println!("已取消，未修改游戏存档。");
                return Ok(());
            }
            if !close_steam()? {
                println!("已取消，未修改游戏存档。");
                return Ok(());
            }
        }
    }
    plan.execute(&backup_dir, |exe, tool_dir, stage| {
        if steam_is_running()? {
            return Err("检测到 Steam 又已启动，请退出后重新操作。".into());
        }
        println!("\n正在处理：{stage}");
        println!("下方将实时显示改签工具输出；出现按任意键继续的提示时，请按键继续。");
        println!("----- 改签工具输出开始 -----");
        let status = signer::run(exe, tool_dir)?;
        println!("\n----- 改签工具输出结束 -----");
        if !status.success() {
            return Err(format!("改签工具异常退出（{status}），未写回本地存档。"));
        }
        if !confirm("工具是否明确提示“已为你改写签名”或改签成功？[y/N] ")?
        {
            return Err("本轮改签未确认成功，已取消写回本地存档。".into());
        }
        if steam_is_running()? {
            return Err("检测到 Steam 已启动，已取消写回本地存档。".into());
        }
        println!("本轮已确认成功：{stage}。两轮均完成后才会写回本地存档。");
        Ok(())
    })?;
    println!("\n两轮改签和存档替换均已完成。");
    println!("原存档备份：{}", backup_dir.join("local").display());
    println!("请启动 Steam 并进入游戏，保留备份直至确认存档正常。");
    Ok(())
}

fn check_paths(
    root: &Path,
    accounts: &[Account],
    requested: Option<&str>,
    source_dir: Option<&Path>,
) -> Result<()> {
    println!("\n只读检查：不会启动改签工具或修改 Steam/存档。");
    let candidates: Vec<_> = match requested {
        Some(id) => vec![find_account(accounts, id)?],
        None => accounts.iter().collect(),
    };
    let kinds = match source_dir {
        Some(directory) => vec![custom_save_kind(directory)?],
        None => vec![SaveKind::Hand, SaveKind::Magic],
    };
    let mut failed = false;
    for account in candidates {
        println!("\n账号 {}：{}", account.id, account.path.display());
        for kind in &kinds {
            match SavePlan::new(root, &account.path, kind.clone()) {
                Ok(_) => println!("  {}：路径检查通过", kind.label()),
                Err(error) => {
                    println!("  {}：{error}", kind.label());
                    failed = true;
                }
            }
        }
    }
    if failed {
        Err("部分路径检查未通过，请按上方具体路径处理。".into())
    } else {
        println!("\n路径检查通过。此检查不验证文件写权限、云同步状态或实际改签结果。");
        Ok(())
    }
}

fn find_account<'a>(accounts: &'a [Account], id: &str) -> Result<&'a Account> {
    accounts
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| format!("未发现账号目录 {id}。请从实际列出的数字目录中选择。"))
}

fn select_account(accounts: &[Account], requested: Option<&str>) -> Result<Option<Account>> {
    if let Some(id) = requested {
        return find_account(accounts, id).map(|a| Some(a.clone()));
    }
    if accounts.len() == 1 {
        println!("\n自动发现唯一账号：{}", accounts[0].id);
        return Ok(Some(accounts[0].clone()));
    }
    println!("\n发现多个账号，请明确选择要替换的账号：");
    for (index, account) in accounts.iter().enumerate() {
        let state = if account.has_system_save() {
            "已有系统存档"
        } else {
            "缺少系统存档，需先在游戏中创建"
        };
        println!(
            "{}. {}（{}）\n   {}",
            index + 1,
            account.id,
            state,
            account.path.display()
        );
    }
    loop {
        let value = read_line("请输入账号序号（q 或直接回车取消）：")?;
        if cancelled(&value) {
            return Ok(None);
        }
        if let Some(index) = selection_index(&value, accounts.len()) {
            return Ok(Some(accounts[index].clone()));
        }
        println!("无效序号，请重新选择。");
    }
}

fn select_save_kind(asset_root: &Path) -> Result<Option<SaveKind>> {
    println!("\n1. 手搓存档（原作者推荐）\n2. 魔改存档\n3. 自定义存档（使用你自己准备的存档）");
    loop {
        let value = read_line("请选择 1、2 或 3（q 或直接回车取消）：")?;
        match value.as_str() {
            "1" => return Ok(Some(SaveKind::Hand)),
            "2" => return Ok(Some(SaveKind::Magic)),
            "3" => return select_custom_save_kind(asset_root),
            _ if cancelled(&value) => return Ok(None),
            _ => println!("请输入 1、2 或 3。"),
        }
    }
}

fn select_custom_save_kind(asset_root: &Path) -> Result<Option<SaveKind>> {
    let default_dir = asset_root.join("CustomSave");
    println!(
        "\n自定义存档需要包含 SYSTEMSAVEDATA00 和 SAVEDATA00 两个文件夹，以及各自的 SAVEDATA.BIN。"
    );
    println!("默认存放位置：{}", default_dir.display());
    loop {
        let value =
            read_line("请输入存档文件夹路径（可直接拖入文件夹；直接回车使用默认位置；q 取消）：")?;
        if value.eq_ignore_ascii_case("q") {
            return Ok(None);
        }
        let entered = strip_quotes(&value);
        let candidate = if entered.is_empty() {
            default_dir.clone()
        } else {
            PathBuf::from(entered)
        };
        let candidate = std::path::absolute(&candidate)
            .map_err(|e| format!("无法解析路径 {}：{e}", candidate.display()))?;
        match SavePlan::validate_source(&candidate) {
            Ok(()) => return Ok(Some(SaveKind::Custom(candidate))),
            Err(error) => {
                println!("此文件夹不能用作自定义存档：{error}\n请放入正确的存档文件，或输入其他文件夹路径。")
            }
        }
    }
}

fn custom_save_kind(directory: &Path) -> Result<SaveKind> {
    let directory = std::path::absolute(directory)
        .map_err(|e| format!("无法解析自定义存档路径 {}：{e}", directory.display()))?;
    SavePlan::validate_source(&directory)
        .map_err(|error| format!("自定义存档目录不可用：{error}"))?;
    Ok(SaveKind::Custom(directory))
}

fn strip_quotes(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|inner| inner.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout()
        .flush()
        .map_err(|e| format!("无法显示提示：{e}"))?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("无法读取输入：{e}"))?;
    Ok(line.trim().to_string())
}

fn confirm(prompt: &str) -> Result<bool> {
    Ok(read_line(prompt)?.eq_ignore_ascii_case("y"))
}

fn cancelled(value: &str) -> bool {
    value.is_empty() || value.eq_ignore_ascii_case("q") || value.eq_ignore_ascii_case("n")
}

fn selection_index(value: &str, count: usize) -> Option<usize> {
    value
        .parse::<usize>()
        .ok()?
        .checked_sub(1)
        .filter(|i| *i < count)
}

fn create_backup_dir(root: &Path, account: &str) -> Result<PathBuf> {
    let parent = root.join("backups").join(account);
    fs::create_dir_all(&parent)
        .map_err(|e| format!("无法创建备份目录 {}：{e}", parent.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    for counter in 0..1000 {
        let path = parent.join(format!("{timestamp}-{}-{counter}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("无法创建备份目录 {}：{e}", path.display())),
        }
    }
    Err("无法分配独立备份目录，请稍后重试。".into())
}

#[cfg(windows)]
fn steam_is_running() -> Result<bool> {
    let output = Command::new("tasklist.exe")
        .args(["/FI", "IMAGENAME eq steam.exe", "/FO", "CSV", "/NH"])
        .output()
        .map_err(|e| format!("无法检查 Steam 进程：{e}"))?;
    if !output.status.success() {
        return Err(format!("检查 Steam 进程失败（{}）。", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.to_ascii_lowercase().starts_with("\"steam.exe\",")))
}

#[cfg(not(windows))]
fn steam_is_running() -> Result<bool> {
    Err("检查 Steam 进程仅支持 Windows。".into())
}

fn close_steam() -> Result<bool> {
    if !steam_is_running()? {
        return Ok(true);
    }
    println!("\n正在请求 Steam 退出...");
    let _ = Command::new("taskkill.exe")
        .args(["/IM", "steam.exe"])
        .output();
    for _ in 0..10 {
        if !steam_is_running()? {
            return Ok(true);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    while steam_is_running()? {
        println!("Steam 仍在运行。请从 Steam 菜单选择退出。");
        if !confirm("退出后输入 y 重试，回车取消：[y/N] ")? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(windows)]
fn enable_utf8_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCP(code_page: u32) -> i32;
        fn SetConsoleOutputCP(code_page: u32) -> i32;
    }
    // Console code pages are shared with attached processes. The legacy signer
    // writes GBK, so signer::run transcodes its piped output before displaying it.
    unsafe {
        SetConsoleCP(65001);
        SetConsoleOutputCP(65001);
    }
}

#[cfg(not(windows))]
fn enable_utf8_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options> {
        Options::parse(args.iter().map(OsString::from))
    }

    #[test]
    fn parses_read_only_check_with_custom_paths_and_account() {
        let options = parse(&[
            "--check",
            "--assets",
            "E:\\中文 资源",
            "--savedata-root",
            "D:\\Savedata",
            "--account",
            "76561199999999999",
        ])
        .unwrap();
        assert!(options.check);
        assert_eq!(options.assets.unwrap(), PathBuf::from("E:\\中文 资源"));
        assert_eq!(options.account.as_deref(), Some("76561199999999999"));
    }

    #[test]
    fn parses_custom_source_directory() {
        let options = parse(&["--source-dir", "D:\\我的 存档"]).unwrap();
        assert_eq!(options.source_dir.unwrap(), PathBuf::from("D:\\我的 存档"));
        assert!(parse(&["--source-dir"]).is_err());
        assert!(parse(&["--source-dir", ""]).is_err());
    }

    #[test]
    fn strips_quotes_from_dragged_or_pasted_paths() {
        assert_eq!(strip_quotes("\"D:\\我的 存档\""), "D:\\我的 存档");
        assert_eq!(strip_quotes("'D:\\我的 存档'"), "D:\\我的 存档");
        assert_eq!(strip_quotes("D:\\我的 存档"), "D:\\我的 存档");
        assert_eq!(strip_quotes(""), "");
        assert_eq!(strip_quotes("\""), "\"");
    }

    #[test]
    fn rejects_unknown_missing_or_unsafe_account_arguments() {
        assert!(parse(&["--unknown"]).is_err());
        assert!(parse(&["--assets"]).is_err());
        assert!(parse(&["--account", ".."]).is_err());
        assert!(parse(&["--account", ""]).is_err());
    }

    #[test]
    fn account_selection_requires_an_existing_exact_match() {
        let accounts = vec![
            Account {
                id: "123".into(),
                path: "a".into(),
            },
            Account {
                id: "456".into(),
                path: "b".into(),
            },
        ];
        assert_eq!(
            find_account(&accounts, "456").unwrap().path,
            PathBuf::from("b")
        );
        assert!(find_account(&accounts, "789").is_err());
    }

    #[test]
    fn invalid_or_cancelled_selection_never_defaults_to_an_account() {
        assert_eq!(selection_index("2", 2), Some(1));
        for value in ["", "0", "3", "q", "-1"] {
            assert_eq!(selection_index(value, 2), None);
        }
        assert!(cancelled(""));
        assert!(cancelled("Q"));
        assert!(cancelled("n"));
        assert!(!cancelled("1"));
    }
}
