# Grid fixture (WinForms port of libs/images/linux/fixtures/grid.py): a
# COLS x ROWS grid of CELL px cells, cell (col, row) painted
# (col*255//(cols-1), row*255//(rows-1), blue), filling the client area.
# Logs pointer, wheel, key, focus and move events with the cell under the
# pointer (fixturelog.ps1). `cua-spacesd doctor` finds it on screen by colour
# and checks where its clicks, scrolls and drags land.
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
. (Join-Path $PSScriptRoot "fixturelog.ps1")
[Windows.Forms.Application]::EnableVisualStyles()

$name = if ($env:CUA_FIXTURE_NAME) { $env:CUA_FIXTURE_NAME } else { "grid" }
$cols = if ($env:CUA_GRID_COLS) { [int]$env:CUA_GRID_COLS } else { 8 }
$rows = if ($env:CUA_GRID_ROWS) { [int]$env:CUA_GRID_ROWS } else { 6 }
$cell = if ($env:CUA_GRID_CELL) { [int]$env:CUA_GRID_CELL } else { 80 }
$blue = if ($env:CUA_GRID_BLUE) { [int]$env:CUA_GRID_BLUE } else { 128 }
$title = if ($env:CUA_GRID_TITLE) { $env:CUA_GRID_TITLE } else { "CUA Fixture Grid" }
$log = New-FixtureLog $name

$bitmap = New-Object Drawing.Bitmap ($cols * $cell), ($rows * $cell)
$g = [Drawing.Graphics]::FromImage($bitmap)
for ($r = 0; $r -lt $rows; $r++) {
  for ($c = 0; $c -lt $cols; $c++) {
    $color = [Drawing.Color]::FromArgb([int][math]::Floor($c * 255 / [math]::Max($cols - 1, 1)),
      [int][math]::Floor($r * 255 / [math]::Max($rows - 1, 1)), $blue)
    $brush = New-Object Drawing.SolidBrush $color
    $g.FillRectangle($brush, $c * $cell, $r * $cell, $cell, $cell)
    $brush.Dispose()
  }
}
$g.Dispose()

$form = New-Object Windows.Forms.Form
$form.Text = $title
$form.FormBorderStyle = "FixedSingle"
$form.MaximizeBox = $false
$form.StartPosition = "Manual"
$form.Location = New-Object Drawing.Point 60, 60
$form.ClientSize = New-Object Drawing.Size ($cols * $cell), ($rows * $cell)
$form.BackgroundImage = $bitmap
$form.BackgroundImageLayout = "None"
$form.KeyPreview = $true
$form.AccessibleName = "$title canvas"

function Cell-At([int]$x, [int]$y) {
  $c = [math]::Floor($x / $cell); $r = [math]::Floor($y / $cell)
  if ($c -ge 0 -and $c -lt $cols -and $r -ge 0 -and $r -lt $rows) { return , @([int]$c, [int]$r) }
  return $null
}
function Pointer($e) {
  $screen = $form.PointToScreen((New-Object Drawing.Point $e.X, $e.Y))
  [ordered]@{ x = [double]$e.X; y = [double]$e.Y; x_root = [double]$screen.X; y_root = [double]$screen.Y
    cell = (Cell-At $e.X $e.Y); mods = (Get-FixtureMods) }
}
function Button-Number($b) {
  switch ($b.ToString()) { "Left" { 1 } "Middle" { 2 } "Right" { 3 } default { 0 } }
}

$form.Add_MouseDown({ param($s, $e)
    $f = Pointer $e; $f["button"] = (Button-Number $e.Button); $f["click_count"] = [int]$e.Clicks
    Write-FixtureEvent $log "button_press" $f })
$form.Add_MouseUp({ param($s, $e)
    $f = Pointer $e; $f["button"] = (Button-Number $e.Button); $f["click_count"] = 0
    Write-FixtureEvent $log "button_release" $f })
$form.Add_MouseWheel({ param($s, $e)
    $f = Pointer $e
    $f["direction"] = if ($e.Delta -gt 0) { "up" } else { "down" }
    $f["dx"] = 0.0; $f["dy"] = [math]::Round(- $e.Delta / 120.0, 3)
    Write-FixtureEvent $log "scroll" $f })
$form.Add_MouseEnter({ Write-FixtureEvent $log "enter" @{} })
$form.Add_MouseLeave({ Write-FixtureEvent $log "leave" @{} })
$form.Add_KeyDown({ param($s, $e)
    Write-FixtureEvent $log "key_press" ([ordered]@{ keyval = [int]$e.KeyValue; key = $e.KeyCode.ToString(); text = ""; keycode = [int]$e.KeyValue; mods = (Get-FixtureMods) }) })
$form.Add_KeyUp({ param($s, $e)
    Write-FixtureEvent $log "key_release" ([ordered]@{ keyval = [int]$e.KeyValue; key = $e.KeyCode.ToString(); text = ""; keycode = [int]$e.KeyValue; mods = (Get-FixtureMods) }) })
$form.Add_Activated({ Write-FixtureEvent $log "focus_in" @{} })
$form.Add_Deactivate({ Write-FixtureEvent $log "focus_out" @{} })
$form.Add_Move({ Write-FixtureEvent $log "configure" ([ordered]@{ x = $form.Left; y = $form.Top; width = $form.Width; height = $form.Height }) })
$form.Add_FormClosed({ Write-FixtureEvent $log "exit" @{} })

$timer = New-Object Windows.Forms.Timer
$timer.Interval = 300
$timer.Add_Tick({
    $timer.Stop()
    $origin = $form.PointToScreen((New-Object Drawing.Point 0, 0))
    Write-FixtureEvent $log "ready" ([ordered]@{ pid = $PID; title = $title; wm_class = "cua-fixture-$name"
        hwnd = [int64]$form.Handle; origin = @($origin.X, $origin.Y); cols = $cols; rows = $rows; cell = $cell; blue = $blue
        color_formula = "r=col*255//(cols-1), g=row*255//(rows-1), b=blue" })
  })
$form.Add_Shown({ $form.Activate(); $timer.Start() })
[Windows.Forms.Application]::Run($form)
