param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [string]$OutputDirectory = "dist"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$dll = "target/x86_64-pc-windows-msvc/release/intimatr.dll"
if (-not (Test-Path $dll -PathType Leaf)) {
    throw "Expected release DLL was not found: $dll"
}
if ((Get-Item $dll).Length -le 0) {
    throw "Release DLL is empty: $dll"
}

$protocolMatch = Select-String -Path "src/rpc/protocol.rs" -Pattern 'pub const PROTOCOL_VERSION: u16 = ([0-9]+);' | Select-Object -First 1
if (-not $protocolMatch) {
    throw "Could not resolve RPC protocol version"
}
$protocolVersion = $protocolMatch.Matches[0].Groups[1].Value

$workspaceMatch = Select-String -Path "src/analysis.rs" -Pattern 'pub const WORKSPACE_VERSION: u32 = ([0-9]+);' | Select-Object -First 1
if (-not $workspaceMatch) {
    throw "Could not resolve analysis workspace version"
}
$workspaceVersion = $workspaceMatch.Matches[0].Groups[1].Value

$name = "intimatr-v$Version-windows-x86_64"
$root = Join-Path $OutputDirectory $name
$docs = Join-Path $root "docs"
$config = Join-Path $root "config"
$zip = Join-Path $OutputDirectory "$name.zip"

if (Test-Path $root) {
    Remove-Item $root -Recurse -Force
}
if (Test-Path $zip) {
    Remove-Item $zip -Force
}
New-Item -ItemType Directory -Force -Path $docs, $config | Out-Null

$releaseDocs = @(
    "quickstart.md",
    "troubleshooting.md",
    "rpc.md",
    "ui.md",
    "debugger.md",
    "analysis.md",
    "performance.md",
    "extensions.md",
    "versioning.md"
)

$requiredFiles = @(
    "README.md",
    "project.md",
    "config/ExampleGame.exe.toml"
)
$requiredFiles += $releaseDocs | ForEach-Object { Join-Path "docs" $_ }

foreach ($file in $requiredFiles) {
    if (-not (Test-Path $file -PathType Leaf)) {
        throw "Required release file is missing: $file"
    }
}

Copy-Item $dll $root
$pdb = "target/x86_64-pc-windows-msvc/release/intimatr.pdb"
if (Test-Path $pdb -PathType Leaf) {
    Copy-Item $pdb $root
}
Copy-Item "config/ExampleGame.exe.toml" $config
Copy-Item "README.md" $root
Copy-Item "project.md" $root
foreach ($doc in $releaseDocs) {
    Copy-Item (Join-Path "docs" $doc) $docs
}

$dllHash = (Get-FileHash $dll -Algorithm SHA256).Hash.ToLowerInvariant()
"$dllHash  intimatr.dll" | Set-Content -Path (Join-Path $root "SHA256SUMS.txt") -Encoding utf8

@"
Intimatr v$Version
Platform: Windows x86_64 (MSVC)
RPC protocol: $protocolVersion
Analysis workspace format: $workspaceVersion
Scope: offline/single-player memory research
"@ | Set-Content -Path (Join-Path $root "VERSION.txt") -Encoding utf8

Compress-Archive -Path "$root/*" -DestinationPath $zip -Force
if (-not (Test-Path $zip -PathType Leaf)) {
    throw "Release archive was not created: $zip"
}
if ((Get-Item $zip).Length -le 0) {
    throw "Release archive is empty: $zip"
}

Write-Output $zip
