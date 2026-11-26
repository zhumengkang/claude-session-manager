param([Parameter(Mandatory=$true)][string]$ConfigPath, [Parameter(Mandatory=$true)][string]$Provider, [string]$SessionId = '', [string]$ArtifactName = '')
$ErrorActionPreference = 'Stop'
if ($Provider -notin @('claude', 'codex')) { throw 'Provider must be claude or codex' }
if (-not $ArtifactName) { $ArtifactName = $Provider }
if ($ArtifactName -notmatch '^[a-z][a-z0-9-]{0,63}$') { throw 'ArtifactName must be a bounded lowercase filename stem' }
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
$config = Get-Content $ConfigPath -Raw | ConvertFrom-Json
if ($SessionId) {
  $expected = @($config.sessions | Where-Object { $_.id -eq $SessionId -and $_.provider -eq $Provider } | ForEach-Object { $_.title })
  if ($expected.Count -ne 1) { throw 'Owned SessionId and Provider did not select exactly one tab' }
} else {
  $expected = @($config.sessions | Where-Object { $_.favorite -and $_.provider -eq $Provider } | ForEach-Object { $_.title })
}
if ($expected.Count -eq 0) { throw "No owned $Provider favorites to verify" }
$root = [System.Windows.Automation.AutomationElement]::RootElement
$walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
$tabs = @()
foreach ($title in $expected) {
  $condition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, [string]$title)
  $matches = @($root.FindAll([System.Windows.Automation.TreeScope]::Subtree, $condition) | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::TabItem })
  if ($matches.Count -ne 1) { throw "Expected one native tab for $title; found $($matches.Count)" }
  $tab = $matches[0]
  $window = $tab
  while ($window -and $window.Current.ControlType -ne [System.Windows.Automation.ControlType]::Window) { $window = $walker.GetParent($window) }
  if (-not $window -or $window.Current.NativeWindowHandle -eq 0) { throw 'Owned native window not found' }
  $process = Get-Process -Id $window.Current.ProcessId -ErrorAction Stop
  if ($process.ProcessName -ne 'WindowsTerminal') { throw 'Matching tab does not belong to Windows Terminal' }
  $tabs += [pscustomobject]@{ Title = $title; RuntimeId = @($tab.GetRuntimeId()); Hwnd = $window.Current.NativeWindowHandle; WindowProcessId = $window.Current.ProcessId }
}
$handles = @($tabs | Select-Object -ExpandProperty Hwnd -Unique)
if ($handles.Count -ne 1) { throw "Expected one native window; found $($handles.Count)" }
$hwnd = [int]$handles[0]
$windowCondition = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NativeWindowHandleProperty, $hwnd)
$window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCondition)
$allTabs = @($window.FindAll([System.Windows.Automation.TreeScope]::Descendants, (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::TabItem))))
if ($allTabs.Count -ne $expected.Count) { throw "Owned window contains $($allTabs.Count) tabs; expected exactly $($expected.Count)" }
foreach ($tab in $allTabs) { if (-not $tab.Current.Name.StartsWith([string]$config.runId)) { throw 'Owned window contains an untrusted tab; refusing validation' } }
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CsmOwnedWindowCapture {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdc, uint flags);
}
'@
$rect = New-Object CsmOwnedWindowCapture+RECT
if (-not [CsmOwnedWindowCapture]::GetWindowRect([IntPtr]$hwnd, [ref]$rect)) { throw 'GetWindowRect failed' }
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
$bitmap = New-Object System.Drawing.Bitmap($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$hdc = $graphics.GetHdc()
try { $captured = [CsmOwnedWindowCapture]::PrintWindow([IntPtr]$hwnd, $hdc, 2) } finally { $graphics.ReleaseHdc($hdc); $graphics.Dispose() }
if (-not $captured) { $bitmap.Dispose(); throw 'PrintWindow failed for owned window' }
$imagePath = Join-Path $config.root "terminal-tabs-$ArtifactName.png"
$bitmap.Save($imagePath, [System.Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose()
$result = [pscustomobject]@{ Provider = $Provider; NativeWindowCount = 1; TabCount = $tabs.Count; Hwnd = $hwnd; WindowProcessId = $tabs[0].WindowProcessId; Tabs = $tabs; ImagePath = $imagePath }
$result | ConvertTo-Json -Depth 6 -Compress
Set-Content -Path (Join-Path $config.root "native-validation-$ArtifactName.json") -Value ($result | ConvertTo-Json -Depth 6) -Encoding utf8
