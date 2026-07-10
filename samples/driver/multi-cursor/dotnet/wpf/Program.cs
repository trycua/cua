using System;
using System.Collections.ObjectModel;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Media;
using System.Windows.Shapes;

// "Meridian CRM — Account Record" : WPF node (XAML / UIA). Real Slider /
// ComboBox / InkCanvas (doodle) / DataGrid (spreadsheet). Account Name TextBox
// + SAVE button are what cua-driver drives.
class Program
{
    public class RowVM
    {
        public string Account { get; set; } public string Type { get; set; }
        public string Region { get; set; } public int Pri { get; set; } public string Credit { get; set; }
    }

    [STAThread]
    static void Main()
    {
        var app = new Application();
        var rows = new ObservableCollection<RowVM>();
        var gray = new SolidColorBrush(Color.FromRgb(0xE0, 0xE0, 0xE0));

        // menu
        var menu = new Menu();
        foreach (var m in new[] { "File", "Edit", "View", "Record", "Tools", "Help" })
            menu.Items.Add(new MenuItem { Header = m });
        // toolbar
        var tray = new ToolBarTray();
        var tb = new ToolBar();
        foreach (var t in new[] { "New", "Open", "Save", "Delete", "|", "◀ Prev", "Next ▶", "|", "Find" })
            tb.Items.Add(t == "|" ? (object)new Separator() : new Button { Content = t, Padding = new Thickness(6, 1, 6, 1) });
        tray.ToolBars.Add(tb);

        // ── left form (dense grid; cell borders only) ───────────────────────
        var form = new Grid { Background = Brushes.White };
        form.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(120) });
        form.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        TextBlock lbl(string t) => new TextBlock { Text = " " + t, VerticalAlignment = VerticalAlignment.Center, Background = new SolidColorBrush(Color.FromRgb(0xF0, 0xF0, 0xF0)) };
        int r = 0;
        void AddRow(string label, FrameworkElement c, double h = 30)
        {
            form.RowDefinitions.Add(new RowDefinition { Height = new GridLength(h) });
            var b1 = new Border { BorderBrush = gray, BorderThickness = new Thickness(0, 0, 1, 1), Child = lbl(label) };
            Grid.SetRow(b1, r); Grid.SetColumn(b1, 0); form.Children.Add(b1);
            var b2 = new Border { BorderBrush = gray, BorderThickness = new Thickness(0, 0, 0, 1), Child = c };
            Grid.SetRow(b2, r); Grid.SetColumn(b2, 1); form.Children.Add(b2);
            r++;
        }
        var nameBox = new TextBox { BorderThickness = new Thickness(0), VerticalContentAlignment = VerticalAlignment.Center };
        var typeCmb = new ComboBox { BorderThickness = new Thickness(0) };
        foreach (var s in new[] { "Enterprise", "SMB", "Government", "Reseller" }) typeCmb.Items.Add(s); typeCmb.SelectedIndex = 0;
        var regionCmb = new ComboBox { BorderThickness = new Thickness(0) };
        foreach (var s in new[] { "North", "South", "EMEA", "APAC", "LATAM" }) regionCmb.Items.Add(s); regionCmb.SelectedIndex = 0;
        var priority = new Slider { Minimum = 1, Maximum = 5, Value = 3, TickFrequency = 1, IsSnapToTickEnabled = true, TickPlacement = TickPlacement.BottomRight, VerticalAlignment = VerticalAlignment.Center };
        var credit = new Slider { Minimum = 0, Maximum = 100, Value = 40, TickFrequency = 10, TickPlacement = TickPlacement.BottomRight, VerticalAlignment = VerticalAlignment.Center };
        AddRow("Account Name", nameBox);
        AddRow("Account Type", typeCmb);
        AddRow("Region", regionCmb);
        AddRow("Priority (1-5)", priority);
        AddRow("Credit Limit", credit);
        var saveBtn = new Button { Content = "Add Record", FontWeight = FontWeights.Bold };
        form.RowDefinitions.Add(new RowDefinition { Height = new GridLength(40) });
        Grid.SetRow(saveBtn, r); Grid.SetColumn(saveBtn, 0); Grid.SetColumnSpan(saveBtn, 2); form.Children.Add(saveBtn);

        // ── right top: line-tool sketch (one straight segment per drag) ─────
        var ink = new Canvas { Background = Brushes.White, ClipToBounds = true };
        Point? dn = null; Line preview = null;
        ink.MouseLeftButtonDown += (s, e) => { dn = e.GetPosition(ink); };
        ink.MouseMove += (s, e) => { if (dn != null) { var p = e.GetPosition(ink);
            if (preview == null) { preview = new Line { Stroke = Brushes.MidnightBlue, StrokeThickness = 1, Opacity = 0.45 }; ink.Children.Add(preview); }
            preview.X1 = dn.Value.X; preview.Y1 = dn.Value.Y; preview.X2 = p.X; preview.Y2 = p.Y; } };
        ink.MouseLeftButtonUp += (s, e) => { if (dn != null) { var p = e.GetPosition(ink);
            ink.Children.Add(new Line { Stroke = Brushes.MidnightBlue, StrokeThickness = 2, X1 = dn.Value.X, Y1 = dn.Value.Y, X2 = p.X, Y2 = p.Y });
            if (preview != null) { ink.Children.Remove(preview); preview = null; }
            dn = null; } };
        var sigHdr = new TextBlock { Text = " Signature / Notes", FontWeight = FontWeights.Bold, Background = new SolidColorBrush(Color.FromRgb(0xE1,0xE1,0xE1)), Padding = new Thickness(2) };
        var sigDock = new DockPanel(); DockPanel.SetDock(sigHdr, Dock.Top); sigDock.Children.Add(sigHdr); sigDock.Children.Add(ink);

        // ── right bottom: spreadsheet ───────────────────────────────────────
        var dg = new DataGrid { AutoGenerateColumns = false, ItemsSource = rows, IsReadOnly = true, GridLinesVisibility = DataGridGridLinesVisibility.All, HeadersVisibility = DataGridHeadersVisibility.Column };
        dg.Columns.Add(new DataGridTextColumn { Header = "Account", Binding = new System.Windows.Data.Binding("Account"), Width = new DataGridLength(1, DataGridLengthUnitType.Star) });
        dg.Columns.Add(new DataGridTextColumn { Header = "Type", Binding = new System.Windows.Data.Binding("Type") });
        dg.Columns.Add(new DataGridTextColumn { Header = "Region", Binding = new System.Windows.Data.Binding("Region") });
        dg.Columns.Add(new DataGridTextColumn { Header = "Pri", Binding = new System.Windows.Data.Binding("Pri") });
        dg.Columns.Add(new DataGridTextColumn { Header = "Credit", Binding = new System.Windows.Data.Binding("Credit") });

        var rightGrid = new Grid();
        rightGrid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(150) });
        rightGrid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(3) });
        rightGrid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        Grid.SetRow(sigDock, 0); rightGrid.Children.Add(sigDock);
        var gs2 = new GridSplitter { Height = 3, HorizontalAlignment = HorizontalAlignment.Stretch, Background = gray }; Grid.SetRow(gs2, 1); rightGrid.Children.Add(gs2);
        Grid.SetRow(dg, 2); rightGrid.Children.Add(dg);

        var main = new Grid();
        main.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(380) });
        main.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(3) });
        main.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        Grid.SetColumn(form, 0); main.Children.Add(form);
        var gs1 = new GridSplitter { Width = 3, Background = gray }; Grid.SetColumn(gs1, 1); main.Children.Add(gs1);
        Grid.SetColumn(rightGrid, 2); main.Children.Add(rightGrid);

        // status bar
        var statusBar = new StatusBar();
        var statusLbl = new StatusBarItem { Content = "Ready" };
        var recLbl = new StatusBarItem { Content = "Records: 0", HorizontalAlignment = HorizontalAlignment.Right };
        statusBar.Items.Add(statusLbl);
        statusBar.Items.Add(new StatusBarItem { Content = "USER: SYSTEM  ▌ CONNECTED", HorizontalAlignment = HorizontalAlignment.Right });
        statusBar.Items.Add(recLbl);

        int records = 0;
        saveBtn.Click += (s, e) =>
        {
            string nm = (nameBox.Text ?? "").Trim(); if (nm.Length == 0) nm = "(unnamed)";
            rows.Add(new RowVM { Account = nm, Type = typeCmb.Text, Region = regionCmb.Text, Pri = (int)priority.Value, Credit = "$" + ((int)credit.Value * 1000) });
            records++; recLbl.Content = $"Records: {records}"; statusLbl.Content = $"Saved account '{nm}'.";
            nameBox.Text = "";
        };

        var dock = new DockPanel();
        DockPanel.SetDock(menu, Dock.Top); DockPanel.SetDock(tray, Dock.Top); DockPanel.SetDock(statusBar, Dock.Bottom);
        dock.Children.Add(menu); dock.Children.Add(tray); dock.Children.Add(statusBar); dock.Children.Add(main);

        var win = new Window { Title = "Meridian CRM — Account Record [WPF]", Width = 960, Height = 600, Content = dock, WindowStartupLocation = WindowStartupLocation.Manual };
        app.Run(win);
    }
}
