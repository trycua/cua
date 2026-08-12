using System;
using System.Collections.Generic;
using System.Drawing;
using System.Windows.Forms;

// "Meridian CRM — Account Record" : WinForms node (classic Win32 controls,
// MSAA/UIA). Explicit fractional layout (dense, no gaps beyond borders).
// cua-driver drives the Account Name field + the "Add Record" button.
static class Program
{
    [STAThread]
    static void Main()
    {
        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);

        var f = new Form { Text = "Meridian CRM — Account Record [WinForms]", ClientSize = new Size(960, 600),
            StartPosition = FormStartPosition.Manual, Font = new Font("Segoe UI", 9f), BackColor = Color.FromArgb(240,240,240) };

        Label band(string t, Color bg, ContentAlignment a = ContentAlignment.MiddleLeft, FontStyle fs = FontStyle.Regular)
            => new Label { Text = t, BackColor = bg, TextAlign = a, Font = new Font("Segoe UI", 9f, fs) };

        var menu = band("  File      Edit      View      Record      Tools      Help", Color.FromArgb(247,247,247));
        var tool = band("  New      Open      Save      Delete       │       ◀ Prev      Next ▶       │       Find", Color.FromArgb(235,235,235));
        var nameLbl = band(" Account Name", Color.FromArgb(240,240,240));
        var typeLbl = band(" Account Type", Color.FromArgb(240,240,240));
        var regionLbl = band(" Region", Color.FromArgb(240,240,240));
        var prioLbl = band(" Priority (1-5)", Color.FromArgb(240,240,240));
        var creditLbl = band(" Credit Limit", Color.FromArgb(240,240,240));
        var nameBox = new TextBox { BorderStyle = BorderStyle.FixedSingle };
        var typeCmb = new ComboBox { DropDownStyle = ComboBoxStyle.DropDownList, FlatStyle = FlatStyle.Flat };
        typeCmb.Items.AddRange(new object[] { "Enterprise", "SMB", "Government", "Reseller" }); typeCmb.SelectedIndex = 0;
        var regionCmb = new ComboBox { DropDownStyle = ComboBoxStyle.DropDownList, FlatStyle = FlatStyle.Flat };
        regionCmb.Items.AddRange(new object[] { "North", "South", "EMEA", "APAC", "LATAM" }); regionCmb.SelectedIndex = 0;
        var prio = new TrackBar { Minimum = 1, Maximum = 5, Value = 3, TickStyle = TickStyle.BottomRight };
        var credit = new TrackBar { Minimum = 0, Maximum = 100, Value = 40, TickFrequency = 10, TickStyle = TickStyle.BottomRight };
        var saveBtn = new Button { Text = "Add Record", FlatStyle = FlatStyle.System, Font = new Font("Segoe UI", 10f, FontStyle.Bold) };
        var sigHdr = band(" Signature / Notes", Color.FromArgb(225,225,225), ContentAlignment.MiddleLeft, FontStyle.Bold);
        var sig = new Panel { BackColor = Color.White, BorderStyle = BorderStyle.FixedSingle };
        var grid = new DataGridView { AllowUserToAddRows = false, ReadOnly = true, BackgroundColor = Color.White,
            BorderStyle = BorderStyle.FixedSingle, RowHeadersVisible = false, AllowUserToResizeRows = false,
            ColumnHeadersHeightSizeMode = DataGridViewColumnHeadersHeightSizeMode.DisableResizing };
        grid.Columns.Add("acct", "Account"); grid.Columns.Add("type", "Type"); grid.Columns.Add("region", "Region");
        grid.Columns.Add("pri", "Pri"); grid.Columns.Add("credit", "Credit");
        grid.Columns["acct"].AutoSizeMode = DataGridViewAutoSizeColumnMode.Fill;
        var status = band("  Ready", Color.FromArgb(232,232,232));
        var recLbl = band("Records: 0   USER: SYSTEM ▌ CONNECTED  ", Color.FromArgb(232,232,232), ContentAlignment.MiddleRight);

