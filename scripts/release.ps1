# Starts a release (the VS Code task "Release"): tags the version in Cargo.toml as vX.Y.Z and pushes the branch and the
# tag. GitHub then builds, packages and publishes it (.github/workflows/release.yml) - nothing built here is uploaded.
# Run it from the repository root with a clean working tree, after the version in Cargo.toml was raised and committed.
#
#   -Suffix rc1   tags vX.Y.Z-rc1: a pre-release
#   -Redo         moves an existing tag to this commit and pushes it again (a failed release run, before anyone has it)
param(
    [string]$Suffix = "",
    [switch]$Redo
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$line = Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1
if (-not $line) { throw "No version in Cargo.toml." }
$version = $line.Matches[0].Groups[1].Value
$tag = if ($Suffix) { "v$version-$Suffix" } else { "v$version" }

$dirty = git status --porcelain
if ($dirty) { throw "The working tree has changes - commit or stash them first:`n$dirty" }

$branch = git rev-parse --abbrev-ref HEAD
$exists = git tag --list $tag
if ($exists -and -not $Redo) { throw "$tag exists already. Raise the version in Cargo.toml, or use -Redo to move it." }

$notes = ".docs/release-notes/$tag.md"
Write-Host "Release $tag from $branch ($(git rev-parse --short HEAD))."
Write-Host ($(if (Test-Path $notes) { "Notes: $notes" } else { "No $notes - GitHub writes the notes from the commits." }))
$answer = Read-Host "Push $tag now? (y/n)"
if ($answer -ne "y") { Write-Host "Nothing pushed."; exit 0 }

git push origin $branch
if ($Redo -and $exists) {
    git tag -d $tag | Out-Null
    git push origin ":refs/tags/$tag"
}
git tag -a $tag -m "DNN Manager $version$(if ($Suffix) { " ($Suffix)" })"
git push origin $tag
Write-Host "Pushed $tag. Follow it on GitHub: Actions > Release. Approve the 'publish' environment if it asks."
