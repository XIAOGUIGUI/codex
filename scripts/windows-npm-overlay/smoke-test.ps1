[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ApplyPatchExe
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$PSNativeCommandUseErrorActionPreference = $false
$OutputEncoding = [Text.UTF8Encoding]::new($false)

function Invoke-Patch {
    param(
        [string]$Patch,
        [int]$ExpectedExitCode
    )

    $output = ($Patch | & $ApplyPatchExe 2>&1 | Out-String).Trim()
    $actualExitCode = $LASTEXITCODE
    if ($actualExitCode -ne $ExpectedExitCode) {
        throw "apply_patch exit code was $actualExitCode, expected $ExpectedExitCode. Output: $output"
    }
    return $output
}

function Assert-BytesEqual {
    param(
        [byte[]]$Expected,
        [byte[]]$Actual,
        [string]$Message
    )

    if ($Expected.Length -ne $Actual.Length) {
        throw $Message
    }
    for ($index = 0; $index -lt $Expected.Length; $index++) {
        if ($Expected[$index] -ne $Actual[$index]) {
            throw $Message
        }
    }
}

$ApplyPatchExe = (Resolve-Path -LiteralPath $ApplyPatchExe).Path
$testDirectory = Join-Path ([IO.Path]::GetTempPath()) ("codex-apply-patch-smoke-" + [Guid]::NewGuid().ToString("N"))
[IO.Directory]::CreateDirectory($testDirectory) | Out-Null
$previousLocation = Get-Location

try {
    Set-Location $testDirectory
    $utf8 = [Text.UTF8Encoding]::new($false)

    $bomCrlfPath = Join-Path $testDirectory "bom-crlf.txt"
    $bom = [byte[]](0xEF, 0xBB, 0xBF)
    $bomBody = $utf8.GetBytes("fn main() {`r`n    println!(`"你好`");`r`n}`r`n")
    [IO.File]::WriteAllBytes($bomCrlfPath, [byte[]]($bom + $bomBody))
    $bomPatch = @'
*** Begin Patch
*** Update File: bom-crlf.txt
@@
-    println!("你好");
+    println!("世界");
*** End Patch
'@
    $null = Invoke-Patch $bomPatch 0
    $updatedBytes = [IO.File]::ReadAllBytes($bomCrlfPath)
    Assert-BytesEqual $bom ($updatedBytes[0..2]) "The UTF-8 BOM was not preserved."
    for ($index = 3; $index -lt $updatedBytes.Length; $index++) {
        if ($updatedBytes[$index] -eq 0x0A -and $updatedBytes[$index - 1] -ne 0x0D) {
            throw "A CRLF line ending was converted to LF."
        }
    }
    $updatedText = $utf8.GetString($updatedBytes, 3, $updatedBytes.Length - 3)
    if ($updatedText -notmatch 'println!\("世界"\);') {
        throw "The UTF-8 Chinese replacement was not applied."
    }

    $indentPath = Join-Path $testDirectory "indent.txt"
    [IO.File]::WriteAllText($indentPath, "fn main() {`r`n    value();`r`n}`r`n", $utf8)
    $indentBefore = [IO.File]::ReadAllBytes($indentPath)
    $indentPatch = @'
*** Begin Patch
*** Update File: indent.txt
@@
-value();
+changed();
*** End Patch
'@
    $null = Invoke-Patch $indentPatch 1
    Assert-BytesEqual $indentBefore ([IO.File]::ReadAllBytes($indentPath)) "Indentation mismatch changed the file."

    $noOpPath = Join-Path $testDirectory "no-op.txt"
    [IO.File]::WriteAllText($noOpPath, "same`r`n", $utf8)
    $noOpBefore = [IO.File]::ReadAllBytes($noOpPath)
    $noOpPatch = @'
*** Begin Patch
*** Update File: no-op.txt
@@
-same
+same
*** End Patch
'@
    $null = Invoke-Patch $noOpPatch 1
    Assert-BytesEqual $noOpBefore ([IO.File]::ReadAllBytes($noOpPath)) "A no-op patch changed the file."

    $utf16Path = Join-Path $testDirectory "utf16.txt"
    [IO.File]::WriteAllText($utf16Path, "old`r`n", [Text.Encoding]::Unicode)
    $utf16Before = [IO.File]::ReadAllBytes($utf16Path)
    $utf16Patch = @'
*** Begin Patch
*** Update File: utf16.txt
@@
-old
+new
*** End Patch
'@
    $utf16Output = Invoke-Patch $utf16Patch 1
    Assert-BytesEqual $utf16Before ([IO.File]::ReadAllBytes($utf16Path)) "A UTF-16 file was changed."
    if ($utf16Output -notmatch "UTF-16 little-endian") {
        throw "The UTF-16 failure did not explain the detected encoding."
    }

    $largePath = Join-Path $testDirectory "large.txt"
    [IO.File]::WriteAllText($largePath, "marker`r`n", $utf8)
    $largeLine = "x" * 40000
    $largePatch = "*** Begin Patch`n*** Update File: large.txt`n@@`n-marker`n+$largeLine`n*** End Patch`n"
    $null = Invoke-Patch $largePatch 0
    $largeText = [IO.File]::ReadAllText($largePath, $utf8)
    if (-not $largeText.StartsWith($largeLine) -or $largeText.Length -lt 40002) {
        throw "The patch larger than the Windows command-line limit was not applied through stdin."
    }

    Write-Host "Windows apply_patch smoke tests passed."
} finally {
    Set-Location $previousLocation
    if (Test-Path -LiteralPath $testDirectory -PathType Container) {
        Remove-Item -LiteralPath $testDirectory -Recurse -Force
    }
}
