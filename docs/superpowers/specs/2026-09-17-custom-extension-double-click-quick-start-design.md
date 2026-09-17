# Custom `.mfz` Extension and Double-Click Quick-Start Design Spec

## Overview
This specification defines the design and implementation for custom file extension (`.mfz`) association and double-click quick-start functionality in MFZ.
It allows users to quickly launch remote server sessions (e.g. STG and PRD) by double-clicking desktop or folder files (`stg.mfz`, `prd.mfz`), without manually typing CLI commands.

---

## 1. Windows Registry Shell Association

### 1.1 Architecture
File association is registered in the Current User registry hive (`HKEY_CURRENT_USER\Software\Classes`).
This requires **zero administrative elevation** (no UAC prompt required) and is isolated to the logged-in user.

### 1.2 Registry Keys & Structure
```text
HKCU\Software\Classes\.mfz
  (Default) = "mfz.session"

HKCU\Software\Classes\mfz.session
  (Default) = "MFZ Remote Desktop Profile"
  DefaultIcon
    (Default) = "<path-to-mfz.exe>" (or "<path-to-icon.ico>")
  shell\open\command
    (Default) = "\"<path-to-mfz.exe>\" launch \"%1\""
```

### 1.3 Windows API / Command Implementation
Registration and unregistration can be executed via Windows `reg` CLI or standard Win32 registry APIs:
- Registration:
  - `reg add "HKCU\Software\Classes\.mfz" /ve /d "mfz.session" /f`
  - `reg add "HKCU\Software\Classes\mfz.session" /ve /d "MFZ Remote Desktop Profile" /f`
  - `reg add "HKCU\Software\Classes\mfz.session\DefaultIcon" /ve /d "<exe-or-ico-path>" /f`
  - `reg add "HKCU\Software\Classes\mfz.session\shell\open\command" /ve /d "\"<exe-path>\" launch \"%1\"" /f`
- Unregistration:
  - `reg delete "HKCU\Software\Classes\.mfz" /f`
  - `reg delete "HKCU\Software\Classes\mfz.session" /f`

---

## 2. `.mfz` File Formats & Parsing

### 2.1 File Resolution in `mfz launch`
When `mfz launch <arg>` receives an argument ending in `.mfz` or referencing an existing file:
1. **Pointer File**:
   Contains `env = <name>` or `environment = <name>`.
   MFZ resolves `<name>` from the configuration file (`mfz.ini`).
2. **Filename Fallback**:
   If the file is 0-bytes or contains no recognized keys, MFZ extracts the file stem (e.g. `stg.mfz` -> `stg`) and loads `[stg]` from `mfz.ini`.
3. **Standalone Portable Profile**:
   If the file contains standard configuration keys (such as `profile = ...`, `instance = ...`, `rdp_host = ...`), MFZ loads it directly as an independent `EnvironmentConfig`.

---

## 3. CLI Commands

### 3.1 `mfz register` & `mfz unregister`
- `mfz register`: Registers `.mfz` file extension in the Windows Shell for the current user.
- `mfz unregister`: Cleans up the `.mfz` registry entries.

### 3.2 `mfz export`
- `mfz export <environment>`:
  - Generates `<environment>.mfz` in the current directory (or destination specified by `--out`).
- `mfz export --all`:
  - Generates `.mfz` files for all configured environments in `mfz.ini`.
- Options:
  - `-d, --desktop`: Writes generated `.mfz` files directly to the user's Desktop (`C:\Users\<user>\Desktop`).
  - `-s, --standalone`: Exports a self-contained portable configuration file instead of a pointer file.
  - `-o, --out <path>`: Custom destination file or directory path.

---

## 4. Multi-Profile User Workflow (`stg` and `prd`)
1. User runs `mfz register`.
2. User runs `mfz export --all --desktop`.
3. Two files appear on Desktop:
   - `stg.mfz`
   - `prd.mfz`
4. Double-clicking `stg.mfz` connects directly to STG.
5. Double-clicking `prd.mfz` connects directly to PRD.

---

## 5. Verification Plan
- Unit tests:
  - Parsing `.mfz` pointer files (`env = stg`).
  - Parsing `.mfz` standalone portable profiles.
  - Parsing empty `.mfz` files by stem fallback.
- CLI verification:
  - Test `mfz export stg` creates a valid `stg.mfz`.
  - Test `mfz launch stg.mfz` executes the `stg` profile.
  - Test `mfz register` creates valid HKCU registry keys.
