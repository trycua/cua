# Form fixture (WinForms port of libs/images/linux/fixtures/form.py): a
# "Name" text box, a multi-line "Notes" box, a "Subscribe" check box, a
# "Color" combo box, a "Submit" button and a status label, each with its
# UI Automation name. Logs every change (fixturelog.ps1); "Submit" logs the
# complete form state. `cua-spacesd doctor` types into Name and presses
# Submit through the accessibility tree.
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
. (Join-Path $PSScriptRoot "fixturelog.ps1")
[Windows.Forms.Application]::EnableVisualStyles()

$name = if ($env:CUA_FIXTURE_NAME) { $env:CUA_FIXTURE_NAME } else { "form" }
$title = if ($env:CUA_FORM_TITLE) { $env:CUA_FORM_TITLE } else { "CUA Fixture Form" }
$log = New-FixtureLog $name

$form = New-Object Windows.Forms.Form
$form.Text = $title
$form.StartPosition = "Manual"
$form.Location = New-Object Drawing.Point 780, 60
$form.ClientSize = New-Object Drawing.Size 440, 360
$form.KeyPreview = $true

function Add-Label([string]$text, [int]$y) {
  $l = New-Object Windows.Forms.Label
  $l.Text = $text; $l.Location = New-Object Drawing.Point 16, ($y + 4); $l.AutoSize = $true
  $form.Controls.Add($l)
}
Add-Label "Name" 16
$entry = New-Object Windows.Forms.TextBox
$entry.Name = "Name"; $entry.AccessibleName = "Name"
$entry.Location = New-Object Drawing.Point 110, 16; $entry.Width = 300
$form.Controls.Add($entry)

Add-Label "Notes" 56
$notes = New-Object Windows.Forms.TextBox
$notes.Name = "Notes"; $notes.AccessibleName = "Notes"; $notes.Multiline = $true
$notes.Location = New-Object Drawing.Point 110, 56; $notes.Size = New-Object Drawing.Size 300, 90
$form.Controls.Add($notes)

$check = New-Object Windows.Forms.CheckBox
$check.Name = "Subscribe"; $check.Text = "Subscribe"; $check.AccessibleName = "Subscribe"
$check.Location = New-Object Drawing.Point 110, 160; $check.AutoSize = $true
$form.Controls.Add($check)

Add-Label "Color" 196
$combo = New-Object Windows.Forms.ComboBox
$combo.Name = "Color"; $combo.AccessibleName = "Color"; $combo.DropDownStyle = "DropDownList"
[void]$combo.Items.AddRange(@("red", "green", "blue"))
$combo.SelectedIndex = 0
$combo.Location = New-Object Drawing.Point 110, 196; $combo.Width = 160
$form.Controls.Add($combo)

$button = New-Object Windows.Forms.Button
$button.Name = "Submit"; $button.Text = "Submit"; $button.AccessibleName = "Submit"
$button.Location = New-Object Drawing.Point 110, 240; $button.AutoSize = $true
$form.Controls.Add($button)

$status = New-Object Windows.Forms.Label
$status.Name = "Status"; $status.Text = "ready"; $status.AccessibleName = "Status"
$status.Location = New-Object Drawing.Point 110, 290; $status.AutoSize = $true
$form.Controls.Add($status)

function Form-State {
  [ordered]@{ name = $entry.Text; notes = $notes.Text; subscribe = [bool]$check.Checked; color = [string]$combo.SelectedItem }
}

$entry.Add_TextChanged({ Write-FixtureEvent $log "entry_changed" ([ordered]@{ widget = "Name"; text = $entry.Text }) })
$entry.Add_KeyDown({ param($s, $e)
    if ($e.KeyCode -eq "Return") { Write-FixtureEvent $log "entry_activate" ([ordered]@{ widget = "Name"; text = $entry.Text }) } })
$notes.Add_TextChanged({ Write-FixtureEvent $log "notes_changed" ([ordered]@{ widget = "Notes"; text = $notes.Text }) })
$check.Add_CheckedChanged({ Write-FixtureEvent $log "toggled" ([ordered]@{ widget = "Subscribe"; active = [bool]$check.Checked }) })
$combo.Add_SelectedIndexChanged({ Write-FixtureEvent $log "combo_changed" ([ordered]@{ widget = "Color"; value = [string]$combo.SelectedItem }) })
$button.Add_Click({
    $status.Text = "submitted"
    Write-FixtureEvent $log "submit" (Form-State) })
$form.Add_KeyDown({ param($s, $e)
    $focus = $form.ActiveControl
    Write-FixtureEvent $log "key_press" ([ordered]@{ keyval = [int]$e.KeyValue; key = $e.KeyCode.ToString(); text = ""
        keycode = [int]$e.KeyValue; focus = $(if ($focus) { $focus.Name } else { $null }) }) })
$form.Add_MouseDown({ param($s, $e)
    $p = $form.PointToScreen((New-Object Drawing.Point $e.X, $e.Y))
    Write-FixtureEvent $log "button_press" ([ordered]@{ button = 1; x = [double]$e.X; y = [double]$e.Y; x_root = [double]$p.X; y_root = [double]$p.Y }) })
$form.Add_Activated({ Write-FixtureEvent $log "focus_in" @{} })
$form.Add_Deactivate({ Write-FixtureEvent $log "focus_out" @{} })
$form.Add_FormClosed({ Write-FixtureEvent $log "exit" @{} })

$timer = New-Object Windows.Forms.Timer
$timer.Interval = 300
$timer.Add_Tick({
    $timer.Stop()
    $f = [ordered]@{ pid = $PID; title = $title; wm_class = "cua-fixture-$name" }
    foreach ($kv in (Form-State).GetEnumerator()) { $f[$kv.Key] = $kv.Value }
    Write-FixtureEvent $log "ready" $f
  })
$form.Add_Shown({ $timer.Start() })
[Windows.Forms.Application]::Run($form)
