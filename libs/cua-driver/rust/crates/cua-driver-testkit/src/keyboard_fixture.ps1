$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing -TypeDefinition @'
using System;
using System.Windows.Forms;
using System.Drawing;
public class KeyboardOracle : Form {
    static bool Observe(Form form, ref Message message, string recipient) {
        bool down = message.Msg == 0x100 || message.Msg == 0x104;
        bool up = message.Msg == 0x101 || message.Msg == 0x105;
        if (!down && !up) return false;
        Console.WriteLine("{\"kind\":\"" + (down ? "down" : "up") + "\",\"key\":" + message.WParam.ToInt64() + ",\"flags\":" + (int)Control.ModifierKeys + ",\"recipient\":\"" + recipient + "\"}");
        Console.Out.Flush();
        if (!down || Environment.GetEnvironmentVariable("CUA_KEYBOARD_CLOSE_ON_KEY") != "1") return false;
        form.Close();
        Console.WriteLine("{\"kind\":\"closed\"}");
        Console.Out.Flush();
        return true;
    }
    protected override void WndProc(ref Message message) {
        if (!Observe(this, ref message, "window")) base.WndProc(ref message);
    }
    class Input : TextBox {
        protected override void WndProc(ref Message message) {
            if (!Observe(FindForm(), ref message, "field")) base.WndProc(ref message);
        }
    }
    public KeyboardOracle() {
        Text = "Cua Keyboard Oracle";
        Size = new Size(420, 240);
        var input = new Input { AccessibleName = "Keyboard input", Text = "unchanged", Location = new Point(30, 80), Width = 350 };
        Controls.Add(input);
        Shown += delegate {
            if (Environment.GetEnvironmentVariable("CUA_KEYBOARD_COMPANION") == "1") {
                var companion = new Form { Text = "Cua Keyboard Oracle Companion", Size = new Size(360, 240) };
                companion.Controls.Add(new Input { Text = "unchanged", Location = new Point(30, 80), Width = 300 });
                companion.Show();
            }
            Activate();
            input.Focus();
            Console.WriteLine("{\"kind\":\"ready\",\"window\":" + Handle.ToInt64() + "}");
            Console.Out.Flush();
        };
    }
}
'@
[System.Windows.Forms.Application]::EnableVisualStyles()
[System.Windows.Forms.Application]::Run([KeyboardOracle]::new())
