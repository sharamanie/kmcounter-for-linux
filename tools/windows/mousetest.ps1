param([string]$ExeDir = 'D:\kmcounter', [int]$TargetPid = 0, [int]$Steps = 30, [int]$WaitSec = 35)

Add-Type -Namespace M -Name T -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
public delegate bool EnumWindowsProc(IntPtr h, IntPtr p);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
'@

[void][M.T]::SetProcessDPIAware()

function ReadMousePx {
    $marker = Join-Path $ExeDir 'datadir.txt'
    $dir = (Get-Content $marker -Encoding UTF8 | Where-Object { $_.Trim() -ne '' -and -not $_.Trim().StartsWith('#') } | Select-Object -First 1).Trim()
    $j = Get-Content (Join-Path $dir 'stats.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $today = (Get-Date).ToString('yyyyMMdd')
    return [double]$j.days.$today.mouse.move_px
}

function Find-Window([string]$classLike, [string]$titlePrefix, [int]$pid) {
    $script:found = [IntPtr]::Zero
    $cb = [M.T+EnumWindowsProc]{
        param($h, $p)
        $owner = 0
        [void][M.T]::GetWindowThreadProcessId($h, [ref]$owner)
        if ($pid -gt 0 -and $owner -ne $pid) { return $true }
        $c = New-Object System.Text.StringBuilder 256
        [void][M.T]::GetClassName($h, $c, 256)
        $t = New-Object System.Text.StringBuilder 256
        [void][M.T]::GetWindowTextW($h, $t, 256)
        if ($classLike -and $c.ToString() -like $classLike) { $script:found = $h; return $false }
        if ($titlePrefix -and $t.ToString().StartsWith($titlePrefix)) { $script:found = $h; return $false }
        return $true
    }
    [void][M.T]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:found
}

function ZigZag([IntPtr]$hwnd, [int]$n) {
    $r = New-Object M.T+RECT
    [void][M.T]::GetWindowRect($hwnd, [ref]$r)
    $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
    Write-Host ("  rect={0},{1} {2}x{3}" -f $r.Left, $r.Top, $w, $h)
    for ($i = 0; $i -lt $n; $i++) {
        $fx = if ($i % 2 -eq 0) { 0.15 } else { 0.85 }
        $fy = 0.2 + 0.6 * (($i % 5) / 4.0)
        $x = [int]($r.Left + $w * $fx); $y = [int]($r.Top + $h * $fy)
        [void][M.T]::SetCursorPos($x, $y)
        Start-Sleep -Milliseconds 40
    }
}

$kmc = Find-Window -classLike '' -titlePrefix 'KMCounter-rs' -pid $TargetPid
$chrome = Find-Window -classLike 'Chrome_WidgetWin_1' -titlePrefix '' -pid 0
Write-Host ("KMC hwnd=0x{0:X}   Chrome hwnd=0x{1:X}" -f [int64]$kmc, [int64]$chrome)

$a0 = ReadMousePx
Write-Host "== phase 1: move cursor over the KMC window (input targeted at our own process) =="
ZigZag $kmc $Steps
Write-Host "waiting $WaitSec s for flush ..."; Start-Sleep -Seconds $WaitSec
$a1 = ReadMousePx
Write-Host ("move_px: {0} -> {1}   delta = {2}" -f $a0, $a1, ($a1 - $a0))

Write-Host "== phase 2: same zigzag over the Chrome window (another process) =="
ZigZag $chrome $Steps
Write-Host "waiting $WaitSec s for flush ..."; Start-Sleep -Seconds $WaitSec
$a2 = ReadMousePx
Write-Host ("move_px: {0} -> {1}   delta = {2}" -f $a1, $a2, ($a2 - $a1))
