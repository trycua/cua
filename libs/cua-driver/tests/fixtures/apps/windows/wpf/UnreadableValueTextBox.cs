using System;
using System.Windows.Automation.Peers;
using System.Windows.Automation.Provider;
using System.Windows.Controls;

namespace CuaTestHarness.Wpf;

/// <summary>
/// Deterministic fixture for UIA providers that accept SetValue but fail to
/// return their current Value.
/// </summary>
public sealed class UnreadableValueTextBox : TextBox
{
    protected override AutomationPeer OnCreateAutomationPeer() =>
        new UnreadableValueTextBoxAutomationPeer(this);
}

internal sealed class UnreadableValueTextBoxAutomationPeer : TextBoxAutomationPeer, IValueProvider
{
    private readonly UnreadableValueTextBox _owner;

    internal UnreadableValueTextBoxAutomationPeer(UnreadableValueTextBox owner) : base(owner)
    {
        _owner = owner;
    }

    public override object GetPattern(PatternInterface patternInterface) =>
        patternInterface == PatternInterface.Value ? this : base.GetPattern(patternInterface);

    bool IValueProvider.IsReadOnly => _owner.Dispatcher.CheckAccess()
        ? _owner.IsReadOnly
        : _owner.Dispatcher.Invoke(() => _owner.IsReadOnly);

    string IValueProvider.Value => throw new InvalidOperationException("Value is not readable.");

    void IValueProvider.SetValue(string value)
    {
        if (_owner.Dispatcher.CheckAccess())
        {
            _owner.Text = value;
        }
        else
        {
            _owner.Dispatcher.Invoke(() => _owner.Text = value);
        }
    }
}
