using System;
using System.Drawing;
using System.Windows.Forms;

// WinForms copy of the shared "legacy form" — classic Win32-backed controls
// (MSAA/UIA exposed). A normal app; cua-driver drives it like any other.
static class Program
{
    [STAThread]
    static void Main()
    {
        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);

        var f = new Form
        {
            Text = "WinForms (.NET classic)",
            ClientSize = new Size(480, 300),
            FormBorderStyle = FormBorderStyle.FixedSingle,
            MaximizeBox = false,
            StartPosition = FormStartPosition.Manual,
        };

        var title = new Label
        {
            Text = "WinForms (.NET classic)",
            Bounds = new Rectangle(0, 8, 480, 28),
            TextAlign = ContentAlignment.MiddleCenter,
            Font = new Font("Segoe UI", 11f, FontStyle.Bold),
        };
        var nameLbl = new Label { Text = "Name:", Bounds = new Rectangle(20, 72, 60, 20) };
        var box = new TextBox { Bounds = new Rectangle(90, 70, 300, 24) };
        var btn = new Button { Text = "SUBMIT", Bounds = new Rectangle(160, 150, 160, 46) };
        var status = new Label { Text = "Clicks: 0    Last: (none)", Bounds = new Rectangle(20, 220, 440, 24) };

        int clicks = 0;
        btn.Click += (s, e) =>
        {
            clicks++;
            string last = box.Text.Length == 0 ? "(none)" : box.Text;
            status.Text = $"Clicks: {clicks}    Last: {last}";
        };

        f.Controls.Add(title);
        f.Controls.Add(nameLbl);
        f.Controls.Add(box);
        f.Controls.Add(btn);
        f.Controls.Add(status);
        Application.Run(f);
    }
}
