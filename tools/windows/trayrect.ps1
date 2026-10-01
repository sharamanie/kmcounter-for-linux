param([Parameter(Mandatory=$true)][int]$TargetPid)

Add-Type -Namespace K -Name T -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder lpClassName, int nMaxCount);
[DllImport("shell32.dll")] public static extern int Shell_NotifyIconGetRect(ref NOTIFYICONIDENTIFIER identifier, out RECT iconLocation);
[DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct NOTIFYICONIDENTIFIER { public uint cbSize; public IntPtr hWnd; public uint uID; public Guid guidItem; }
'@

[void][K.T]::SetProcessDPIAware()

# 找到托盘窗口句柄（KMCounterTrayWnd）
$trayHwnd = [IntPtr]::Zero
$cb = [K.T+EnumWindowsProc]{
    param($hWnd, $lParam)
    $owner = 0
    [void][K.T]::GetWindowThreadProcessId($hWnd, [ref]$owner)
    if ($owner -eq $TargetPid) {
        $cn = New-Object System.Text.StringBuilder 256
        [void][K.T]::GetClassName($hWnd, $cn, 256)
        if ($cn.ToString() -like '*KMCounterTrayWnd*') { $script:trayHwnd = $hWnd; return $false }
    }
    return $true
}
[void][K.T]::EnumWindows($cb, [IntPtr]::Zero)
if ($trayHwnd -eq [IntPtr]::Zero) { Write-Output 'tray window not found'; exit 1 }
Write-Output ("tray hwnd = 0x{0:X}" -f [int64]$trayHwnd)

$nid = New-Object K.T+NOTIFYICONIDENTIFIER
$nid.cbSize = [System.Runtime.InteropServices.Marshal]::SizeOf([type][K.T+NOTIFYICONIDENTIFIER])
$nid.hWnd = $trayHwnd
$nid.uID = 1
$nid.guidItem = [Guid]::Empty

$rect = New-Object K.T+RECT
$hr = [K.T]::Shell_NotifyIconGetRect([ref]$nid, [ref]$rect)
Write-Output "Shell_NotifyIconGetRect hr=0x$('{0:X8}' -f $hr)"
if ($hr -eq 0) {
    $cx = [int](($rect.Left + $rect.Right) / 2)
    $cy = [int](($rect.Top + $rect.Bottom) / 2)
    Write-Output "icon rect = $($rect.Left),$($rect.Top) - $($rect.Right),$($rect.Bottom)  center=$cx,$cy"
} else {
    Write-Output 'icon not visible (probably in the overflow flyout)'
}
