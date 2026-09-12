# Repository Guidelines

## Project Structure & Module Organization

- `main.py` contains the Windows-only workflow: Tkinter confirmation, Steam Cloud configuration, external signing-tool execution, and save replacement.
- `image.png` is the confirmation-screen reference image.
- `HandMakeSave/` and `MagicMakeSave/` contain alternative save sets, each with `SAVEDATA00/SAVEDATA.BIN` and `SYSTEMSAVEDATA00/SAVEDATA.BIN`.
- `Nioh3SaveCertificateTool/` contains the signing executable and the `别人存档扔里面/` and `你的存档扔里面/` staging directories. Preserve these names; code references them directly.
- `requirements.txt` lists Pillow and PyInstaller. No separate source package, test directory, or CI configuration exists.

## Build, Test, and Development Commands

Run commands from the repository root in PowerShell. Use Miniconda's `base` environment for all Python and package operations:

```powershell
conda activate base
python -m pip install -r requirements.txt
python -m py_compile main.py
python main.py
```

Installation provides dependencies; `py_compile` checks syntax without launching the workflow; `main.py` starts the interactive application.

There is no checked-in build script or PyInstaller specification. An example console-enabled build is:

```powershell
python -m PyInstaller --onefile --name "双击我" main.py
```

Place `dist/双击我.exe` beside `image.png` and the three asset directories before running. Keep console support because the application uses `input()`.

## Coding Style & Naming Conventions

Use four-space indentation, `snake_case` functions and variables, and `UPPER_SNAKE_CASE` module constants. Preserve the existing Chinese interface text and UTF-8 encoding. Construct paths with `os.path.join` and retain script/executable compatibility through `get_script_dir()`. Keep changes focused; no formatter or linter is configured.

## Testing Guidelines

No automated framework or coverage threshold is configured. Run the syntax check above for Python changes. For new automated tests, use standard-library `unittest`, name files `tests/test_*.py`, and run `python -m unittest discover -s tests`. Use temporary save directories and mock subprocess calls.

For manual checks, cover cancellation, both save choices, missing files, and signing-tool completion using disposable save copies.

## Commit & Pull Request Guidelines

History contains one commit, `完成基本功能`; no formal convention is established. Write short, descriptive commit subjects. PRs should explain behavior changes, validation performed, and affected paths; link relevant issues and include screenshots for GUI changes. Exclude generated builds and unintended binary-save changes.

## Configuration & Save Safety

Review the hardcoded account directory in `NIOH3_LOCAL_BASE` before local execution. The workflow force-closes Steam, edits `sharedconfig.vdf`, and overwrites saves without automatic backups. Back up configuration and saves before manual testing; never use the full workflow as an automated smoke test.
