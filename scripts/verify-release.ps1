param(
    [Parameter(Mandatory = $true)]
    [string]$ArchivePath,
    [string]$ExpectedVersion
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$archive = (Resolve-Path $ArchivePath).Path
if (-not (Test-Path $archive -PathType Leaf)) {
    throw "Release archive was not found: $ArchivePath"
}
if ((Get-Item $archive).Length -le 0) {
    throw "Release archive is empty: $archive"
}

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("intimatr-release-verify-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

try {
    Expand-Archive -Path $archive -DestinationPath $tempRoot -Force

    $requiredFiles = @(
        "intimatr.dll",
        "README.md",
        "project.md",
        "VERSION.txt",
        "SHA256SUMS.txt",
        "config/ExampleGame.exe.toml",
        "docs/quickstart.md",
        "docs/troubleshooting.md",
        "docs/rpc.md",
        "docs/ui.md",
        "docs/debugger.md",
        "docs/analysis.md",
        "docs/performance.md",
        "docs/extensions.md",
        "docs/versioning.md"
    )

    foreach ($relative in $requiredFiles) {
        $path = Join-Path $tempRoot $relative
        if (-not (Test-Path $path -PathType Leaf)) {
            throw "Required release file is missing from archive: $relative"
        }
    }

    $dll = Join-Path $tempRoot "intimatr.dll"
    if ((Get-Item $dll).Length -le 0) {
        throw "Packaged intimatr.dll is empty"
    }

    $sumFile = Join-Path $tempRoot "SHA256SUMS.txt"
    $sumLine = Get-Content $sumFile | Where-Object { $_.Trim().Length -gt 0 } | Select-Object -First 1
    if (-not $sumLine) {
        throw "SHA256SUMS.txt is empty"
    }
    if ($sumLine -notmatch '^([0-9a-fA-F]{64})\s+intimatr\.dll$') {
        throw "SHA256SUMS.txt has an invalid intimatr.dll entry: $sumLine"
    }

    $expectedDllHash = $Matches[1].ToLowerInvariant()
    $actualDllHash = (Get-FileHash $dll -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualDllHash -ne $expectedDllHash) {
        throw "Packaged DLL checksum mismatch: expected $expectedDllHash, got $actualDllHash"
    }

    $versionFile = Join-Path $tempRoot "VERSION.txt"
    $versionText = Get-Content $versionFile -Raw
    if ($versionText -notmatch '(?m)^Intimatr v([^\r\n]+)$') {
        throw "VERSION.txt does not contain an Intimatr version line"
    }
    $packagedVersion = $Matches[1].Trim()

    if ($ExpectedVersion -and $packagedVersion -ne $ExpectedVersion) {
        throw "Packaged version '$packagedVersion' does not match expected version '$ExpectedVersion'"
    }

    if ($versionText -notmatch '(?m)^RPC protocol: [0-9]+$') {
        throw "VERSION.txt does not contain an RPC protocol version"
    }
    if ($versionText -notmatch '(?m)^Analysis workspace format: [0-9]+$') {
        throw "VERSION.txt does not contain an analysis workspace version"
    }

    Write-Host "Verified release archive: $archive"
    Write-Host "Version: $packagedVersion"
    Write-Host "DLL SHA256: $actualDllHash"
    Write-Output $archive
}
finally {
    if (Test-Path $tempRoot) {
        Remove-Item $tempRoot -Recurse -Force
    }
}
