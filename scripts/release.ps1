# Starts a release (the VS Code task "Release: Github"). The task asks four things, each a picker filled by this
# script, then runs it to tag and push:
#
#   1. type      -Label latest | prerelease | none           (how GitHub labels the release)
#   2. version   -Suggest -Label <type>                       the versions with release notes not published yet;
#                                                             any other can be typed
#   3. commit    -Commits                                     the newest commits on the branch's remote
#   4. confirm   -Summary -Label -Version -Commit             the release spelled out, or why it can't be made
#
# Then:  release.ps1 -Label <type> -Version <x.y.z[-suffix]> -Commit <sha> -Confirm yes
# checks everything again, tags the commit vX.Y.Z (an annotated tag whose message carries "Release-Label: <type>") and
# pushes only the tag. GitHub (.github/workflows/release.yml) then waits for CI on that commit (CI is the test - a
# commit whose CI failed is refused here already), packages the installers, and publishes the release only when all of
# that passed - nothing built on this PC is uploaded.
#
# A version counts as released only when GitHub has a published release for it. A tag without one (a failed or
# cancelled run) is free again: releasing that version moves the tag to the commit picked.
param(
    [ValidateSet("latest", "prerelease", "none")][string]$Label = "latest",
    [string]$Version = "",
    [string]$Commit = "",
    [string]$Confirm = "",
    [switch]$Suggest,
    [switch]$Commits,
    [switch]$Summary
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

# The published releases on GitHub as versions; $null when GitHub can't be asked. Drafts aren't listed without a
# token, and a tag whose run failed has no release - neither counts as released.
function Released {
    try {
        $list = Invoke-RestMethod "https://api.github.com/repos/$(Repo-Slug)/releases?per_page=100" `
            -Headers @{ "User-Agent" = "DnnManager-release" }
    } catch {
        return $null
    }
    return @($list | Where-Object { -not $_.draft } | ForEach-Object { Parse-Version $_.tag_name } | Where-Object { $_ })
}

# Whether the tag exists here or on GitHub.
function Tag-Exists([string]$tag) {
    $local = git tag --list $tag
    $remote = Git-Quiet ls-remote --tags origin "refs/tags/$tag"
    return [bool]($local -or $remote)
}

function Newest-Stable($tags) {
    return $tags | Where-Object { -not $_.Suffix } | Sort-Object { Base-Key $_ } | Select-Object -Last 1
}

# CI (.github/workflows/ci.yml) on the commit, from GitHub: @{ State = passed | running | failed | none | unknown;
# Url }. The release workflow waits for it and makes nothing unless it passed.
function Ci-State([string]$commit) {
    try {
        $runs = Invoke-RestMethod "https://api.github.com/repos/$(Repo-Slug)/actions/workflows/ci.yml/runs?head_sha=$commit&event=push&per_page=50" `
            -Headers @{ "User-Agent" = "DnnManager-release" }
    } catch {
        return @{ State = "unknown"; Url = "" }
    }
    $run = @($runs.workflow_runs | Sort-Object created_at) | Select-Object -Last 1
    if (-not $run) { return @{ State = "none"; Url = "" } }
    if ($run.status -ne "completed") { return @{ State = "running"; Url = $run.html_url } }
    if ($run.conclusion -eq "success") { return @{ State = "passed"; Url = $run.html_url } }
    return @{ State = "failed"; Url = $run.html_url; Conclusion = $run.conclusion }
}

# Everything that would make the release wrong - empty when it can be made.
function Problems([string]$label, [string]$version, [string]$commit) {
    $issues = @()
    if ($version -eq "no-notes") {
        return @("No release notes for this type yet: write .docs/release-notes/vX.Y.Z.md first, or type a version.")
    }
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
    $released = Released
    if ($null -eq $released) {
        return @("GitHub can't be reached to see what is released - try again in a minute.")
    }
    $newest = Newest-Stable $released
    if ($newest -and (Base-Key $v) -le (Base-Key $newest)) {
        $issues += "v$($newest.Text) is released already: the version must be newer."
    }
    if (@($released | ForEach-Object { $_.Text }) -contains $v.Text) {
        $issues += "v$($v.Text) is published already: a published release is never redone - pick a newer version."
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
            if ($LASTEXITCODE -ne 0) {
                $issues += "Commit $commit isn't on $upstream - push it first."
            } else {
                $ci = Ci-State (git rev-parse $commit)
                if ($ci.State -eq "failed") {
                    $issues += "CI on this commit ended '$($ci.Conclusion)' - fix it or re-run it first: $($ci.Url)"
                } elseif ($ci.State -eq "none") {
                    $issues += "CI hasn't run on this commit - push it to a branch and let CI pass first."
                }
            }
        }
    }
    return $issues
}

