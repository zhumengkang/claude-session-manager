param([Parameter(Mandatory=$true)][string]$ConfigPath, [string]$SessionId = '')
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$config = Get-Content $ConfigPath -Raw | ConvertFrom-Json
$targetTitle = $null
if ($SessionId) {
  $ownedSessions = @($config.sessions | Where-Object { $_.id -eq $SessionId })
  if ($ownedSessions.Count -ne 1) { throw 'Owned SessionId was not found exactly once' }
  $targetTitle = [string]$ownedSessions[0].title
}
function Get-OwnedTabs($root, $condition) {
  $tabs = @($root.FindAll([System.Windows.Automation.TreeScope]::Subtree, $condition) | Where-Object { $_.Current.Name.StartsWith([string]$config.runId) })
  if ($targetTitle) { return @($tabs | Where-Object { $_.Current.Name -eq $targetTitle }) }
  return $tabs
}
$root = [System.Windows.Automation.AutomationElement]::RootElement
$tabCondition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::TabItem)
$buttonCondition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)
$walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
$closed = 0
for ($attempt = 0; $attempt -lt 100; $attempt++) {
  $owned = @(Get-OwnedTabs $root $tabCondition)
  if ($owned.Count -eq 0) { break }
  $tab = $owned[0]
  $window = $tab
  while ($window -and $window.Current.ControlType -ne [System.Windows.Automation.ControlType]::Window) { $window = $walker.GetParent($window) }
  if (-not $window) { throw 'Owned tab has no parent native window; left untouched' }
  if ((Get-Process -Id $window.Current.ProcessId -ErrorAction SilentlyContinue).ProcessName -ne 'WindowsTerminal') {
    throw 'Owned tab does not belong to Windows Terminal; left untouched'
  }
  # Never close the parent window: it may contain pre-existing user tabs. The
  # unique child button belongs only to this randomly titled test tab.
  $buttons = @($tab.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition))
  if ($buttons.Count -ne 1) { throw "Owned tab lacks exactly one child close button: $($tab.Current.Name)" }
  $invoke = $buttons[0].GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern) -as [System.Windows.Automation.InvokePattern]
  if (-not $invoke) { throw 'Owned tab child close button has no InvokePattern' }
  $invoke.Invoke()
  $closed++
  Start-Sleep -Milliseconds 600
}
$remaining = @(Get-OwnedTabs $root $tabCondition)
if ($remaining.Count -ne 0) { throw "Owned Terminal tabs remain after cleanup: $($remaining.Count)" }
"OWNED_TERMINAL_TABS_CLOSED=$closed"
"OWNED_TERMINAL_TABS_REMAINING=0"
