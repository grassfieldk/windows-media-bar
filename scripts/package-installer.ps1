param(
    [string]$Version,
    [string]$CompilerPath
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
if (-not $Version) {
    $manifest = Get-Content (Join-Path $projectRoot 'Cargo.toml') -Raw
    $Version = [regex]::Match($manifest, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
}
$Version = $Version -replace '^v', ''
if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw 'Version must have the form 1.2.3 or v1.2.3'
}

if (-not $CompilerPath) {
    $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($command) {
        $CompilerPath = $command.Source
    } else {
        $CompilerPath = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
        if (-not (Test-Path -LiteralPath $CompilerPath -PathType Leaf)) {
            $CompilerPath = Join-Path $projectRoot 'target\tools\inno-setup\ISCC.exe'
        }
    }
}
if (-not (Test-Path -LiteralPath $CompilerPath -PathType Leaf)) {
    throw 'Inno Setup 6 compiler not found. Supply -CompilerPath with the path to ISCC.exe'
}

$binaryPath = Join-Path $projectRoot 'target\x86_64-pc-windows-msvc\release\windows-media-bar.exe'
if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
    throw 'Release binary not found. Run mise run build first'
}
$outputPath = Join-Path $projectRoot 'target\installer'
& $CompilerPath "/DAppVersion=$Version" "/DBinaryPath=$binaryPath" "/DOutputPath=$outputPath" (Join-Path $projectRoot 'installer\windows-media-bar.iss')
if ($LASTEXITCODE -ne 0) {
    throw "Inno Setup failed with exit code $LASTEXITCODE"
}
$installerPath = Join-Path $outputPath "windows-media-bar-v$Version-x86_64-setup.exe"
if (-not (Test-Path -LiteralPath $installerPath -PathType Leaf)) {
    throw "Installer was not generated: $installerPath"
}
Write-Output $installerPath
$algorithm = [System.Security.Cryptography.SHA256]::Create()
$stream = [System.IO.File]::OpenRead($installerPath)
try {
    $checksum = [BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-', '').ToLowerInvariant()
} finally {
    $stream.Dispose()
    $algorithm.Dispose()
}
Set-Content -LiteralPath "$installerPath.sha256" -Value "$checksum  $(Split-Path $installerPath -Leaf)" -Encoding ascii
