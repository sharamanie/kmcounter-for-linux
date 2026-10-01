param([Parameter(Mandatory=$true)][int]$TargetPid)

# 用与修复后的 Rust 代码完全相同的判据，在真实进程上模拟“找主窗口”，检查会不会再选错
Add-Type -Namespace S -Name W -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder lpClassName, int nMaxCount);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, System.Text.StringBuilder lpString, int nMaxCount);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
'@
[void][S.W]::SetProcessDPIAware()

$WINIT_CLASS = 'Window Class'
$TITLE_PREFIX = 'KMCounter-rs'

function ClassOf([IntPtr]$h) {
    $b = New-Object System.Text.StringBuilder 128
    $n = [S.W]::GetClassName($h, $b, 128); if ($n -le 0) { return '' }
    return $b.ToString(0, $n)
}
function TitleOf([IntPtr]$h) {
    $b = New-Object System.Text.StringBuilder 256
    $n = [S.W]::GetWindowTextW($h, $b, 256); if ($n -le 0) { return '' }
    return $b.ToString(0, $n)
}
function IsHelper([string]$c) {
    return ($c -like '*KMCounterTrayWnd*') -or ($c -like '*Pbuffer*') -or ($c -like '*Winit Thread Event Target*') `
        -or ($c -like '*IME*') -or ($c -like '*Sogou*') -or $c.StartsWith('SoPY')
}

$byClass = [IntPtr]::Zero; $byTitle = [IntPtr]::Zero; $fallback = [IntPtr]::Zero
$rows = New-Object System.Collections.ArrayList
$cb = [S.W+EnumWindowsProc]{
    param($hWnd, $lParam)
    $owner = 0
    [void][S.W]::GetWindowThreadProcessId($hWnd, [ref]$owner)
    if ($owner -ne $TargetPid) { return $true }
    $cls = ClassOf $hWnd; $tit = TitleOf $hWnd
    $helper = IsHelper $cls
    $pick = '-'
    if (-not $helper) {
        if ($cls -eq $WINIT_CLASS -and $script:byClass -eq [IntPtr]::Zero) { $script:byClass = $hWnd; $pick = 'by_class' }
        elseif ($tit.StartsWith($TITLE_PREFIX)) { if ($script:byTitle -eq [IntPtr]::Zero) { $script:byTitle = $hWnd; $pick = 'by_title' } }
        elseif ($script:fallback -eq [IntPtr]::Zero) { $script:fallback = $hWnd; $pick = 'fallback' }
    } elseif ($helper) { $pick = '(helper, skipped)' }
    $r = New-Object S.W+RECT; [void][S.W]::GetWindowRect($hWnd, [ref]$r)
    [void]$script:rows.Add([pscustomobject]@{
        HWND = ('0x{0:X}' -f [int64]$hWnd); Class = $cls; Title = $tit;
        Visible = [S.W]::IsWindowVisible($hWnd);
        Rect = "$($r.Left),$($r.Top) $($r.Right-$r.Left)x$($r.Bottom-$r.Top)"; Pick = $pick
    })
    return $true
}
[void][S.W]::EnumWindows($cb, [IntPtr]::Zero)

$rows | Format-Table -AutoSize | Out-String -Width 200

$chosen = if ($byClass -ne [IntPtr]::Zero) { $byClass } elseif ($byTitle -ne [IntPtr]::Zero) { $byTitle } else { $fallback }
$how = if ($byClass -ne [IntPtr]::Zero) { 'by_class' } elseif ($byTitle -ne [IntPtr]::Zero) { 'by_title' } else { 'fallback (not cached)' }
Write-Host ("fixed criteria pick: 0x{0:X} ({1})" -f [int64]$chosen, $how)
