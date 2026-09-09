# Shoots every widget and state the feature page shows.
#
# Each shot is its own overlay run against a throwaway APPDATA, so the config
# it needs (which panels are visible, where, danger marks, theme) can be
# written fresh and can never touch the real race-overlay.toml. The overlay's
# own --screenshot flag is the only capture route that sees a layered GL
# window; see OverlayApp::take_screenshot. Panels are parked near the top-left
# so the crop below lands on them.
#
# Usage:  powershell -ExecutionPolicy Bypass -File docs/features/shoot.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$exe = Join-Path $root 'target/release/race-overlay.exe'
$outDir = Join-Path $PSScriptRoot 'img'
$scratch = Join-Path $env:TEMP 'race-overlay-shots'

if (-not (Test-Path $exe)) { throw "build the release binary first: cargo build --release --bin race-overlay" }
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
Add-Type -AssemblyName System.Drawing

# Every panel, and the config table each one hangs off.
$panels = @{
  relative    = 'relative'
  standings   = 'standings'
  radar       = 'radar'
  fasterclass = 'faster_class'
  pitstall    = 'pit_stall'
}

function Write-ShotConfig {
  param([string]$Visible, [string]$PanelLines = '', [string]$Tables = '', [string]$Top = '', [double]$Scale = 1.0)
  $lines = New-Object System.Collections.Generic.List[string]
  $lines.Add('only_show_when_iracing_focused = false')
  $lines.Add('hide_in_garage = false')
  if ($Top) { $lines.Add($Top) }
  foreach ($key in $panels.Values) {
    $on = if ($key -eq $panels[$Visible]) { 'true' } else { 'false' }
    $lines.Add("[$key]")
    $lines.Add("visible = $on")
    $lines.Add('pos = [60.0, 60.0]')
    # Always a decimal point: these fields are f32, and TOML `1` is an integer
    # the parser refuses -- which drops the whole file back to defaults.
    $lines.Add("scale = $($Scale.ToString('0.0#', [System.Globalization.CultureInfo]::InvariantCulture))")
    # Per-panel overrides go inside that panel's own table; a second [table]
    # header for the same name is a TOML error, and the config load that fails
    # takes every panel back to visible-by-default.
    if ($on -eq 'true' -and $PanelLines) { $lines.Add($PanelLines) }
    $lines.Add('')
  }
  if ($Tables) { $lines.Add($Tables) }
  # The settings live in the APPDATA subfolder named by
  # config::CONFIG_DIR_NAME, which is 'race' -- not 'race-overlay'.
  $dir = Join-Path $scratch 'race'
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  # No BOM: the TOML parser refuses one, which silently resets every setting.
  [System.IO.File]::WriteAllText((Join-Path $dir 'race-overlay.toml'), ($lines -join "`n"),
    (New-Object System.Text.UTF8Encoding($false)))
}

