# Codex Windows x64 npm overlay

This archive replaces the native executable used by an existing global
`@openai/codex` npm installation. It is intentionally Windows x64 only.

`npm install -g @openai/codex` installs a JavaScript launcher plus an optional
platform package. On Windows x64 the launcher ultimately runs:

```text
@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe
```

Depending on npm's dependency layout, the platform package may instead be
nested below `@openai\codex\node_modules`. The installer detects both layouts.
It also installs the dedicated helper at:

```text
vendor\x86_64-pc-windows-msvc\codex-path\apply_patch.exe
```

## Install

1. Install the matching official package first:

   ```powershell
   npm install -g @openai/codex@0.150.1
   ```

2. Close all running Codex processes.
3. Extract this ZIP, open PowerShell 7 in the extracted directory, and run:

   ```powershell
   pwsh -File .\install.ps1
   ```

The installer verifies the archive checksums, checks the installed npm version,
backs up replaced files below
`$env:USERPROFILE\.codex\backups\windows-npm-overlay`, installs both native
executables, and runs `codex.exe --version`.

Use `-NpmRoot <path>` when the npm global root cannot be detected. Use `-Force`
only to override a version mismatch; it never overrides the running-process
safety check.

## Restore

To restore the most recent backup:

```powershell
pwsh -File .\restore.ps1
```

The install command also prints an exact restore command for its backup. A
later `npm install -g @openai/codex` may overwrite the overlay; reinstall the
matching overlay after npm upgrades.

## File behavior in this build

- Exact and trailing-whitespace matching preserve leading indentation.
- UTF-8 BOM and existing CRLF/LF line endings are preserved by default.
- UTF-16 and other non-UTF-8 files fail with an explicit error and remain
  unchanged.
- Patches that make no byte-level change fail instead of reporting success.
- The native `apply_patch.exe` accepts UTF-8 patches on stdin, avoiding the
  Windows command-line length limit.
