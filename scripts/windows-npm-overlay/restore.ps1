[CmdletBinding()]
param(
    [string]$BackupDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

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

function Copy-FileReplacing {
    param(
        [string]$Source,
        [string]$Destination
    )

    [IO.File]::Copy($Source, $Destination, $true)
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
            # Ignore processes whose executable paths cannot be inspected.
        }
    }
    if ($matchingProcesses.Count -gt 0) {
        $processIds = ($matchingProcesses.Id -join ", ")
        throw "Codex is still running from the target package (PID: $processIds). Close it and rerun restore."
    }
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
    throw "This restore script supports Windows only."
}

if ([string]::IsNullOrWhiteSpace($BackupDirectory)) {
    $backupRoot = Resolve-BackupRoot
    $latestBackup = Get-ChildItem -LiteralPath $backupRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName "manifest.json") -PathType Leaf } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if ($null -eq $latestBackup) {
        throw "No Windows overlay backup was found under $backupRoot."
    }
    $BackupDirectory = $latestBackup.FullName
}

$BackupDirectory = (Resolve-Path -LiteralPath $BackupDirectory).Path
$manifestPath = Join-Path $BackupDirectory "manifest.json"
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Missing backup manifest: $manifestPath"
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ([int]$manifest.schemaVersion -ne 1) {
    throw "Unsupported backup manifest version: $($manifest.schemaVersion)"
}

$codexBackup = Join-Path $BackupDirectory ([string]$manifest.codexBackup)
$codexTarget = [string]$manifest.codexPath
$applyPatchTarget = [string]$manifest.applyPatchPath
if (-not (Test-Path -LiteralPath $codexBackup -PathType Leaf)) {
    throw "Missing codex.exe backup: $codexBackup"
}
if ((Get-Sha256 $codexBackup) -ne [string]$manifest.originalCodexSha256) {
    throw "The codex.exe backup checksum does not match its manifest."
}

$applyPatchBackup = $null
if ([bool]$manifest.applyPatchExisted) {
    $applyPatchBackup = Join-Path $BackupDirectory ([string]$manifest.applyPatchBackup)
    if (-not (Test-Path -LiteralPath $applyPatchBackup -PathType Leaf)) {
        throw "Missing apply_patch.exe backup: $applyPatchBackup"
    }
    if ((Get-Sha256 $applyPatchBackup) -ne [string]$manifest.originalApplyPatchSha256) {
        throw "The apply_patch.exe backup checksum does not match its manifest."
    }
}

Assert-CodexNotRunning $codexTarget
Write-Host "==> Restoring npm files from $BackupDirectory"
Copy-FileReplacing $codexBackup $codexTarget
if ([bool]$manifest.applyPatchExisted) {
    Copy-FileReplacing $applyPatchBackup $applyPatchTarget
} elseif (Test-Path -LiteralPath $applyPatchTarget -PathType Leaf) {
    Remove-Item -LiteralPath $applyPatchTarget -Force
}

if ((Get-Sha256 $codexTarget) -ne [string]$manifest.originalCodexSha256) {
    throw "Restored codex.exe checksum verification failed."
}

$versionOutput = (& $codexTarget --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "The restored codex.exe failed its version check: $versionOutput"
}

Write-Host "Restored the original npm installation successfully: $versionOutput"
