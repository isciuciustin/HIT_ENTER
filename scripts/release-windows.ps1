# release-windows.ps1 <version> - the Slint window (M12) for Windows x64:
# a portable .zip (hit-enter.exe + the notices) in target/release-upload/,
# its checksum appended to SHA256SUMS. Run on Windows (docs/BUILD_WINDOWS.md).
#
# Not signed (no code-signing certificate): SmartScreen warns on first run
# ("More info" -> "Run anyway"), as the download page says.
param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$root = (Get-Location).Path
$home_ = $env:USERPROFILE

# No home paths in the binary (as the other scripts), and the C runtime
# linked in: a PC without the Visual C++ redistributable runs it too.
$env:RUSTFLAGS = "-C target-feature=+crt-static " +
  "--remap-path-prefix=$home_\.cargo\registry\src=/cargo " +
  "--remap-path-prefix=$home_\.cargo\git\checkouts=/cargo-git " +
  "--remap-path-prefix=$root=/hit-enter --remap-path-prefix=$home_=/home"
# libopus's assertions carry __FILE__: MSVC's -ffile-prefix-map is the
# (undocumented, long-standing) /d1trimfile, which cmake gets from CFLAGS.
$env:CFLAGS = "/d1trimfile:$home_\"
$env:CXXFLAGS = $env:CFLAGS
# audiopus_sys 0.2.2's bundled libopus asks for CMake < 3.5, which CMake 4
# no longer accepts without this.
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
# Cargo reports progress on stderr, which PowerShell 5.1 turns into errors
# under 'Stop' when output is redirected: the exit code decides instead.
$ErrorActionPreference = 'Continue'
cargo build --release --locked -p hit-desktop
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$ErrorActionPreference = 'Stop'

$bin = 'target\release\hit-enter.exe'
$bytes = [IO.File]::ReadAllBytes((Resolve-Path $bin))
foreach ($enc in [Text.Encoding]::ASCII, [Text.Encoding]::Unicode) {
  $text = $enc.GetString($bytes)
  if ($text.IndexOf($home_, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
    throw "the binary still names $home_ - not releasable"
  }
}
# Only Windows' own DLLs: anything else (vcruntime140, a vcpkg opus) is
# missing on someone's PC.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -property installationPath
$dumpbin = Get-ChildItem "$vs\VC\Tools\MSVC" -Recurse -Filter dumpbin.exe |
  Where-Object FullName -match 'Hostx64\\x64' | Select-Object -First 1
$dlls = & $dumpbin.FullName /nologo /dependents $bin | Where-Object { $_ -match '^\s+\S+\.dll$' } |
  ForEach-Object { $_.Trim() }
$foreign = $dlls | Where-Object { $_ -match '^(vcruntime|msvcp|ucrtbase|opus|libopus)' }
if ($foreign) { throw "links DLLs users may not have: $($foreign -join ', ')" }

$work = 'target\windows-pkg\HIT_ENTER'
if (Test-Path 'target\windows-pkg') { Remove-Item 'target\windows-pkg' -Recurse -Force }
New-Item -ItemType Directory $work | Out-Null
Copy-Item $bin "$work\hit-enter.exe"
Copy-Item THIRD_PARTY_NOTICES.md $work

$out = 'target\release-upload'
New-Item -ItemType Directory $out -Force | Out-Null
$zip = "$out\HIT_ENTER_${Version}_x64.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $work -DestinationPath $zip
# Always the current notices: a copy left by an older build is stale.
Copy-Item THIRD_PARTY_NOTICES.md $out -Force
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
# shasum's format, LF endings: one file for every platform's lines.
[IO.File]::AppendAllText((Join-Path $root "$out\SHA256SUMS"), "$hash  $(Split-Path $zip -Leaf)`n")
Get-ChildItem $out | Format-Table Name, Length
