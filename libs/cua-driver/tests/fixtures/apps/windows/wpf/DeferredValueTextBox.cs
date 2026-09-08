using System;
using System.Threading.Tasks;
using System.Windows.Automation.Peers;
using System.Windows.Automation.Provider;
using System.Windows.Controls;

namespace CuaTestHarness.Wpf;

/// <summary>
/// Deterministic fixture for UIA providers that accept SetValue synchronously
/// but publish the new Value only after the provider callback has unwound.
/// </summary>
public sealed class DeferredValueTextBox : TextBox
{
    protected override AutomationPeer OnCreateAutomationPeer() =>
        new DeferredValueTextBoxAutomationPeer(this);
}

internal sealed class DeferredValueTextBoxAutomationPeer : TextBoxAutomationPeer, IValueProvider
{
    private readonly DeferredValueTextBox _owner;

    internal DeferredValueTextBoxAutomationPeer(DeferredValueTextBox owner) : base(owner)
    {
        _owner = owner;
    }

    public override object GetPattern(PatternInterface patternInterface) =>
        patternInterface == PatternInterface.Value ? this : base.GetPattern(patternInterface);

    bool IValueProvider.IsReadOnly => _owner.Dispatcher.CheckAccess()
        ? _owner.IsReadOnly
        : _owner.Dispatcher.Invoke(() => _owner.IsReadOnly);

    string IValueProvider.Value => _owner.Dispatcher.CheckAccess()
        ? _owner.Text
        : _owner.Dispatcher.Invoke(() => _owner.Text);

    void IValueProvider.SetValue(string value)
    {
        // Start the delay on the UI thread, then publish later. The driver's
        // immediate CurrentValue read therefore sees the old value, while a
        // separate snapshot after the action call observes the accepted write.
        _owner.Dispatcher.BeginInvoke(new Action(async () =>
        {
            await Task.Delay(400);
            _owner.Text = value;
        }));
    }
}
