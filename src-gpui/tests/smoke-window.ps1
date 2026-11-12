param([Parameter(Mandatory)][string]$Executable)
$ErrorActionPreference = 'Stop'
$smokeProfile = Join-Path ([IO.Path]::GetTempPath()) ("voice-hub-smoke-" + [Guid]::NewGuid())
$previousLocalAppData = $env:LOCALAPPDATA
$smokeProcess = $null
try {
  New-Item -ItemType Directory -Path $smokeProfile | Out-Null
  $env:LOCALAPPDATA = $smokeProfile
  $smokeProcess = Start-Process -FilePath (Resolve-Path $Executable).Path -PassThru
  $opened = $false
  for ($attempt = 0; $attempt -lt 30; $attempt++) {
    Start-Sleep -Milliseconds 500
    $smokeProcess.Refresh()
    if ($smokeProcess.HasExited) { throw "GPUI exited during startup: $($smokeProcess.ExitCode)" }
    if ($smokeProcess.MainWindowTitle -eq 'Voice Hub — GPUI') { $opened = $true; break }
  }
  if (-not $opened) { throw 'GPUI did not open its native window' }
  Start-Sleep -Seconds 3
  $smokeProcess.Refresh()
  if ($smokeProcess.HasExited) { throw "GPUI exited after opening its window: $($smokeProcess.ExitCode)" }
  Write-Output 'Native GPUI window opened and stayed alive.'
} finally {
  if ($null -ne $smokeProcess -and -not $smokeProcess.HasExited) { Stop-Process -Id $smokeProcess.Id -Force }
  $env:LOCALAPPDATA = $previousLocalAppData
  if (Test-Path $smokeProfile) { Remove-Item $smokeProfile -Recurse -Force -ErrorAction SilentlyContinue }
}
