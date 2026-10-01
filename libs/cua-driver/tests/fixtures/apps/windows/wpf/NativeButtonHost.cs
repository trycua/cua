using System;
using System.Runtime.InteropServices;
using System.Windows.Interop;

namespace CuaTestHarness.Wpf;

internal sealed class NativeButtonHost : HwndHost
{
    private const int WS_CHILD     = 0x40000000;
    private const int WS_VISIBLE   = 0x10000000;
    private const int WS_TABSTOP   = 0x00010000;
    private const int BS_DEFPUSHBUTTON = 0x00000001;

    private readonly string _text;
    private IntPtr _hwnd;

    public NativeButtonHost(string text) { _text = text; }

    protected override HandleRef BuildWindowCore(HandleRef hwndParent)
    {
        _hwnd = CreateWindowEx(
            0,
            "BUTTON",
            _text,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
            0, 0, 200, 32,
            hwndParent.Handle,
            IntPtr.Zero,
            GetModuleHandle(null),
            IntPtr.Zero);
        if (_hwnd == IntPtr.Zero)
            throw new InvalidOperationException("CreateWindowEx(BUTTON) failed");
        return new HandleRef(this, _hwnd);
    }

    protected override void DestroyWindowCore(HandleRef hwnd)
    {
        DestroyWindow(hwnd.Handle);
        _hwnd = IntPtr.Zero;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateWindowEx(
        int dwExStyle,
        string lpClassName,
        string lpWindowName,
        int dwStyle,
        int x, int y, int nWidth, int nHeight,
        IntPtr hWndParent,
        IntPtr hMenu,
        IntPtr hInstance,
        IntPtr lpParam);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool DestroyWindow(IntPtr hWnd);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr GetModuleHandle(string? lpModuleName);
}
