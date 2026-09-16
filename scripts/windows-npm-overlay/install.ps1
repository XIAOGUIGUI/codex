[CmdletBinding()]
param(
    [string]$NpmRoot,
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$TargetTriple = "x86_64-pc-windows-msvc"
$PlatformPackageName = "codex-win32-x64"
$PayloadDirectory = $PSScriptRoot

function Write-Step {
    param([string]$Message)

    Write-Host "==> $Message"
}

function Get-Sha256 {
    param([string]$Path)

    $stream = [IO.File]::OpenRead($Path)
    try {
        $sha256 = [Security.Cryptography.SHA256]::Create()
        try {
            $hash = $sha256.ComputeHash($stream)
            return [BitConverter]::ToString($hash).Replace("-", "").ToLowerInvariant()
        } finally {
            $sha256.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Assert-PayloadHashes {
    $checksumPath = Join-Path $PayloadDirectory "SHA256SUMS"
    if (-not (Test-Path -LiteralPath $checksumPath -PathType Leaf)) {
        throw "Missing checksum file: $checksumPath"
    }

    $expected = @{}
    foreach ($line in Get-Content -LiteralPath $checksumPath) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }
        if ($line -notmatch '^([0-9a-fA-F]{64})\s+\*?(.+)$') {
            throw "Invalid SHA256SUMS line: $line"
        }
        $expected[$Matches[2].Trim()] = $Matches[1].ToLowerInvariant()
    }

    foreach ($fileName in @("codex.exe", "apply_patch.exe")) {
        $sourcePath = Join-Path $PayloadDirectory $fileName
        if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
            throw "Missing payload file: $sourcePath"
        }
        if (-not $expected.ContainsKey($fileName)) {
            throw "SHA256SUMS does not contain $fileName."
        }
        $actual = Get-Sha256 $sourcePath
        if ($actual -ne $expected[$fileName]) {
            throw "SHA-256 mismatch for ${fileName}: expected $($expected[$fileName]), got $actual."
        }
    }
}

function Resolve-GlobalNpmRoot {
    if (-not [string]::IsNullOrWhiteSpace($NpmRoot)) {
        if (-not (Test-Path -LiteralPath $NpmRoot -PathType Container)) {
            throw "The npm root does not exist: $NpmRoot"
        }
        return (Resolve-Path -LiteralPath $NpmRoot).Path
    }

    $detectedRoot = (& npm root -g 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($detectedRoot)) {
        throw "Could not locate the global npm root. Install npm or pass -NpmRoot explicitly."
    }
    if (-not (Test-Path -LiteralPath $detectedRoot -PathType Container)) {
        throw "npm reported a global root that does not exist: $detectedRoot"
    }
    return (Resolve-Path -LiteralPath $detectedRoot).Path
}

function Resolve-CodexPackageDirectory {
    param([string]$GlobalNpmRoot)

    $candidates = @(
        (Join-Path $GlobalNpmRoot "@openai\$PlatformPackageName\vendor\$TargetTriple"),
        (Join-Path $GlobalNpmRoot "@openai\codex\node_modules\@openai\$PlatformPackageName\vendor\$TargetTriple"),
        (Join-Path $GlobalNpmRoot "@openai\codex\vendor\$TargetTriple")
    )

    foreach ($candidate in $candidates) {
        $manifest = Join-Path $candidate "codex-package.json"
        $entrypoint = Join-Path $candidate "bin\codex.exe"
        if ((Test-Path -LiteralPath $manifest -PathType Leaf) -and
            (Test-Path -LiteralPath $entrypoint -PathType Leaf)) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    $formattedCandidates = $candidates -join [Environment]::NewLine
    throw "Could not find the Windows x64 Codex platform package under:`n$formattedCandidates`nRun 'npm install -g @openai/codex' first."
}

function Assert-CodexNotRunning {
    param([string]$CodexPath)

    $targetPath = [IO.Path]::GetFullPath($CodexPath)
    $matchingProcesses = @()
    foreach ($process in Get-Process -Name "codex" -ErrorAction SilentlyContinue) {
        try {
            if ([IO.Path]::GetFullPath($process.Path) -eq $targetPath) {
                $matchingProcesses += $process
            }
        } catch {
            # Access to another process path can be denied. The file copy will
            # still fail safely if that process has the target executable open.
        }
    }
    if ($matchingProcesses.Count -gt 0) {
        $processIds = ($matchingProcesses.Id -join ", ")
        throw "Codex is still running from the target package (PID: $processIds). Close it and rerun the installer."
    }
}

function Copy-FileReplacing {
    param(
        [string]$Source,
        [string]$Destination
    )

    [IO.File]::Copy($Source, $Destination, $true)
}

function Resolve-BackupRoot {
    $configuredRoot = [Environment]::GetEnvironmentVariable("CODEX_WINDOWS_PATCH_BACKUP_ROOT")
    if (-not [string]::IsNullOrWhiteSpace($configuredRoot)) {
        return [IO.Path]::GetFullPath($configuredRoot)
    }

    $userProfileDirectory = [Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile)
    return Join-Path $userProfileDirectory ".codex\backups\windows-npm-overlay"
}

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw "This overlay supports Windows only."
}
if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne
    [Runtime.InteropServices.Architecture]::X64) {
    throw "This overlay supports Windows x64 only."
}

