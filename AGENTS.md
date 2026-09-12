# Repository Guidelines

Windows-only Chinese tool 仁王 3 存档改签助手 (Nioh 3 save re-signing assistant), rewritten from Python to Rust. `README.md` is the user-facing Chinese doc; update it when behavior changes.

## Build, Test, and Development Commands

```powershell
cargo fmt --check
cargo test --offline --locked
cargo build --release --offline --locked
.\build.ps1 -Test   # tests, then fills dist/ and updates root 双击我.exe
```

- Rust 1.85+ / edition 2021. There are zero external crates (empty `Cargo.lock`); Win32 APIs are called via raw `extern "system"` FFI. Do not add dependencies casually.
- `build.ps1` prefers the portable `.build-tools/rustup/toolchains/1.85.0-x86_64-pc-windows-gnu`, otherwise the configured Cargo; it never installs or modifies system toolchains.
- Safe verification that never touches saves: `cargo run -- --check` (read-only, exit 1 on failure) and `cargo run -- --preview-confirmation` (dialog only). Asset discovery walks up to 5 parent directories of the exe plus cwd, so running from `target/debug` works.
- Never use the full interactive workflow as a smoke test: it force-closes Steam, edits `sharedconfig.vdf`, and overwrites real saves. Manual checks need disposable save copies.

## Layout and Platform Quirks

- `src/main.rs` CLI parsing, Chinese prompts, Steam process control, backup dir allocation, console UTF-8 code page. Custom saves come from menu option 3 (`<assets>/CustomSave`, Enter accepts the default, quoted drag-and-drop paths are stripped) or `--source-dir`; `--check --source-dir` validates that folder instead of the two bundled ones.
- `src/confirmation.rs` native Win32 modal dialog with GDI+ image rendering (`gdiplus.dll` loaded dynamically).
- `src/signer.rs` runs the bundled signer with inherited stdin (it prompts/`pause`s) and transcodes its GBK/CP936 stdout+stderr to UTF-8. Do not replace this with console code-page changes.
- `src/steam.rs` SteamID64 → account mapping (base `76561197960265728`), format-preserving VDF editing of `sharedconfig.vdf`.
- `src/workflow.rs` preflight, backups, two signing rounds, writeback, rollback, tool lock. `SaveKind::Custom(PathBuf)` reuses the bundled pipeline with a player-supplied source directory; `SavePlan::validate_source` is the shared source-dir check.
- `src/paths.rs` numeric account discovery, asset root discovery, `LOCALAPPDATA`/`USERPROFILE` fallback.
- Chinese names are load-bearing: `HandMakeSave`, `MagicMakeSave`, `Nioh3SaveCertificateTool`, `双击我改签v0.5.exe`, `你的存档扔里面`, `别人存档扔里面`. Do not rename.
- Windows behavior is `#[cfg(windows)]`; non-Windows stubs return Chinese errors. `src/lib.rs` defines `Result<T> = std::result::Result<T, String>`; messages and test assertions are Chinese.
- Nioh 3 Steam AppID `4198760` is hardcoded in `main.rs`.

## Workflow Invariants (tests pin these)

- Two rounds in order: `SYSTEMSAVEDATA00`, then `SAVEDATA00`. Before each round the user's original local SYSTEM save is restored as the signer identity, even if the signer changed the staging copy.
- Local saves are written only after both rounds are confirmed and their bytes are unchanged during signing; any change aborts writeback and preserves the newer file.
- Bundled staging files are backed up and restored afterwards; staging directories created by the run are removed.
- Backups go to `backups/<account>/<timestamp>-<pid>-<n>/` and never overwrite existing files (`create_new`).
- `.nioh3-save-manager.lock` with Windows `share_mode(0)` enforces one run per tool dir; a stale lock file is harmless.
- File replacements write a temp file then rename; Steam config is backed up before the first edit.

## Testing

- Tests are inline `#[cfg(test)]` modules using temp directories and fake signer callbacks; never launch the real signer or touch real saves. Single test: `cargo test --offline --locked <name>`.
- Manual checks must cover cancel, both bundled save choices plus the custom folder, missing files, and signer completion.

## Repo Notes

- Generated/ignored: `target/`, `dist/`, `backups/`, `.build-tools/`, root `双击我.exe`, `.nioh3-save-manager.lock`.
- Keep commit subjects short and descriptive; history is a mix of Chinese and English.
