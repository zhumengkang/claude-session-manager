param([Parameter(Mandatory=$true)][string]$ConfigPath)
$ErrorActionPreference = 'Stop'
$config = Get-Content $ConfigPath -Raw | ConvertFrom-Json
$stopped = 0
$daemonStopped = 0
$interactiveStopped = 0
$entries = @()
function Test-OwnedProvider($entry, $process) {
  if ($null -eq $process) { return $false }
  $command = [string]$process.CommandLine
  if (-not $command.Contains($config.providerScript)) { return $false }
  if ($entry.daemon -eq $true) {
    # The daemon executable itself is unique to this fixture root. Older daemon
    # logs did not put the session ID in argv; the exact owned path, script and
    # app-server role still prevent matching any real Codex service.
    return $process.ExecutablePath -eq $config.daemonExe -and $command.Contains('app-server')
  }
  return $process.ExecutablePath -eq $config.nodeExe -and $command.Contains($entry.sessionId)
}
if (Test-Path $config.logPath) {
  $entries = @(Get-Content $config.logPath | ForEach-Object { $_ | ConvertFrom-Json })
  foreach ($entry in $entries) {
    $process = Get-CimInstance Win32_Process -Filter "ProcessId=$($entry.pid)" -ErrorAction SilentlyContinue
    if ($null -eq $process) { continue }
    if (-not (Test-OwnedProvider $entry $process)) {
      throw "Refusing to stop unverified PID $($entry.pid)"
    }
    Stop-Process -Id $entry.pid -Force -ErrorAction Stop
    $stopped++
    if ($entry.daemon -eq $true) { $daemonStopped++ } else { $interactiveStopped++ }
  }
}
$remaining = @()
for ($attempt = 0; $attempt -lt 50; $attempt++) {
  $remaining = @($entries | Where-Object {
    $process = Get-CimInstance Win32_Process -Filter "ProcessId=$($_.pid)" -ErrorAction SilentlyContinue
    return Test-OwnedProvider $_ $process
  })
  if ($remaining.Count -eq 0) { break }
  Start-Sleep -Milliseconds 100
}
if ($remaining.Count -ne 0) { throw "Owned provider processes remain after cleanup: $($remaining.Count)" }
"OWNED_PROVIDER_PROCESSES_STOPPED=$stopped"
"OWNED_DAEMON_PROCESSES_STOPPED=$daemonStopped"
"OWNED_INTERACTIVE_PROVIDER_PROCESSES_STOPPED=$interactiveStopped"
"OWNED_PROVIDER_PROCESSES_REMAINING=$($remaining.Count)"
