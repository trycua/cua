using System.Windows.Automation.Peers;
using System.Windows.Controls;

namespace CuaTestHarness.Wpf;

// Models providers that report the child's own state even when its parent
// disables input. WPF still prevents interaction through the disabled parent.
public sealed class EnabledReportingCheckBox : CheckBox
{
    protected override AutomationPeer OnCreateAutomationPeer() => new Peer(this);

    private sealed class Peer(EnabledReportingCheckBox owner) : CheckBoxAutomationPeer(owner)
    {
        protected override bool IsEnabledCore() => true;
    }
}
