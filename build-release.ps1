# Builds dist\EyeBreak.exe (and refreshes the exe icon/version resources
# when the Windows SDK + MSVC build tools are installed).
#   .\build-release.ps1            build only
#   .\build-release.ps1 -Install   build, then install/update on this PC
param([switch]$Install)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

# 1. Windows resources (icon + version) -> assets\resources.obj
$rc = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\rc.exe" -ErrorAction SilentlyContinue | Sort-Object FullName | Select-Object -Last 1
$cvtres = Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\bin\Hostx64\x64\cvtres.exe", "$env:ProgramFiles\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\bin\Hostx64\x64\cvtres.exe" -ErrorAction SilentlyContinue | Select-Object -Last 1
if ($rc -and $cvtres) {
    Push-Location assets
    & $rc.FullName /nologo /fo app.res app.rc
    & $cvtres.FullName /nologo /machine:x64 /out:resources.obj app.res | Out-Null
    Remove-Item app.res
    Pop-Location
} else {
    Write-Host "rc.exe/cvtres.exe not found: using the committed assets\resources.obj"
}

# 2. Build (target dir is outside this folder: the GNU toolchain's dlltool
#    breaks on paths with spaces; see .cargo\config.toml)
cargo build --release
$target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item (Join-Path $target 'release\EyeBreak.exe') dist\EyeBreak.exe -Force
Write-Host "Built dist\EyeBreak.exe"

# 3. Optional: install. The exe installs itself to %LOCALAPPDATA%\Programs\EyeBreak,
#    replaces a running older copy and registers itself to start with Windows.
if ($Install) { Start-Process dist\EyeBreak.exe }
