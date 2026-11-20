using System;
using System.IO;
using System.Windows.Forms;

// A classic WinForms provider synchronously handles Invoke until ShowDialog
// returns. Its journal is an independent oracle, not a driver success marker.
internal static class Program
{
    [STAThread]
    private static void Main(string[] args)
    {
        Application.EnableVisualStyles();
        int normal = 0, opened = 0, closed = 0, mouseDown = 0, toggled = 0, selected = 0, expanded = 0, fallback = 0;
        string journal = args[0];
        Action publish = () => File.WriteAllText(journal,
            "normal=" + normal + "\nopened=" + opened + "\nclosed=" + closed + "\nmouse_down=" + mouseDown + "\ntoggled=" + toggled + "\nselected=" + selected + "\nexpanded=" + expanded + "\nfallback=" + fallback + "\n");
        var main = new Form { Text = "CUA semantic deadline fixture", Width = 440, Height = 340 };
        var ordinary = new Button { Text = "Normal semantic action", Left = 30, Top = 30, Width = 340 };
        var opener = new Button { Text = "Open synchronous modal", Left = 30, Top = 85, Width = 340 };
        var checkbox = new CheckBox { Text = "Toggle semantic action", Left = 30, Top = 135, Width = 340 };
        checkbox.CheckedChanged += (s, e) => { toggled++; publish(); };
        var radio = new RadioButton { Text = "Select semantic action", Left = 30, Top = 170, Width = 340 };
        radio.CheckedChanged += (s, e) => { if (radio.Checked) selected++; publish(); };
        var combo = new ComboBox { AccessibleName = "Expand semantic action", Left = 30, Top = 205, Width = 340, DropDownStyle = ComboBoxStyle.DropDownList };
        combo.Items.AddRange(new object[] { "Choice A", "Choice B" });
        combo.DropDown += (s, e) => { expanded++; publish(); };
        var edit = new TextBox { AccessibleName = "Coordinate fallback", Text = "Native edit", Left = 30, Top = 250, Width = 340 };
        edit.MouseDown += (s, e) => { fallback++; publish(); };
        ordinary.Click += (s, e) => { normal++; publish(); };
        ordinary.MouseDown += (s, e) => { mouseDown++; publish(); };
        opener.MouseDown += (s, e) => { mouseDown++; publish(); };
        opener.Click += (s, e) =>
        {
            opened++;
            publish();
            using (var modal = new Form { Text = "CUA synchronous modal", Width = 300, Height = 150 })
            {
                var cancel = new Button { Text = "Cancel modal", Left = 50, Top = 40, Width = 180, DialogResult = DialogResult.Cancel };
                modal.Controls.Add(cancel);
                modal.CancelButton = cancel;
                modal.ShowDialog(main);
            }
            closed++;
            publish();
        };
        main.Controls.AddRange(new Control[] { ordinary, opener, checkbox, radio, combo, edit });
        publish();
        Application.Run(main);
    }
}
