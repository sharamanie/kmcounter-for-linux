param([Parameter(Mandatory=$true)][int]$TargetPid)

Add-Type -Namespace K -Name Win -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder lpClassName, int nMaxCount);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, System.Text.StringBuilder lpString, int nMaxCount);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
[DllImport("user32.dll")] public static extern int GetWindowLongW(IntPtr hWnd, int nIndex);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
'@

$results = New-Object System.Collections.ArrayList

$cb = [K.Win+EnumWindowsProc]{
    param($hWnd, $lParam)
    $owner = 0
    [void][K.Win]::GetWindowThreadProcessId($hWnd, [ref]$owner)
    if ($owner -eq $TargetPid) {
        $cn = New-Object System.Text.StringBuilder 256
        [void][K.Win]::GetClassName($hWnd, $cn, 256)
        $tt = New-Object System.Text.StringBuilder 512
        [void][K.Win]::GetWindowTextW($hWnd, $tt, 512)
        $r = New-Object K.Win+RECT
        [void][K.Win]::GetWindowRect($hWnd, [ref]$r)
        $style = [K.Win]::GetWindowLongW($hWnd, -16)
        [void]$results.Add([pscustomobject]@{
            HWND      = ('0x{0:X}' -f [int64]$hWnd)
            Class     = $cn.ToString()
            Title     = $tt.ToString()
            Visible   = [K.Win]::IsWindowVisible($hWnd)
            Minimized = [K.Win]::IsIconic($hWnd)
            Rect      = "$($r.Left),$($r.Top) $($r.Right - $r.Left)x$($r.Bottom - $r.Top)"
            Style     = ('0x{0:X8}' -f $style)
        })
    }
    return $true
}

[void][K.Win]::EnumWindows($cb, [IntPtr]::Zero)
$fg = [K.Win]::GetForegroundWindow()
Write-Output ("foreground=0x{0:X}" -f [int64]$fg)
$results | Format-Table -AutoSize | Out-String -Width 200
