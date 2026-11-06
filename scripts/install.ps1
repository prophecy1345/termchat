# termchat installer for Windows.
#
#   irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex
#
# Downloads the release archive, checks it against the release's SHA256SUMS,
# copies termchat.exe (and lyrebird.exe, for Tor bridges, ADR-015) to
# %LOCALAPPDATA%\Programs\termchat and adds that folder
# to your user PATH. Nothing else is changed.
#
# The desktop app (ADR-018) instead: set TERMCHAT_GUI=1 first.
#
#   $env:TERMCHAT_GUI = 1; irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex
#
# It puts termchat-gui.exe in the same folder and adds a Start menu entry.
#
# Settings (environment variables):
#   TERMCHAT_VERSION      release tag, e.g. v0.1.1 (default: latest)
#   TERMCHAT_BASE_URL     download from here instead of GitHub (mirrors, tests)
#   TERMCHAT_GUI          1: the desktop app instead of the terminal client
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repo = 'prophecy1345/termchat'
$tag = $env:TERMCHAT_VERSION
if (-not $tag) {
    $tag = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
}
$base = $env:TERMCHAT_BASE_URL
if (-not $base) { $base = "https://github.com/$repo/releases/download/$tag" }
$name = if ($env:TERMCHAT_GUI) { 'termchat-gui' } else { 'termchat' }
$archive = "$name-$tag-x86_64-pc-windows-msvc.zip"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("termchat-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "Downloading $archive"
    Invoke-WebRequest "$base/$archive" -OutFile (Join-Path $tmp $archive)
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS')

    # `*name` is how sha256sum marks binary mode on some systems.
    $line = Get-Content (Join-Path $tmp 'SHA256SUMS') | Where-Object { ($_ -split '\s+')[1].TrimStart('*') -eq $archive }
    if (-not $line) { throw "termchat: $archive is not listed in SHA256SUMS, nothing installed" }
    $expected = ($line -split '\s+')[0]
    $actual = (Get-FileHash (Join-Path $tmp $archive) -Algorithm SHA256).Hash
    if ($actual -ne $expected) { throw "termchat: checksum mismatch for $archive, nothing installed" }

    Expand-Archive (Join-Path $tmp $archive) -DestinationPath $tmp
    $dest = Join-Path $env:LOCALAPPDATA 'Programs\termchat'
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    $src = Join-Path $tmp "$name-$tag-x86_64-pc-windows-msvc"
    Copy-Item (Join-Path $src "$name.exe") $dest -Force
    # Releases before 0.3.0 have no lyrebird.
    $lyrebird = Join-Path $src 'lyrebird.exe'
    if (Test-Path $lyrebird) { Copy-Item $lyrebird $dest -Force }
    if ($env:TERMCHAT_GUI) {
        $menu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\termchat.lnk'
        $link = (New-Object -ComObject WScript.Shell).CreateShortcut($menu)
        $link.TargetPath = Join-Path $dest 'termchat-gui.exe'
        $link.Save()
        Write-Host "Installed termchat $tag to $dest\termchat-gui.exe, with a Start menu entry"
        return
    }

    $path = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not (($path -split ';') -contains $dest)) {
        $new = if ($path) { "$path;$dest" } else { $dest }
        [Environment]::SetEnvironmentVariable('Path', $new, 'User')
        Write-Host "Added $dest to your PATH. Open a new terminal, then run: termchat"
    } else {
        Write-Host "Run: termchat"
    }
    Write-Host "Installed termchat $tag to $dest\termchat.exe"
} finally {
    Remove-Item -Recurse -Force $tmp
}