function Notes-Line([string]$version) {
    $notes = ".docs/release-notes/v$((Parse-Version $version).Text).md"
    if (Test-Path $notes) { return "Notes: $notes" }
    return "No $notes - GitHub writes the notes from the commits"
}

# -- 2. version: the versions with release notes waiting (only those - nothing is guessed) --
if ($Suggest) {
    # Published releases; when GitHub can't be asked, the tags (a failed run's tag then looks released).
    $tags = Released
    if ($null -eq $tags) { $tags = @(Tags) }
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
            $detail = ".docs/release-notes/v$($v.Text).md - or type another version"
            if (Tag-Exists "v$($v.Text)") { $detail = "Its tag exists without a release: it moves to the commit you pick" }
            Write-Output "$($v.Text)|$icon $($v.Text)|release notes ready|$detail"
        }
        exit 0
    }

    # No notes for this type: nothing is guessed. A version can still be typed.
    $kind = if ($Label -eq "prerelease") { "pre-release, e.g. v2.1.0-rc1.md" } else { "release, e.g. v2.1.0.md" }
    Write-Output "no-notes|`$(info) No release notes for a $($labelNames[$Label]) release|write them first in .docs/release-notes/|One file per $kind - or type a version here"
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
    $ci = Ci-State (git rev-parse $Commit)
    $ciText = switch ($ci.State) {
        "passed" { "CI passed" }
        "running" { "CI still running - the release waits for it" }
        default { "CI state unknown - the release checks it" }
    }
    $moves = if (Tag-Exists "v$($v.Text)") { " The tag v$($v.Text) (no release) moves to this commit." } else { "" }
    Write-Output "yes|`$(rocket) Release v$($v.Text)|$($labelNames[$Label]) - $short|$ciText.$moves $(Notes-Line $Version). GitHub packages and publishes the installers once CI passed."
    Write-Output "no|`$(close) Cancel|nothing is tagged or pushed|"
    exit 0
}

# -- Release --
if ($Confirm -ne "yes") {
    Write-Host "Cancelled - nothing was tagged or pushed."
    exit 0
}
$issues = @(Problems $Label $Version $Commit)
if ($issues.Count) {
    $issues | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    throw "The release can't be made - nothing was tagged or pushed."
}
$v = Parse-Version $Version
$tag = "v$($v.Text)"
$slug = Repo-Slug

# A tag without a published release (Problems refused a published one): move it to the commit picked.
if (Tag-Exists $tag) {
    Write-Host "$tag exists without a published release - moving it to the commit picked."
    Git-Quiet tag -d $tag | Out-Null
    Git-Quiet push origin ":refs/tags/$tag" | Out-Null
    if (Git-Quiet ls-remote --tags origin "refs/tags/$tag") { throw "Removing the old $tag from GitHub failed - nothing was released." }
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
Write-Host "Pushed $tag. GitHub waits for CI on the commit, then packages and publishes the installers:" -ForegroundColor Green
Write-Host "  https://github.com/$slug/actions/workflows/release.yml"
Write-Host "Approve the 'publish' environment there if it asks. If anything fails, nothing is published."