Write-Step "Verifying overlay payload"
Assert-PayloadHashes

$globalNpmRoot = Resolve-GlobalNpmRoot
$packageDirectory = Resolve-CodexPackageDirectory $globalNpmRoot
$packageManifestPath = Join-Path $packageDirectory "codex-package.json"
$packageManifest = Get-Content -LiteralPath $packageManifestPath -Raw | ConvertFrom-Json
$buildInfoPath = Join-Path $PayloadDirectory "build-info.json"
if (Test-Path -LiteralPath $buildInfoPath -PathType Leaf) {
    $buildInfo = Get-Content -LiteralPath $buildInfoPath -Raw | ConvertFrom-Json
    if (-not [string]::IsNullOrWhiteSpace([string]$buildInfo.version) -and
        [string]$packageManifest.version -ne [string]$buildInfo.version -and
        -not $Force) {
        throw "Version mismatch: npm package is $($packageManifest.version), overlay is $($buildInfo.version). Install the matching @openai/codex version or rerun with -Force."
    }
}

$codexTarget = Join-Path $packageDirectory "bin\codex.exe"
$pathDirectory = Join-Path $packageDirectory "codex-path"
$applyPatchTarget = Join-Path $pathDirectory "apply_patch.exe"
$codexSource = Join-Path $PayloadDirectory "codex.exe"
$applyPatchSource = Join-Path $PayloadDirectory "apply_patch.exe"
Assert-CodexNotRunning $codexTarget

$backupRoot = Resolve-BackupRoot
$backupDirectory = Join-Path $backupRoot ([DateTime]::UtcNow.ToString("yyyyMMdd-HHmmssfff"))
[IO.Directory]::CreateDirectory($backupDirectory) | Out-Null

$codexBackup = Join-Path $backupDirectory "codex.exe"
$applyPatchBackup = Join-Path $backupDirectory "apply_patch.exe"
$applyPatchExisted = Test-Path -LiteralPath $applyPatchTarget -PathType Leaf

Write-Step "Backing up the installed npm binaries to $backupDirectory"
Copy-Item -LiteralPath $codexTarget -Destination $codexBackup
if ($applyPatchExisted) {
    Copy-Item -LiteralPath $applyPatchTarget -Destination $applyPatchBackup
}

$manifest = [ordered]@{
    schemaVersion = 1
    createdAtUtc = [DateTime]::UtcNow.ToString("o")
    npmRoot = $globalNpmRoot
    packageDirectory = $packageDirectory
    codexPath = $codexTarget
    applyPatchPath = $applyPatchTarget
    codexBackup = "codex.exe"
    applyPatchExisted = $applyPatchExisted
    applyPatchBackup = if ($applyPatchExisted) { "apply_patch.exe" } else { $null }
    originalCodexSha256 = Get-Sha256 $codexTarget
    originalApplyPatchSha256 = if ($applyPatchExisted) { Get-Sha256 $applyPatchTarget } else { $null }
    overlayCodexSha256 = Get-Sha256 $codexSource
    overlayApplyPatchSha256 = Get-Sha256 $applyPatchSource
}
$manifestPath = Join-Path $backupDirectory "manifest.json"
$manifestJson = $manifest | ConvertTo-Json
[IO.File]::WriteAllText($manifestPath, $manifestJson, [Text.UTF8Encoding]::new($false))

try {
    Write-Step "Installing codex.exe and native apply_patch.exe"
    [IO.Directory]::CreateDirectory($pathDirectory) | Out-Null
    Copy-FileReplacing $codexSource $codexTarget
    Copy-FileReplacing $applyPatchSource $applyPatchTarget

    if ((Get-Sha256 $codexTarget) -ne $manifest.overlayCodexSha256 -or
        (Get-Sha256 $applyPatchTarget) -ne $manifest.overlayApplyPatchSha256) {
        throw "Installed binary checksum verification failed."
    }

    $versionOutput = (& $codexTarget --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "The installed codex.exe failed its version check: $versionOutput"
    }
} catch {
    Write-Warning "Installation failed. Restoring the original npm files."
    Copy-FileReplacing $codexBackup $codexTarget
    if ($applyPatchExisted) {
        Copy-FileReplacing $applyPatchBackup $applyPatchTarget
    } elseif (Test-Path -LiteralPath $applyPatchTarget -PathType Leaf) {
        Remove-Item -LiteralPath $applyPatchTarget -Force
    }
    throw
}

Write-Host ""
Write-Host "Installed Windows overlay successfully: $versionOutput"
Write-Host "Package directory: $packageDirectory"
Write-Host "Backup directory:  $backupDirectory"
$powerShellExecutableName = if ($PSVersionTable.PSEdition -eq "Core") { "pwsh.exe" } else { "powershell.exe" }
$powerShellExecutable = Join-Path $PSHOME $powerShellExecutableName
$restoreScript = Join-Path $PayloadDirectory "restore.ps1"
Write-Host "Restore command:    & `"$powerShellExecutable`" -NoProfile -ExecutionPolicy Bypass -File `"$restoreScript`" -BackupDirectory `"$backupDirectory`""
