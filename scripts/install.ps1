# Installs geneva for the current user on Windows.
#
# From inside an unpacked release zip, installs the geneva.exe next to this
# script:
#
#   powershell -ExecutionPolicy Bypass -File install.ps1
#
# Otherwise downloads the release first:
#
#   irm https://raw.githubusercontent.com/geneva-render/geneva/main/scripts/install.ps1 | iex
#
# Options (environment variables):
#   GENEVA_VERSION   release tag to download, for example v1.0.0 (default: latest)
#   GENEVA_PREFIX    folder to install into (default: %LOCALAPPDATA%\Programs\geneva),
#                    added to the user's PATH when it is not on it
#   GITHUB_TOKEN     token with access to the repository, needed to download
#                    while the repository is private
$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1 does not offer TLS 1.2 by default.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$repo = 'geneva-render/geneva'
$api = "https://api.github.com/repos/$repo"
$target = 'x86_64-pc-windows-gnu'
$prefix = if ($env:GENEVA_PREFIX) { $env:GENEVA_PREFIX } else { Join-Path $env:LOCALAPPDATA 'Programs\geneva' }

function Install-Binary([string]$exe) {
  New-Item -ItemType Directory -Force -Path $prefix | Out-Null
  $installed = Join-Path $prefix 'geneva.exe'
  Copy-Item $exe $installed -Force
  # A browser download carries the mark of the web, which SmartScreen
  # checks; the installed copy starts without it.
  Unblock-File $installed -ErrorAction SilentlyContinue
  Write-Host "installed $(& $installed --version) to $installed"
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (-not (($userPath -split ';') -contains $prefix)) {
    $joined = (@($userPath.TrimEnd(';'), $prefix) | Where-Object { $_ }) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $joined, 'User')
    Write-Host "added $prefix to your PATH; open a new terminal to use it"
  }
}

# Inside an unpacked zip there is nothing to download.
if ($PSScriptRoot -and (Test-Path (Join-Path $PSScriptRoot 'geneva.exe'))) {
  Install-Binary (Join-Path $PSScriptRoot 'geneva.exe')
  return
}

$headers = @{ 'User-Agent' = 'geneva-install' }
if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }
$version = $env:GENEVA_VERSION
if (-not $version) {
  $version = (Invoke-RestMethod -UseBasicParsing -Headers $headers "$api/releases/latest").tag_name
  if (-not $version) { throw 'install.ps1: could not find the latest release (set GITHUB_TOKEN for a private repository)' }
}
$name = "geneva-$version-$target"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  $zip = Join-Path $tmp "$name.zip"
  Write-Host "downloading $name.zip"
  if ($env:GITHUB_TOKEN) {
    # Private repositories serve assets through the API only.
    $release = Invoke-RestMethod -UseBasicParsing -Headers $headers "$api/releases/tags/$version"
    $asset = $release.assets | Where-Object { $_.name -eq "$name.zip" } | Select-Object -First 1
    if (-not $asset) { throw "install.ps1: no $name.zip in release $version" }
    Invoke-WebRequest -UseBasicParsing -Headers ($headers + @{ Accept = 'application/octet-stream' }) $asset.url -OutFile $zip
  } else {
    Invoke-WebRequest -UseBasicParsing -Headers $headers "https://github.com/$repo/releases/download/$version/$name.zip" -OutFile $zip
  }
  Expand-Archive $zip -DestinationPath $tmp
  Install-Binary (Join-Path $tmp "$name\geneva.exe")
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
