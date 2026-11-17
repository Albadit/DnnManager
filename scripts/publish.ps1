# Builds DNN Manager for the systems picked (the VS Code task "Publish") into dist\:
#   executable  dist\DnnManager-windows-x86_64.exe, dist\DnnManager-linux-amd64, ...
#   installer   the executable and its .msi / .dmg / .deb (made with cargo-packager)
# Without -Platform it lists the systems and asks which ones - several at once: "1,2", "all", or Enter for this PC's.
# A PC compiles for its own operating system only - Windows for both Windows builds, a Mac for both macOS builds,
# Linux for Linux. Others picked are skipped with a note: the release on GitHub (.github/workflows/release.yml) builds
# every one.
#
#   -Kind      executable (default) | installer
#   -Platform  one or more of: windows-x86_64, windows-arm64, macos-arm64, macos-x86_64, linux-amd64, linux-arm64,
#              linux-x86_64, linux-aarch64 (the last two are amd64 and arm64 under the other names, as the release
#              publishes them: DnnManager-linux-x86_64.deb, DnnManager-linux-aarch64.deb)
# The task's picker lists them itself (.vscode/tasks.json, input publishPlatforms).
# Every build is warning-free (RUSTFLAGS=-D warnings): a warning stops it.
param(
    [ValidateSet("executable", "installer")][string]$Kind = "executable",
    [string[]]$Platform = @()
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$order = @("windows-x86_64", "windows-arm64", "macos-arm64", "macos-x86_64", "linux-amd64", "linux-arm64", "linux-x86_64", "linux-aarch64")
$targets = @{
    "windows-x86_64" = @{ Triple = "x86_64-pc-windows-msvc"; Os = "Windows"; Ext = ".exe"; Format = "wix"; Toolchain = "stable-x86_64-pc-windows-msvc" }
    "windows-arm64"  = @{ Triple = "aarch64-pc-windows-msvc"; Os = "Windows"; Ext = ".exe"; Format = "wix"; Toolchain = "stable-x86_64-pc-windows-msvc" }
    "macos-arm64"    = @{ Triple = "aarch64-apple-darwin"; Os = "macOS"; Ext = ""; Format = "dmg"; Toolchain = "stable" }
    "macos-x86_64"   = @{ Triple = "x86_64-apple-darwin"; Os = "macOS"; Ext = ""; Format = "dmg"; Toolchain = "stable" }
    "linux-amd64"    = @{ Triple = "x86_64-unknown-linux-gnu"; Os = "Linux"; Ext = ""; Format = "deb"; Toolchain = "stable" }
    "linux-arm64"    = @{ Triple = "aarch64-unknown-linux-gnu"; Os = "Linux"; Ext = ""; Format = "deb"; Toolchain = "stable" }
    "linux-x86_64"   = @{ Triple = "x86_64-unknown-linux-gnu"; Os = "Linux"; Ext = ""; Format = "deb"; Toolchain = "stable" }
    "linux-aarch64"  = @{ Triple = "aarch64-unknown-linux-gnu"; Os = "Linux"; Ext = ""; Format = "deb"; Toolchain = "stable" }
}
$here = if ($IsMacOS) { "macOS" } elseif ($IsLinux) { "Linux" } else { "Windows" }
# The systems: given, or picked here.
$Platform = @($Platform | ForEach-Object { $_ -split "," } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($Platform.Count -eq 0) {
    Write-Host "Build the $Kind for which systems?"
    for ($i = 0; $i -lt $order.Count; $i++) {
        $name = $order[$i]
        $note = if ($targets[$name].Os -eq $here) { "" } else { "  (built on $($targets[$name].Os) - skipped here)" }
        Write-Host ("  {0}) {1}{2}" -f ($i + 1), $name, $note)
    }
    $own = @($order | Where-Object { $targets[$_].Os -eq $here })
    $answer = Read-Host "Numbers separated by commas, 'all', or Enter for this PC's ($($own -join ', '))"
    $Platform = if (-not $answer.Trim()) {
        $own
    } elseif ($answer.Trim() -eq "all") {
        $order
    } else {
        @($answer -split "[,\s]+" | Where-Object { $_ } | ForEach-Object {
            $n = 0
            if (-not [int]::TryParse($_, [ref]$n) -or $n -lt 1 -or $n -gt $order.Count) { throw "'$_' isn't one of 1-$($order.Count)." }
            $order[$n - 1]
        } | Select-Object -Unique)
    }
}
foreach ($p in $Platform) {
    if (-not $targets.ContainsKey($p)) { throw "Unknown system '$p'. Pick from: $($order -join ', ')." }
}

$made = @()
$skipped = @()
foreach ($p in $Platform) {
    $t = $targets[$p]
    if ($t.Os -ne $here) {
        $skipped += $p
        continue
    }
    Write-Host ""
    Write-Host "=== $p ($($t.Triple)) ===" -ForegroundColor Cyan
    # rust-toolchain.toml pins the Windows (MSVC) toolchain; elsewhere the host's stable one builds.
    $env:RUSTUP_TOOLCHAIN = $t.Toolchain
    rustup target add $t.Triple | Out-Null
    if ($p -eq "windows-arm64") {
        Write-Host "windows-arm64 needs Visual Studio's 'MSVC ARM64 build tools' component; without it the link step fails."
    }

    # Warnings are errors: a build that warns isn't published.
    $env:RUSTFLAGS = "-D warnings"
    cargo build --release --locked -p dnnmanager --target $t.Triple
    $built_ok = $LASTEXITCODE -eq 0
    Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
    if (-not $built_ok) { throw "The build for $p failed (a warning counts as an error)." }

    New-Item -ItemType Directory -Force dist | Out-Null
    $built = "target/$($t.Triple)/release/DnnManager$($t.Ext)"
    $out = "dist/DnnManager-$p$($t.Ext)"
    Copy-Item $built $out -Force
    $made += $out

    if ($Kind -eq "installer") {
        if (-not (Get-Command cargo-packager -ErrorAction SilentlyContinue)) {
            throw "cargo-packager isn't installed: cargo install cargo-packager --locked"
        }
        # cargo-packager takes the exe from its output folder, by its own name.
        $stage = "dist/package-$p"
        New-Item -ItemType Directory -Force $stage | Out-Null
        Copy-Item $built "$stage/DnnManager$($t.Ext)" -Force
        cargo packager --release --target $t.Triple --formats $t.Format --out-dir $stage
        if ($LASTEXITCODE -ne 0) { throw "Packaging $p failed." }
        $ext = @{ wix = "msi"; dmg = "dmg"; deb = "deb" }[$t.Format]
        $file = Get-ChildItem $stage -Filter "*.$ext" -Recurse | Select-Object -First 1
        $installer = "dist/DnnManager-$p.$ext"
        Copy-Item $file.FullName $installer -Force
        $made += $installer
    }
}

Write-Host ""
foreach ($m in $made) { Write-Host "Made: $m" -ForegroundColor Green }
foreach ($s in $skipped) {
    Write-Host "Skipped: $s is built on $($targets[$s].Os) (this PC runs $here) - the release on GitHub builds it (task 'Release: Github')." -ForegroundColor Yellow
}
if ($made.Count -eq 0) { exit 1 }
