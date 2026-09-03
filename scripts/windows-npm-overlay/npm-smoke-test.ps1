[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackageTarball,

    [Parameter(Mandatory = $true)]
    [string]$CodexExe,

    [Parameter(Mandatory = $true)]
    [string]$ApplyPatchExe,

    [Parameter(Mandatory = $true)]
    [string]$UpstreamVersion
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
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

function Assert-Equal {
    param(
        [object]$Expected,
        [object]$Actual,
        [string]$Message
    )

    if ($Expected -ne $Actual) {
        throw "${Message}: expected '$Expected', got '$Actual'."
    }
}

function Invoke-ExpectSuccess {
    param(
        [string]$FilePath,
        [string[]]$ArgumentList
    )

    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $FilePath $($ArgumentList -join ' ')"
    }
}

function Invoke-ExpectFailure {
    param(
        [string]$FilePath,
        [string[]]$ArgumentList,
        [string]$ExpectedMessage
    )

    $output = (& $FilePath @ArgumentList 2>&1 | Out-String)
    if ($LASTEXITCODE -eq 0) {
        throw "Command unexpectedly succeeded: $FilePath $($ArgumentList -join ' ')"
    }
    if ($output -notmatch [regex]::Escape($ExpectedMessage)) {
        throw "Command failure did not contain '$ExpectedMessage'. Output: $output"
    }
}

function Write-PackageManifest {
    param(
        [string]$Path,
        [string]$Version
    )

    $json = [ordered]@{ version = $Version } | ConvertTo-Json
    [IO.File]::WriteAllText($Path, $json, [Text.UTF8Encoding]::new($false))
}

$PackageTarball = (Resolve-Path -LiteralPath $PackageTarball).Path
$CodexExe = (Resolve-Path -LiteralPath $CodexExe).Path
$ApplyPatchExe = (Resolve-Path -LiteralPath $ApplyPatchExe).Path
$testDirectory = Join-Path ([IO.Path]::GetTempPath()) ("codex-npm-overlay-" + [Guid]::NewGuid().ToString("N"))
$npmPrefix = Join-Path $testDirectory "npm-prefix"
$globalNpmRoot = Join-Path $npmPrefix "node_modules"
$packageDirectory = Join-Path $globalNpmRoot "@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc"
$codexTarget = Join-Path $packageDirectory "bin\codex.exe"
$applyPatchTarget = Join-Path $packageDirectory "codex-path\apply_patch.exe"
$packageManifestPath = Join-Path $packageDirectory "codex-package.json"
$backupRoot = Join-Path $testDirectory "backups"
$previousNpmPrefix = [Environment]::GetEnvironmentVariable("npm_config_prefix")
$previousBackupRoot = [Environment]::GetEnvironmentVariable("CODEX_WINDOWS_PATCH_BACKUP_ROOT")
$previousPowerShell = [Environment]::GetEnvironmentVariable("CODEX_WINDOWS_PATCH_POWERSHELL")