        // line tool: one straight segment per press-drag-release (down→up)
        var segs = new List<(Point a, Point b)>(); Point? down = null; Point cur = Point.Empty;
        sig.MouseDown += (s, e) => { down = e.Location; cur = e.Location; };
        sig.MouseMove += (s, e) => { if (down != null) { cur = e.Location; sig.Invalidate(); } };
        sig.MouseUp += (s, e) => { if (down != null) { segs.Add((down.Value, e.Location)); down = null; sig.Invalidate(); } };
        sig.Paint += (s, e) => { e.Graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
            using var p = new Pen(Color.MidnightBlue, 2); foreach (var sg in segs) e.Graphics.DrawLine(p, sg.a, sg.b);
            if (down != null) { using var pp = new Pen(Color.FromArgb(110, 25, 25, 112), 1); e.Graphics.DrawLine(pp, down.Value, cur); } };

        int records = 0;
        saveBtn.Click += (s, e) =>
        {
            string nm = nameBox.Text.Trim(); if (nm.Length == 0) nm = "(unnamed)";
            grid.Rows.Add(nm, typeCmb.Text, regionCmb.Text, prio.Value, "$" + (credit.Value * 1000));
            if (grid.Rows.Count > 0) grid.FirstDisplayedScrollingRowIndex = grid.Rows.Count - 1;
            records++; status.Text = $"  Saved account '{nm}'."; recLbl.Text = $"Records: {records}   USER: SYSTEM ▌ CONNECTED  ";
            nameBox.Clear();
        };

        foreach (Control c in new Control[] { menu, tool, nameLbl, typeLbl, regionLbl, prioLbl, creditLbl,
            nameBox, typeCmb, regionCmb, prio, credit, saveBtn, sigHdr, sig, grid, status, recLbl })
            f.Controls.Add(c);

        void Layout()
        {
            int W = f.ClientSize.Width, H = f.ClientSize.Height;
            int X(double a) => (int)(W * a); int Y(double a) => (int)(H * a);
            Rectangle R(double x0, double y0, double x1, double y1) => Rectangle.FromLTRB(X(x0), Y(y0), X(x1), Y(y1));
            menu.Bounds = R(0, 0, 1, 0.045);
            tool.Bounds = R(0, 0.045, 1, 0.095);
            double lc = 0.13; // label/control split
            nameLbl.Bounds = R(0, 0.105, lc, 0.16);   nameBox.Bounds = R(lc, 0.105, 0.42, 0.16);
            typeLbl.Bounds = R(0, 0.165, lc, 0.22);    typeCmb.Bounds = R(lc, 0.167, 0.42, 0.22);
            regionLbl.Bounds = R(0, 0.225, lc, 0.28);  regionCmb.Bounds = R(lc, 0.227, 0.42, 0.28);
            prioLbl.Bounds = R(0, 0.285, lc, 0.345);   prio.Bounds = R(lc, 0.285, 0.42, 0.345);
            creditLbl.Bounds = R(0, 0.35, lc, 0.41);   credit.Bounds = R(lc, 0.35, 0.42, 0.41);
            saveBtn.Bounds = R(0.04, 0.45, 0.40, 0.52);
            sigHdr.Bounds = R(0.42, 0.095, 1, 0.135);
            sig.Bounds = R(0.42, 0.135, 0.995, 0.42);
            grid.Bounds = R(0.005, 0.55, 0.995, 0.93);
            status.Bounds = R(0, 0.94, 0.5, 1);
            recLbl.Bounds = R(0.5, 0.94, 1, 1);
            float gfs = Math.Max(8, H / 70f);
            grid.Font = new Font("Segoe UI", gfs);
            grid.ColumnHeadersDefaultCellStyle.Font = new Font("Segoe UI", gfs, FontStyle.Bold);
        }
        f.Load += (s, e) => Layout();
        f.Resize += (s, e) => Layout();
        Application.Run(f);
    }
}
