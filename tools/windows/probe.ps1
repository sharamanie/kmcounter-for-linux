param([Parameter(Mandatory=$true)][int]$TargetPid, [string]$Action = 'scan', [int]$SettleMs = 1200)

Add-Type -Namespace K -Name Win -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder lpClassName, int nMaxCount);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, System.Text.StringBuilder lpString, int nMaxCount);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
'@

function Get-OurWindows {
    $list = New-Object System.Collections.ArrayList
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
            [void]$list.Add([pscustomobject]@{
                HWND = [int64]$hWnd
                Hex = ('0x{0:X}' -f [int64]$hWnd)
                Class = $cn.ToString()
                Title = $tt.ToString()
                Visible = [K.Win]::IsWindowVisible($hWnd)
                Minimized = [K.Win]::IsIconic($hWnd)
                Rect = "$($r.Left),$($r.Top) $($r.Right - $r.Left)x$($r.Bottom - $r.Top)"
            })
        }
        return $true
    }
    [void][K.Win]::EnumWindows($cb, [IntPtr]::Zero)
    return $list
}

function Show-State([string]$tag) {
    $wins = Get-OurWindows
    $main = $wins | Where-Object { $_.Title -like 'KMCounter-rs*' } | Select-Object -First 1
    $tray = $wins | Where-Object { $_.Class -like '*KMCounterTrayWnd*' } | Select-Object -First 1
    $fg = [K.Win]::GetForegroundWindow()
    Write-Host ("[$tag] foreground=0x{0:X}" -f [int64]$fg)
    if ($main) {
        Write-Host ("[$tag] MAIN  hwnd={0} visible={1} minimized={2} rect={3}" -f $main.Hex, $main.Visible, $main.Minimized, $main.Rect)
    } else {
        Write-Host "[$tag] MAIN  <not found>"
    }
    if ($tray) {
        Write-Host ("[$tag] TRAY  hwnd={0} visible={1}" -f $tray.Hex, $tray.Visible)
    } else {
        Write-Host "[$tag] TRAY  <not found>"
    }
    return $main
}

$main = Show-State 'before'

switch ($Action) {
    'click' {
        $tray = (Get-OurWindows) | Where-Object { $_.Class -like '*KMCounterTrayWnd*' } | Select-Object -First 1
        if (-not $tray) { Write-Output 'ERROR: tray window not found'; exit 1 }
        # WM_APP_TRAY = 0x8000, lParam = WM_LBUTTONUP (0x0202)  <- same message the shell sends on icon click
        $ok = [K.Win]::PostMessageW([IntPtr]$tray.HWND, 0x8000, [IntPtr]::Zero, [IntPtr]0x0202)
        Write-Output "posted WM_APP_TRAY(LBUTTONUP) to tray hwnd=$($tray.Hex) ok=$ok"
        Start-Sleep -Milliseconds $SettleMs
        [void](Show-State 'after-click')
    }
    'close' {
        if (-not $main) { Write-Output 'ERROR: main window not found'; exit 1 }
        # WM_CLOSE (0x0010) <- exactly what the × button sends
        $ok = [K.Win]::PostMessageW([IntPtr]$main.HWND, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
        Write-Output "posted WM_CLOSE to main hwnd=$($main.Hex) ok=$ok"
        Start-Sleep -Milliseconds $SettleMs
        [void](Show-State 'after-close')
    }
    'both' {
        $tray = (Get-OurWindows) | Where-Object { $_.Class -like '*KMCounterTrayWnd*' } | Select-Object -First 1
        [void][K.Win]::PostMessageW([IntPtr]$tray.HWND, 0x8000, [IntPtr]::Zero, [IntPtr]0x0202)
        Write-Output 'posted tray click'
        Start-Sleep -Milliseconds $SettleMs
        $m = Show-State 'after-click'
        if ($m) {
            [void][K.Win]::PostMessageW([IntPtr]$m.HWND, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
            Write-Output 'posted WM_CLOSE'
            Start-Sleep -Milliseconds $SettleMs
            [void](Show-State 'after-close')
        }
    }
    default { }
}
