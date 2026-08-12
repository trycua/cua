using System.Windows;

namespace CuaTestHarness.Wpf;

public partial class LayeredPopupWindow : Window
{
    public LayeredPopupWindow() { InitializeComponent(); }
    private void OnCloseClick(object sender, RoutedEventArgs e) => Close();
}
