[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Package", "Release")]
    [string]$Mode,
    [Parameter(Mandatory = $true)]
    [string]$PayloadDirectory,
    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory,
    [Parameter(Mandatory = $true)]
    [string]$UpstreamVersion,
    [Parameter(Mandatory = $true)]
    [string]$PackageVersion,
    [Parameter(Mandatory = $true)]
    [string]$Commit,
    [Parameter(Mandatory = $true)]
    [string]$Repository,
    [Parameter(Mandatory = $true)]
    [string]$WorkflowRunUrl,
    [Parameter(Mandatory = $true)]
    [string]$BuiltAtUtc,
    [string]$TarballPath,
    [string]$NpmShasum,
    [string]$NpmIntegrity
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)

    $stream = [IO.File]::OpenRead($Path)
    try {
        $sha = [Security.Cryptography.SHA256]::Create()
        try {
            return [Convert]::ToHexString($sha.ComputeHash($stream)).ToLowerInvariant()
        } finally {
            $sha.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Read-ExecutableChecksums {
    param([Parameter(Mandatory = $true)][string]$Path)

    $checksums = @{}
    foreach ($line in [IO.File]::ReadAllLines($Path)) {
        if ($line -notmatch '^([0-9a-fA-F]{64})  (codex\.exe|apply_patch\.exe)$') {
            throw "Invalid executable checksum line: $line"
        }
        $checksums[$Matches[2]] = $Matches[1].ToLowerInvariant()
    }
    foreach ($fileName in @("codex.exe", "apply_patch.exe")) {
        if (-not $checksums.ContainsKey($fileName)) {
            throw "SHA256SUMS does not contain $fileName."
        }
    }
    return $checksums
}

function Render-Template {
    param(
        [Parameter(Mandatory = $true)][string]$TemplatePath,
        [Parameter(Mandatory = $true)][string]$DestinationPath,
        [Parameter(Mandatory = $true)][hashtable]$Values
    )

    $template = [IO.File]::ReadAllText($TemplatePath)
    foreach ($key in $Values.Keys) {
        $template = $template.Replace("{{$key}}", [string]$Values[$key])
    }
    if ($template -match '{{[A-Z0-9_]+}}') {
        throw "Unresolved template value '$($Matches[0])' in $TemplatePath."
    }
    [IO.Directory]::CreateDirectory((Split-Path -Parent $DestinationPath)) | Out-Null
    [IO.File]::WriteAllText($DestinationPath, $template, [Text.UTF8Encoding]::new($false))
}

$PayloadDirectory = (Resolve-Path -LiteralPath $PayloadDirectory).Path
$templateDirectory = Join-Path $PSScriptRoot "docs\templates"
$checksums = Read-ExecutableChecksums (Join-Path $PayloadDirectory "SHA256SUMS")
$tarballName = "见同一次流水线的最终发布材料"
$tarballSize = "见同一次流水线的最终发布材料"
$tarballSha256 = "见同一次流水线的最终发布材料"
$resolvedNpmShasum = "发布后以 npm registry 为准"
$resolvedNpmIntegrity = "发布后以 npm registry 为准"

if ($Mode -eq "Release") {
    if ([string]::IsNullOrWhiteSpace($TarballPath) -or
        [string]::IsNullOrWhiteSpace($NpmShasum) -or
        [string]::IsNullOrWhiteSpace($NpmIntegrity)) {
        throw "Release mode requires TarballPath, NpmShasum, and NpmIntegrity."
    }
    $TarballPath = (Resolve-Path -LiteralPath $TarballPath).Path
    $tarball = Get-Item -LiteralPath $TarballPath
    $tarballName = $tarball.Name
    $tarballSize = "$($tarball.Length) bytes"
    $tarballSha256 = Get-Sha256 $TarballPath
    $resolvedNpmShasum = $NpmShasum
    $resolvedNpmIntegrity = $NpmIntegrity
}

$values = @{
    APPLY_PATCH_SHA256 = $checksums["apply_patch.exe"]
    BUILT_AT_UTC = $BuiltAtUtc
    CODEX_SHA256 = $checksums["codex.exe"]
    COMMIT = $Commit
    NPM_INTEGRITY = $resolvedNpmIntegrity
    NPM_SHASUM = $resolvedNpmShasum
    PACKAGE_VERSION = $PackageVersion
    REPOSITORY = $Repository
    REPOSITORY_URL = "https://github.com/$Repository"
    TARBALL_NAME = $tarballName
    TARBALL_SHA256 = $tarballSha256
    TARBALL_SIZE = $tarballSize
    UPSTREAM_VERSION = $UpstreamVersion
    WORKFLOW_RUN_URL = $WorkflowRunUrl
}

$docsDirectory = Join-Path $OutputDirectory "docs"
Render-Template (Join-Path $templateDirectory "README.template.md") (Join-Path $OutputDirectory "README.md") $values
Render-Template (Join-Path $templateDirectory "操作说明.template.md") (Join-Path $docsDirectory "操作说明.md") $values
Render-Template (Join-Path $templateDirectory "验证提示词.template.md") (Join-Path $docsDirectory "验证提示词.md") $values
Render-Template (Join-Path $templateDirectory "日志反馈说明.template.md") (Join-Path $docsDirectory "日志反馈说明.md") $values

if ($Mode -eq "Release") {
    Copy-Item -LiteralPath (Join-Path $docsDirectory "操作说明.md") `
        -Destination (Join-Path $OutputDirectory "操作说明-$PackageVersion.md")
    [IO.File]::WriteAllText(
        (Join-Path $OutputDirectory "$tarballName.sha256"),
        "$tarballSha256  $tarballName`n",
        [Text.UTF8Encoding]::new($false)
    )
    $manifest = [ordered]@{
        package = "@chenronggui/codex-win-patch"
        packageVersion = $PackageVersion
        upstreamVersion = $UpstreamVersion
        target = "x86_64-pc-windows-msvc"
        commit = $Commit
        repository = $Repository
        workflowRunUrl = $WorkflowRunUrl
        builtAtUtc = $BuiltAtUtc
        tarball = [ordered]@{
            file = $tarballName
            size = (Get-Item -LiteralPath $TarballPath).Length
            sha256 = $tarballSha256
            npmShasum = $NpmShasum
            npmIntegrity = $NpmIntegrity
        }
        executables = [ordered]@{
            "codex.exe" = $checksums["codex.exe"]
            "apply_patch.exe" = $checksums["apply_patch.exe"]
        }
    }
    $manifestText = $manifest | ConvertTo-Json -Depth 10
    [IO.File]::WriteAllText(
        (Join-Path $OutputDirectory "release-manifest.json"),
        $manifestText,
        [Text.UTF8Encoding]::new($false)
    )
}
