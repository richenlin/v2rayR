$ErrorActionPreference = 'Stop'
$dist = Join-Path $PSScriptRoot '..\dist'
$msi = @(Get-ChildItem -LiteralPath $dist -Filter '*.msi')
$nsis = @(Get-ChildItem -LiteralPath $dist -Filter '*.exe')
if ($msi.Count -ne 1 -or $nsis.Count -ne 1) {
    throw 'Expected one MSI and one NSIS setup executable'
}

$root = Join-Path $env:ProgramFiles 'v2rayR'
$helper = Join-Path $root 'windows-tun\v2rayr-helper.exe'
$dll = Join-Path $root 'windows-tun\wintun.dll'

function Invoke-Installer([string] $File, [string] $Arguments) {
    $process = Start-Process -FilePath $File -ArgumentList $Arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "$File exited with code $($process.ExitCode)"
    }
}

function Assert-Bundle {
    & $helper tun-verify-install-v2
    if ($LASTEXITCODE -ne 0) {
        throw 'Installed TUN bundle did not pass path, ACL and hash validation'
    }
}

$package = '"' + $msi[0].FullName + '"'
$msiexec = Join-Path $env:SystemRoot 'System32\msiexec.exe'
Invoke-Installer $msiexec "/i $package /qn /norestart"
Assert-Bundle
# Verify the runtime rejects a damaged bundle, then exercise Windows Installer repair.
[IO.File]::WriteAllBytes($dll, [byte[]]@(0))
try {
    & $helper tun-verify-install-v2 2>$null
    if ($LASTEXITCODE -eq 0) { throw 'Damaged Wintun DLL passed validation' }
} finally {
    Invoke-Installer $msiexec "/fa $package /qn /norestart"
}
Assert-Bundle
Invoke-Installer $msiexec "/x $package /qn /norestart"
Invoke-Installer $nsis[0].FullName '/S'
Assert-Bundle
Invoke-Installer (Join-Path $root 'uninstall.exe') '/S'
