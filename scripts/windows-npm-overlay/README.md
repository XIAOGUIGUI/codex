# Codex Windows x64 npm patch

This unofficial community package replaces the native executable used by an
existing global `@openai/codex` npm installation. It is intentionally Windows
x64 only and is not published or supported by OpenAI.

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

## Install from npm

Install the matching official package and patch version globally:

```powershell
npm install -g @openai/codex@0.153.4 @chenronggui/codex-win-patch@0.153.4-patch.1
```

The npm `postinstall` hook applies the patch automatically. It prefers
PowerShell 7 (`pwsh.exe`) and falls back to Windows PowerShell 5.1
(`powershell.exe`). Set `CODEX_WINDOWS_PATCH_POWERSHELL` to an executable path
to select one explicitly.

After an official Codex reinstall or upgrade, reapply the matching patch with:

```powershell
codex-win-patch-install
```

If npm lifecycle scripts were disabled with `--ignore-scripts`, use the same
command to perform the installation explicitly.

## Install from a workflow artifact

Download the npm package artifact from the `Windows custom Codex overlay`
workflow, extract it, and install the `.tgz` file:

```powershell
npm install -g @openai/codex@0.153.4
npm install -g C:\Downloads\chenronggui-codex-win-patch-0.153.4-patch.1.tgz
```

## Install from the ZIP overlay

1. Install the matching official package first:

   ```powershell
   npm install -g @openai/codex@0.153.4
   ```

2. Close all running Codex processes.
3. Extract this ZIP and run it with PowerShell 7:

   ```powershell
   pwsh -File .\install.ps1
   ```

   Or use the built-in Windows PowerShell 5.1:

   ```powershell
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
   ```

The installer verifies the archive checksums, checks the installed npm version,
backs up replaced files below
`$env:USERPROFILE\.codex\backups\windows-npm-overlay`, installs both native
executables, and runs `codex.exe --version`.

Use `-NpmRoot <path>` when the npm global root cannot be detected. Use `-Force`
only to override a version mismatch; it never overrides the running-process
safety check.

## Restore

For an npm installation, restore the most recent backup with:

```powershell
codex-win-patch-restore
```

From the extracted ZIP, run:

```powershell
pwsh -File .\restore.ps1
```

The install command also prints an exact restore command for its backup. A
later `npm install -g @openai/codex` may overwrite the overlay; reinstall the
matching overlay after npm upgrades.

## Publishing

The `Windows custom Codex overlay` workflow is manual-only. Its `publish_npm`
input defaults to `false`, so a normal run builds and tests the ZIP and `.tgz`
artifacts without publishing anything. Publishing requires manually selecting
`publish_npm: true`.

For the first npm release, publish the downloaded `.tgz` once from an
authenticated workstation:

```powershell
npm publish .\chenronggui-codex-win-patch-0.153.4-patch.1.tgz --access public
```

After the package exists, configure npm Trusted Publishing for repository
`XIAOGUIGUI/codex` and workflow `windows-custom-build.yml`. Later releases can
then use the workflow's publish option without a long-lived npm token.

## File behavior in this build

- Exact and trailing-whitespace matching preserve leading indentation.
- UTF-8 BOM and existing CRLF/LF line endings are preserved by default.
- UTF-16 and other non-UTF-8 files fail with an explicit error and remain
  unchanged.
- Patches that make no byte-level change fail instead of reporting success.
- The native `apply_patch.exe` accepts UTF-8 patches on stdin, avoiding the
  Windows command-line length limit.
