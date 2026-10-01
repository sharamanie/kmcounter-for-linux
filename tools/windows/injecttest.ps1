param([string]$ExeDir = 'D:\kmcounter', [int]$TargetPid = 0, [int]$Count = 10, [int]$WaitSec = 35)

Add-Type -Namespace I -Name K -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, System.UIntPtr dwExtraInfo);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
public delegate bool EnumWindowsProc(IntPtr h, IntPtr p);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
'@

function ReadCount {
    $marker = Join-Path $ExeDir 'datadir.txt'
    $dir = (Get-Content $marker -Encoding UTF8 | Where-Object { $_.Trim() -ne '' -and -not $_.Trim().StartsWith('#') } | Select-Object -First 1).Trim()
    $j = Get-Content (Join-Path $dir 'stats.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $today = (Get-Date).ToString('yyyyMMdd')
    return [int64]$j.days.$today.keystrokes
}

$main = [IntPtr]::Zero
$cb = [I.K+EnumWindowsProc]{
    param($h, $p)
    $owner = 0
    [void][I.K]::GetWindowThreadProcessId($h, [ref]$owner)
    if ($owner -eq $script:TargetPid) {
        $t = New-Object System.Text.StringBuilder 256
        [void][I.K]::GetWindowTextW($h, $t, 256)
        if ($t.ToString().StartsWith('KMCounter-rs')) { $script:main = $h; return $false }
    }
    return $true
}
if ($TargetPid -gt 0) { [void][I.K]::EnumWindows($cb, [IntPtr]::Zero) }
if ($main -ne [IntPtr]::Zero) {
    [void][I.K]::SetForegroundWindow($main)
    Start-Sleep -Milliseconds 400
}
Write-Host ("foreground = 0x{0:X}   KMC main = 0x{1:X}" -f [int64][I.K]::GetForegroundWindow(), [int64]$main)

$before = ReadCount
Write-Host "keystrokes before: $before"
for ($i = 0; $i -lt $Count; $i++) {
    [I.K]::keybd_event(0x7C, 0, 0, [UIntPtr]::Zero)   # VK_F13 down
    [I.K]::keybd_event(0x7C, 0, 2, [UIntPtr]::Zero)   # VK_F13 up
    Start-Sleep -Milliseconds 60
}
Write-Host "injected $Count synthetic F13 keypresses (LLKHF_INJECTED should make the app ignore them)"
Write-Host "waiting $WaitSec s for the app's 30 s flush ..."
Start-Sleep -Seconds $WaitSec
$after = ReadCount
Write-Host "keystrokes after:  $after   (delta = $($after - $before))"
