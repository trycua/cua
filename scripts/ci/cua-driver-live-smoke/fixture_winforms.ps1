# WinForms window for the cua-driver live smoke: a text box, two buttons, a
# drop-down-list combo box and an editable combo box, each with a label that
# echoes its state so a fresh snapshot can verify every action.
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.Application]::EnableVisualStyles()

$form = New-Object System.Windows.Forms.Form
$form.Text = "Cua Live Smoke"
$form.Width = 440
$form.Height = 860
$form.StartPosition = "Manual"
$form.Location = New-Object System.Drawing.Point(40, 40)

$panel = New-Object System.Windows.Forms.FlowLayoutPanel
$panel.Dock = "Fill"
$panel.FlowDirection = "TopDown"
$panel.WrapContents = $false
$form.Controls.Add($panel)

function Add-Label([string]$text) {
    $label = New-Object System.Windows.Forms.Label
    $label.Text = $text
    $label.AutoSize = $true
    $panel.Controls.Add($label)
    return $label
}

[void](Add-Label "Name field")
$entry = New-Object System.Windows.Forms.TextBox
$entry.Width = 300
$entry.AccessibleName = "Name field"
$panel.Controls.Add($entry)

$apply = New-Object System.Windows.Forms.Button
$apply.Text = "Apply"
$panel.Controls.Add($apply)
$status = Add-Label "Status: idle"
$apply.Add_Click({ $status.Text = "Applied: " + $entry.Text })

$increment = New-Object System.Windows.Forms.Button
$increment.Text = "Increment"
$panel.Controls.Add($increment)
$count = Add-Label "Count: 0"
$script:clicks = 0
$increment.Add_Click({
    $script:clicks += 1
    $count.Text = "Count: $($script:clicks)"
})

[void](Add-Label "Color")
$colorCombo = New-Object System.Windows.Forms.ComboBox
$colorCombo.DropDownStyle = "DropDownList"
$colorCombo.AccessibleName = "Color"
[void]$colorCombo.Items.AddRange(@("Red", "Green", "Blue"))
$colorCombo.SelectedIndex = 0
$panel.Controls.Add($colorCombo)
$color = Add-Label "Color: Red"
$colorCombo.Add_SelectedIndexChanged({ $color.Text = "Color: " + $colorCombo.Text })

[void](Add-Label "Size")
$sizeCombo = New-Object System.Windows.Forms.ComboBox
$sizeCombo.DropDownStyle = "DropDown"
$sizeCombo.AccessibleName = "Size"
[void]$sizeCombo.Items.AddRange(@("Small", "Medium", "Large"))
$sizeCombo.SelectedIndex = 0
$panel.Controls.Add($sizeCombo)
$size = Add-Label "Size: Small"
$sizeCombo.Add_TextChanged({ $size.Text = "Size: " + $sizeCombo.Text })

# Padding rows keep a one-row change small next to the whole tree, as in a
# real window, so `since` answers with a diff rather than a full read.
foreach ($row in 1..15) { [void](Add-Label "Row $row") }

[System.Windows.Forms.Application]::Run($form)
