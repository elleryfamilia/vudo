# vudo installer for Windows.
#   irm https://raw.githubusercontent.com/elleryfamilia/vudo/main/install.ps1 | iex
#
# Env overrides:
#   $env:VUDO_VERSION       install a specific tag (default: latest)
#   $env:VUDO_INSTALL_DIR   where to put vudo.exe (default: %LOCALAPPDATA%\Programs\vudo)

$ErrorActionPreference = 'Stop'
try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 } catch {}

$Repo = 'elleryfamilia/vudo'
$InstallDir = if ($env:VUDO_INSTALL_DIR) { $env:VUDO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\vudo' }

$tag = $env:VUDO_VERSION
if (-not $tag) {
    $rel = Invoke-RestMethod -UseBasicParsing "https://api.github.com/repos/$Repo/releases/latest" `
        -Headers @{ 'User-Agent' = 'vudo-install' }
    $tag = $rel.tag_name
}
if (-not $tag) { throw 'could not determine the latest release' }

# Only an x86_64 build is published; it also runs on ARM64 via emulation.
$name = "vudo-$tag-x86_64-pc-windows-msvc"
$base = "https://github.com/$Repo/releases/download/$tag/$name.zip"

Write-Host "vudo: installing $tag"

$tmp = Join-Path $env:TEMP ('vudo-' + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp "$name.zip"
    Invoke-WebRequest -UseBasicParsing -Uri $base -OutFile $zip
    Invoke-WebRequest -UseBasicParsing -Uri "$base.sha256" -OutFile "$zip.sha256"

    $expected = (((Get-Content "$zip.sha256" -Raw) -split '\s+')[0]).ToLower()
    $actual = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    if ($expected -ne $actual) { throw "checksum mismatch (expected $expected, got $actual)" }

    Expand-Archive -Path $zip -DestinationPath $tmp -Force
    $exe = Join-Path $tmp "$name\vudo.exe"
    if (-not (Test-Path $exe)) { throw 'vudo.exe not found in the archive' }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $dest = Join-Path $InstallDir 'vudo.exe'

    # If the target is in use (e.g. `vudo --update` running the installer),
    # move it aside first: Windows allows renaming a running executable, but
    # not overwriting it in place.
    if (Test-Path $dest) {
        $old = "$dest.old"
        Remove-Item $old -Force -ErrorAction SilentlyContinue
        try { Move-Item $dest $old -Force } catch {}
    }
    Copy-Item $exe $dest -Force

    # Ensure the install dir is on the user's PATH.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $InstallDir) {
        $newPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        $env:Path = "$env:Path;$InstallDir"
        Write-Host "vudo: added $InstallDir to your PATH (restart your terminal to pick it up)"
    }

    Write-Host "vudo: installed to $dest"
    Write-Host "vudo: run 'vudo --help' to get started"
}
finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
