# JSONL event log of the Windows conformance fixtures: the same records as
# libs/images/linux/fixtures/fixturelog.py, one object per line
# in $env:CUA_FIXTURE_LOG_DIR\<name>.jsonl ("ts", "mono", "fixture", "type"
# and the event's fields). Dot-source it, then call New-FixtureLog.
$script:FixtureClock = [Diagnostics.Stopwatch]::StartNew()

function New-FixtureLog([string]$Name) {
  $dir = $env:CUA_FIXTURE_LOG_DIR
  if (-not $dir) { $dir = Join-Path $env:TEMP "cua-fixtures" }
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  [pscustomobject]@{ Name = $Name; Path = (Join-Path $dir "$Name.jsonl") }
}

function Write-FixtureEvent($Log, [string]$Type, [System.Collections.IDictionary]$Fields = @{}) {
  $record = [ordered]@{
    ts      = [math]::Round([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() / 1000.0, 6)
    mono    = [math]::Round($script:FixtureClock.Elapsed.TotalSeconds, 6)
    fixture = $Log.Name
    type    = $Type
  }
  foreach ($k in $Fields.Keys) { $record[$k] = $Fields[$k] }
  $line = (ConvertTo-Json -InputObject $record -Compress -Depth 5) + "`n"
  # The doctor reads the log while the fixture writes it: retry briefly on a
  # sharing violation.
  for ($i = 0; $i -lt 20; $i++) {
    try { [IO.File]::AppendAllText($Log.Path, $line, [Text.UTF8Encoding]::new($false)); return }
    catch [IO.IOException] { Start-Sleep -Milliseconds 25 }
  }
}

function Get-FixtureMods {
  $m = [Windows.Forms.Control]::ModifierKeys
  $names = @()
  if ($m -band [Windows.Forms.Keys]::Shift) { $names += "shift" }
  if ($m -band [Windows.Forms.Keys]::Control) { $names += "ctrl" }
  if ($m -band [Windows.Forms.Keys]::Alt) { $names += "alt" }
  $b = [Windows.Forms.Control]::MouseButtons
  if ($b -band [Windows.Forms.MouseButtons]::Left) { $names += "button1" }
  if ($b -band [Windows.Forms.MouseButtons]::Middle) { $names += "button2" }
  if ($b -band [Windows.Forms.MouseButtons]::Right) { $names += "button3" }
  , $names
}