function Invoke-Trim {
  param([string]$Path)
  # The frame is monitor-sized and composited on black, so crop to the drawn
  # pixels; the page wants the widget, not a wall of background.
  $bmp = New-Object System.Drawing.Bitmap($Path)
  $w = $bmp.Width; $h = $bmp.Height
  $rectAll = New-Object System.Drawing.Rectangle(0, 0, $w, $h)
  $data = $bmp.LockBits($rectAll, [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
    [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $stride = $data.Stride
  $bytes = New-Object byte[] ($stride * $h)
  [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
  $bmp.UnlockBits($data)

  $minX = $w; $minY = $h; $maxX = -1; $maxY = -1
  for ($y = 0; $y -lt $h; $y++) {
    $row = $y * $stride
    for ($x = 0; $x -lt $w; $x++) {
      $i = $row + $x * 4
      # Anything above near-black counts as drawn.
      if ($bytes[$i] -gt 14 -or $bytes[$i + 1] -gt 14 -or $bytes[$i + 2] -gt 14) {
        if ($x -lt $minX) { $minX = $x }
        if ($x -gt $maxX) { $maxX = $x }
        if ($y -lt $minY) { $minY = $y }
        if ($y -gt $maxY) { $maxY = $y }
      }
    }
  }
  if ($maxX -lt 0) { $bmp.Dispose(); Write-Host '  (nothing drawn)'; return }

  $pad = 14
  $minX = [Math]::Max(0, $minX - $pad); $minY = [Math]::Max(0, $minY - $pad)
  $maxX = [Math]::Min($w - 1, $maxX + $pad); $maxY = [Math]::Min($h - 1, $maxY + $pad)
  $rect = New-Object System.Drawing.Rectangle($minX, $minY, ($maxX - $minX + 1), ($maxY - $minY + 1))
  $crop = $bmp.Clone($rect, $bmp.PixelFormat)
  $bmp.Dispose()
  $crop.Save($Path + '.tmp', [System.Drawing.Imaging.ImageFormat]::Png)
  $crop.Dispose()
  Move-Item -Force ($Path + '.tmp') $Path
  Write-Host "  $($rect.Width)x$($rect.Height)"
}

function Invoke-Shot {
  param([string]$Name, [string]$Panel, [string]$Page = '', [string]$State = '',
        [string]$PanelLines = '', [string]$Tables = '', [string]$Top = '', [double]$Scale = 1.0)
  Write-Host "shooting $Name"
  Write-ShotConfig -Visible $Panel -PanelLines $PanelLines -Tables $Tables -Top $Top -Scale $Scale
  $png = Join-Path $outDir "$Name.png"
  if (Test-Path $png) { Remove-Item $png }
  $log = Join-Path $scratch 'run.log'
  $shotArgs = @('--demo', "--screenshot=$png")
  if ($Page) { $shotArgs += "--demo-page=$Page" }
  if ($State) { $shotArgs += "--demo-state=$State" }
  $env:APPDATA = $scratch
  $proc = Start-Process -FilePath $exe -ArgumentList $shotArgs -PassThru -WindowStyle Hidden -RedirectStandardOutput $log
  if (-not $proc.WaitForExit(30000)) { $proc.Kill(); Write-Host '  (timed out)'; return }
  Start-Sleep -Milliseconds 200
  # A config this run could not read is the one failure that looks like
  # success: every panel comes back visible and the shot is of all of them.
  if ((Test-Path $log) -and (Select-String -Path $log -SimpleMatch 'using default overlay settings' -Quiet)) {
    Write-Warning "  $Name : the settings file was not read -- the shot shows every panel"
  }
  if (Test-Path $png) { Invoke-Trim -Path $png } else { Write-Host '  (no file)' }
}

$realAppData = $env:APPDATA
try {
  # --- Relative, and the status border around it ------------------------
  Invoke-Shot -Name 'relative-default'    -Panel relative
  Invoke-Shot -Name 'relative-caution'    -Panel relative -State 'caution'
  Invoke-Shot -Name 'relative-lastlap'    -Panel relative -State 'lastlap'
  Invoke-Shot -Name 'relative-finish'     -Panel relative -State 'finish'
  Invoke-Shot -Name 'relative-box'        -Panel relative -State 'box'
  Invoke-Shot -Name 'relative-spectating' -Panel relative -State 'spectating'
  Invoke-Shot -Name 'relative-teammate'   -Panel relative -State 'teammate'
  Invoke-Shot -Name 'relative-grid'       -Panel relative -State 'grid'
  Invoke-Shot -Name 'relative-danger'     -Panel relative -Tables "[danger]`n1000 = 'severe'`n1002 = 'warning'`n1004 = 'caution'"
  Invoke-Shot -Name 'relative-fueltarget' -Panel relative -State 'spectating,sync'
  Invoke-Shot -Name 'relative-minimal'    -Panel relative -PanelLines "show_car_number = false`nshow_brand = false`nshow_recent_lap = false`nshow_irating = false"
  Invoke-Shot -Name 'relative-racechange' -Panel relative -PanelLines "position_change = 'race'"
  Invoke-Shot -Name 'relative-narrow'     -Panel relative -PanelLines "width = 520.0`nahead_count = 2`nbehind_count = 2"

  # --- Black box pages ---------------------------------------------------
  Invoke-Shot -Name 'blackbox-fuel'         -Panel relative -Page fuel
  Invoke-Shot -Name 'blackbox-fuel-auto'    -Panel relative -Page fuel -Tables "[blackbox]`nauto_fuel = true`nfuel_margin_laps = 1.5"
  Invoke-Shot -Name 'blackbox-tires'        -Panel relative -Page tires -State 'worntyres'
  Invoke-Shot -Name 'blackbox-tires-wear'   -Panel relative -Page tires -State 'worntyres' -Tables "[blackbox]`ntyre_bars = 'wear'"
  Invoke-Shot -Name 'blackbox-strategy'     -Panel relative -Page strategy -State 'needsstops'
  Invoke-Shot -Name 'blackbox-strategy-crew' -Panel relative -Page strategy -State 'needsstops,spectating,sync'
  Invoke-Shot -Name 'blackbox-incar'        -Panel relative -Page in-car
  Invoke-Shot -Name 'blackbox-weather'      -Panel relative -Page weather
  Invoke-Shot -Name 'blackbox-fuel-crew'    -Panel relative -Page fuel -State 'spectating,sync'
  Invoke-Shot -Name 'blackbox-tires-crew'   -Panel relative -Page tires -State 'spectating,sync'

  # --- The other panels ---------------------------------------------------
  Invoke-Shot -Name 'standings-default'   -Panel standings
  Invoke-Shot -Name 'standings-tyres'     -Panel standings -PanelLines "show_tyres = true`nshow_stint_laps = true"
  Invoke-Shot -Name 'standings-endurance' -Panel standings -PanelLines "endurance_mode = 'on'"
  Invoke-Shot -Name 'standings-oneclass'  -Panel standings -PanelLines "show_other_classes = false"
  Invoke-Shot -Name 'radar-default'       -Panel radar -Scale 0.6
  Invoke-Shot -Name 'radar-numbers'       -Panel radar -Scale 0.6 -PanelLines "show_numbers = true"
  Invoke-Shot -Name 'fasterclass-default' -Panel fasterclass
  Invoke-Shot -Name 'pitstall-approach'   -Panel pitstall -State 'pitroad'
  Invoke-Shot -Name 'pitstall-inbox'      -Panel pitstall -State 'inbox'

  # --- Themes --------------------------------------------------------------
  Invoke-Shot -Name 'theme-panel'      -Panel relative -Top "theme = 'panel'"
  Invoke-Shot -Name 'theme-instrument' -Panel relative -Top "theme = 'instrument'"
}
finally {
  $env:APPDATA = $realAppData
}
Write-Host "done -> $outDir"