try {
    [IO.Directory]::CreateDirectory((Split-Path -Parent $codexTarget)) | Out-Null
    [IO.Directory]::CreateDirectory((Split-Path -Parent $applyPatchTarget)) | Out-Null
    Write-PackageManifest $packageManifestPath $UpstreamVersion
    $nodeExe = (Get-Command "node.exe" -ErrorAction Stop).Source
    Copy-Item -LiteralPath $nodeExe -Destination $codexTarget
    Copy-Item -LiteralPath "$env:SystemRoot\System32\whoami.exe" -Destination $applyPatchTarget
    $originalCodexSha256 = Get-Sha256 $codexTarget
    $originalApplyPatchSha256 = Get-Sha256 $applyPatchTarget

    $env:npm_config_prefix = $npmPrefix
    $env:CODEX_WINDOWS_PATCH_BACKUP_ROOT = $backupRoot
    $env:CODEX_WINDOWS_PATCH_POWERSHELL = "pwsh.exe"

    Invoke-ExpectSuccess "npm.cmd" @(
        "install",
        "--global",
        "--ignore-scripts=false",
        "--no-audit",
        "--no-fund",
        "--prefix",
        $npmPrefix,
        $PackageTarball
    )

    Assert-Equal (Get-Sha256 $CodexExe) (Get-Sha256 $codexTarget) "postinstall codex.exe hash"
    Assert-Equal (Get-Sha256 $ApplyPatchExe) (Get-Sha256 $applyPatchTarget) "postinstall apply_patch.exe hash"

    $installCommand = Join-Path $npmPrefix "codex-win-patch-install.cmd"
    $restoreCommand = Join-Path $npmPrefix "codex-win-patch-restore.cmd"
    Invoke-ExpectSuccess $restoreCommand @()
    Assert-Equal $originalCodexSha256 (Get-Sha256 $codexTarget) "restored codex.exe hash"
    Assert-Equal $originalApplyPatchSha256 (Get-Sha256 $applyPatchTarget) "restored apply_patch.exe hash"

    $env:CODEX_WINDOWS_PATCH_POWERSHELL = "powershell.exe"
    Invoke-ExpectSuccess $installCommand @()
    Assert-Equal (Get-Sha256 $CodexExe) (Get-Sha256 $codexTarget) "Windows PowerShell codex.exe hash"
    Invoke-ExpectSuccess $restoreCommand @()

    Write-PackageManifest $packageManifestPath "0.0.0-mismatch"
    Invoke-ExpectFailure $installCommand @() "Version mismatch"
    Assert-Equal $originalCodexSha256 (Get-Sha256 $codexTarget) "version mismatch target hash"
    Write-PackageManifest $packageManifestPath $UpstreamVersion

    $installedPatchDirectory = Join-Path $globalNpmRoot "@chenronggui\codex-win-patch"
    $checksumPath = Join-Path $installedPatchDirectory "SHA256SUMS"
    $originalChecksums = [IO.File]::ReadAllText($checksumPath)
    $corruptedChecksums = $originalChecksums -replace '^[0-9a-fA-F]{64}', ('0' * 64)
    [IO.File]::WriteAllText($checksumPath, $corruptedChecksums, [Text.UTF8Encoding]::new($false))
    Invoke-ExpectFailure $installCommand @() "SHA-256 mismatch"
    Assert-Equal $originalCodexSha256 (Get-Sha256 $codexTarget) "checksum failure target hash"
    [IO.File]::WriteAllText($checksumPath, $originalChecksums, [Text.UTF8Encoding]::new($false))

    $missingPackageDirectory = "${packageDirectory}.missing"
    Move-Item -LiteralPath $packageDirectory -Destination $missingPackageDirectory
    try {
        Invoke-ExpectFailure $installCommand @() "Could not find the Windows x64 Codex platform package"
    } finally {
        Move-Item -LiteralPath $missingPackageDirectory -Destination $packageDirectory
    }

    Copy-Item -LiteralPath "$env:SystemRoot\System32\ping.exe" -Destination $codexTarget -Force
    $runningCodexSha256 = Get-Sha256 $codexTarget
    $codexProcess = Start-Process -FilePath $codexTarget -ArgumentList "-t", "127.0.0.1" -WindowStyle Hidden -PassThru
    try {
        Start-Sleep -Milliseconds 500
        Invoke-ExpectFailure $installCommand @() "Codex is still running"
        Assert-Equal $runningCodexSha256 (Get-Sha256 $codexTarget) "running process target hash"
    } finally {
        Stop-Process -Id $codexProcess.Id -Force -ErrorAction SilentlyContinue
        $codexProcess.WaitForExit()
    }

    Write-Host "Windows npm overlay smoke tests passed."
} finally {
    [Environment]::SetEnvironmentVariable("npm_config_prefix", $previousNpmPrefix)
    [Environment]::SetEnvironmentVariable("CODEX_WINDOWS_PATCH_BACKUP_ROOT", $previousBackupRoot)
    [Environment]::SetEnvironmentVariable("CODEX_WINDOWS_PATCH_POWERSHELL", $previousPowerShell)
    if (Test-Path -LiteralPath $testDirectory -PathType Container) {
        Remove-Item -LiteralPath $testDirectory -Recurse -Force
    }
}

exit 0
