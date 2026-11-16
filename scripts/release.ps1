# Starts a release (the VS Code task "Release: Github"). The task asks four things, each a picker filled by this
# script, then runs it to tag and push:
#
#   1. type      -Label latest | prerelease | none           (how GitHub labels the release)
#   2. version   -Suggest -Label <type>                       next versions after the newest release tag; any other
#                                                             can be typed (the picker allows custom values)
#   3. commit    -Commits                                     the newest commits on the branch's remote
#   4. confirm   -Summary -Label -Version -Commit             the release spelled out, or why it can't be made
#
# Then:  release.ps1 -Label <type> -Version <x.y.z[-suffix]> -Commit <sha> -Confirm yes
# checks everything again, tags the commit vX.Y.Z (an annotated tag whose message carries "Release-Label: <type>") and
# pushes only the tag. GitHub (.github/workflows/release.yml) then builds and tests on every system, packages the
# installers, and publishes the release only when all of that passed - nothing built on this PC is uploaded.
#
#   -Redo   moves an existing tag that has no published release (a failed run) to the commit picked
param(
    [ValidateSet("latest", "prerelease", "none")][string]$Label = "latest",
    [string]$Version = "",
    [string]$Commit = "",
    [string]$Confirm = "",
    [switch]$Suggest,
    [switch]$Commits,
    [switch]$Summary,
    [switch]$Redo
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$labelNames = @{ latest = "Latest"; prerelease = "Pre-release"; none = "None" }

# git for a call that may fail (its exit code is the answer): Windows PowerShell would stop on its stderr otherwise.
function Git-Quiet {
    $ErrorActionPreference = "Continue"
    & git @args 2>$null
}

function Repo-Slug {
    $url = (git remote get-url origin).Trim()
    if ($url -match "github\.com[:/](.+?)(\.git)?$") { return $Matches[1] }
    throw "origin isn't a GitHub repository: $url"
}

# The branch's remote (origin/<branch>), else origin/main: a release is made from a pushed commit only.
function Upstream {
    $u = Git-Quiet rev-parse --abbrev-ref --symbolic-full-name "@{u}"
    if ($LASTEXITCODE -eq 0 -and $u) { return $u.Trim() }
    return "origin/main"
}

# vX.Y.Z[-suffix] -> @{ Major; Minor; Patch; Suffix; Text }; $null when it isn't one.
function Parse-Version([string]$text) {
    $t = $text.Trim().TrimStart("v", "V")
    if ($t -notmatch "^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z][0-9A-Za-z.]*))?$") { return $null }
    return @{ Major = [int]$Matches[1]; Minor = [int]$Matches[2]; Patch = [int]$Matches[3]; Suffix = $Matches[4]; Text = $t }
}

function Base-Key($v) { return ($v.Major * 1000000) + ($v.Minor * 1000) + $v.Patch }

# The version tags on GitHub (origin) - not this PC's: a tag made here and never pushed isn't a release.
function Tags {
    $names = @(Git-Quiet ls-remote --tags --refs origin "v*") | ForEach-Object { ($_ -split "refs/tags/")[1] }
    return $names | ForEach-Object { Parse-Version $_ } | Where-Object { $_ }
}

function Newest-Stable($tags) {
    return $tags | Where-Object { -not $_.Suffix } | Sort-Object { Base-Key $_ } | Select-Object -Last 1
}

# Everything that would make the release wrong - empty when it can be made.
function Problems([string]$label, [string]$version, [string]$commit, [switch]$redo) {
    $issues = @()
    $v = Parse-Version $version
    if (-not $v) {
        return @("'$version' isn't a version: use X.Y.Z, or X.Y.Z-rc1 for a pre-release.")
    }
    if ($label -eq "prerelease" -and -not $v.Suffix) {
        $issues += "A pre-release needs a suffix, e.g. $($v.Text)-rc1 - so the final $($v.Text) can follow it."
    }
    if ($label -ne "prerelease" -and $v.Suffix) {
        $issues += "$($v.Text) has a suffix, which makes it a pre-release: pick Pre-release, or drop the suffix."
    }
    $tags = Tags
    $newest = Newest-Stable $tags
    if ($newest -and (Base-Key $v) -le (Base-Key $newest)) {
        $issues += "v$($newest.Text) is released already: the version must be newer."
    }
    $tag = "v$($v.Text)"
    $local = git tag --list $tag
    $remote = Git-Quiet ls-remote --tags origin "refs/tags/$tag"
    if (($local -or $remote) -and -not $redo) {
        $issues += "$tag exists already. Pick another version (or run the script with -Redo to move it)."
    }
    if (-not $commit) {
        $issues += "No commit picked."
    } else {
        Git-Quiet cat-file -e "$commit^{commit}" | Out-Null
        if ($LASTEXITCODE -ne 0) {
            $issues += "Commit $commit isn't in this repository - fetch first."
        } else {
            $upstream = Upstream
            Git-Quiet merge-base --is-ancestor $commit $upstream | Out-Null
            if ($LASTEXITCODE -ne 0) { $issues += "Commit $commit isn't on $upstream - push it first." }
        }
    }
    return $issues
}

function Notes-Line([string]$version) {
    $notes = ".docs/release-notes/v$((Parse-Version $version).Text).md"
    if (Test-Path $notes) { return "Notes: $notes" }
    return "No $notes - GitHub writes the notes from the commits"
}

# -- 2. version: the versions with release notes waiting; without any, what comes after the newest release --
if ($Suggest) {
    $tags = Tags
    $newest = Newest-Stable $tags
    if (-not $newest) { $newest = @{ Major = 0; Minor = 0; Patch = 0; Text = "0.0.0" } }
    $released = @($tags | ForEach-Object { $_.Text })

    # .docs/release-notes/vX.Y.Z.md not released yet, newer than the newest release, of the type picked.
    $ready = @(Get-ChildItem ".docs/release-notes" -Filter "v*.md" -ErrorAction SilentlyContinue |
        ForEach-Object { Parse-Version $_.BaseName } |
        Where-Object { $_ -and ($released -notcontains $_.Text) -and ((Base-Key $_) -gt (Base-Key $newest)) } |
        Where-Object { if ($Label -eq "prerelease") { [bool]$_.Suffix } else { -not $_.Suffix } } |
        Sort-Object { Base-Key $_ }, { $_.Text })
    if ($ready.Count) {
        foreach ($v in $ready) {
            $icon = if ($v.Suffix) { "`$(beaker)" } else { "`$(tag)" }
            Write-Output "$($v.Text)|$icon $($v.Text)|release notes ready|.docs/release-notes/v$($v.Text).md - or type another version"
        }
        exit 0
    }

    # No notes for this type yet: the next versions, said to have none.
    $bases = @(
        @{ Kind = "patch"; Detail = "fixes and small changes"; V = "$($newest.Major).$($newest.Minor).$($newest.Patch + 1)" },
        @{ Kind = "minor"; Detail = "new features, nothing removed"; V = "$($newest.Major).$($newest.Minor + 1).0" },
        @{ Kind = "major"; Detail = "changes that break something"; V = "$($newest.Major + 1).0.0" }
    )
    foreach ($b in $bases) {
        if ($Label -eq "prerelease") {
            # The next rcN for that version.
            $existing = @($tags | Where-Object { $_.Suffix -match "^rc(\d+)$" -and "$($_.Major).$($_.Minor).$($_.Patch)" -eq $b.V } |
                ForEach-Object { [int]($_.Suffix -replace "^rc", "") })
            $n = 1 + ($existing + 0 | Measure-Object -Maximum).Maximum
            $value = "$($b.V)-rc$n"
            Write-Output "$value|`$(beaker) $value|$($b.Kind) pre-release, no release notes yet|After v$($newest.Text): $($b.Detail) - or type another version"
        } else {
            Write-Output "$($b.V)|`$(tag) $($b.V)|$($b.Kind), no release notes yet|After v$($newest.Text): $($b.Detail) - or type another version"
        }
    }
    exit 0
}

# -- 3. commit: the newest pushed commits --
if ($Commits) {
    Git-Quiet fetch origin --quiet | Out-Null
    $upstream = Upstream
    $sep = [char]9
    foreach ($line in @(git log $upstream -n 30 "--format=%H%x09%h%x09%s%x09%an%x09%ar%x09%D")) {
        $f = $line -split $sep
        $subject = ($f[2] -replace "\|", "/")
        if ($subject.Length -gt 72) { $subject = $subject.Substring(0, 71) + "..." }
        $released = @(($f[5] -split ",\s*") | Where-Object { $_ -match "^tag: (v\S+)" } | ForEach-Object { $_ -replace "^tag: ", "" })
        $detail = if ($released.Count) { "Already released as $($released -join ', ')" } elseif ($f[5]) { $f[5] -replace "\|", "/" } else { "" }
        Write-Output "$($f[0])|`$(git-commit) $($f[1]) $subject|$($f[3]), $($f[4])|$detail"
    }
    exit 0
}

# -- 4. confirm: the release, or why it can't be made --
if ($Summary) {
    $issues = @(Problems $Label $Version $Commit)
    if ($issues.Count) {
        Write-Output "no|`$(error) Can't release - pick this to stop|$($issues[0])|$(($issues | Select-Object -Skip 1) -join ' ')"
        exit 0
    }
    $v = Parse-Version $Version
    $short = (git log -1 "--format=%h %s" $Commit) -replace "\|", "/"
    Write-Output "yes|`$(rocket) Release v$($v.Text)|$($labelNames[$Label]) - $short|$(Notes-Line $Version). GitHub builds, tests and publishes the installers."
    Write-Output "no|`$(close) Cancel|nothing is tagged or pushed|"
    exit 0
}

# -- Release --
if ($Confirm -ne "yes") {
    Write-Host "Cancelled - nothing was tagged or pushed."
    exit 0
}
$issues = @(Problems $Label $Version $Commit -redo:$Redo)
if ($issues.Count) {
    $issues | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    throw "The release can't be made - nothing was tagged or pushed."
}
$v = Parse-Version $Version
$tag = "v$($v.Text)"
$slug = Repo-Slug

if ($Redo) {
    $published = $true
    try {
        Invoke-RestMethod "https://api.github.com/repos/$slug/releases/tags/$tag" -Headers @{ "User-Agent" = "DnnManager-release" } | Out-Null
    } catch {
        # 404: no published release - the tag may move.
        $published = $false
    }
    if ($published) { throw "$tag is published already: a published release is never redone - pick a newer version." }
    Git-Quiet tag -d $tag | Out-Null
    Git-Quiet push origin ":refs/tags/$tag" | Out-Null
}

Write-Host "Release $tag ($($labelNames[$Label])) from $(git log -1 '--format=%h %s' $Commit)"
Write-Host (Notes-Line $Version)
git tag -a $tag $Commit -m "DNN Manager $($v.Text)" -m "Release-Label: $Label"
if ($LASTEXITCODE -ne 0) { throw "Tagging $tag failed." }
git push origin "refs/tags/$tag"
if ($LASTEXITCODE -ne 0) {
    git tag -d $tag | Out-Null
    throw "Pushing $tag failed - the local tag was removed again; nothing was released."
}
Write-Host ""
Write-Host "Pushed $tag. GitHub now builds and tests on every system, then publishes the installers:" -ForegroundColor Green
Write-Host "  https://github.com/$slug/actions/workflows/release.yml"
Write-Host "Approve the 'publish' environment there if it asks. If anything fails, nothing is published."
