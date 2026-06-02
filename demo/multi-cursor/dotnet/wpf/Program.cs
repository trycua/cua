using System;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

// WPF copy of the shared "legacy form" — XAML/UIA. Built in code (no XAML file)
// for a single-file project. A normal app; cua-driver drives it via UIA Invoke.
class Program
{
    [STAThread]
    static void Main()
    {
        var app = new Application();
        var canvas = new Canvas { Background = Brushes.WhiteSmoke };

        var title = new TextBlock
        {
            Text = "WPF (XAML / UIA)",
            FontSize = 16,
            FontWeight = FontWeights.Bold,
            Width = 480,
            TextAlignment = TextAlignment.Center,
        };
        Canvas.SetLeft(title, 0); Canvas.SetTop(title, 10);

        var nameLbl = new TextBlock { Text = "Name:" };
        Canvas.SetLeft(nameLbl, 20); Canvas.SetTop(nameLbl, 74);

        var box = new TextBox { Width = 300, Height = 24 };
        Canvas.SetLeft(box, 90); Canvas.SetTop(box, 70);

        var btn = new Button { Content = "SUBMIT", Width = 160, Height = 46 };
        Canvas.SetLeft(btn, 160); Canvas.SetTop(btn, 150);

        var status = new TextBlock { Text = "Clicks: 0    Last: (none)" };
        Canvas.SetLeft(status, 20); Canvas.SetTop(status, 220);

        int clicks = 0;
        btn.Click += (s, e) =>
        {
            clicks++;
            string last = string.IsNullOrEmpty(box.Text) ? "(none)" : box.Text;
            status.Text = $"Clicks: {clicks}    Last: {last}";
        };

        canvas.Children.Add(title);
        canvas.Children.Add(nameLbl);
        canvas.Children.Add(box);
        canvas.Children.Add(btn);
        canvas.Children.Add(status);

        var win = new Window
        {
            Title = "WPF (XAML / UIA)",
            Width = 496,
            Height = 338,
            ResizeMode = ResizeMode.NoResize,
            Content = canvas,
            WindowStartupLocation = WindowStartupLocation.Manual,
        };
        app.Run(win);
    }
}
