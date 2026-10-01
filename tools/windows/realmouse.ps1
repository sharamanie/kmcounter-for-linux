param([int]$X, [int]$Y, [int]$WaitMs = 1500, [switch]$NoClick)

Add-Type -Namespace K -Name M -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
[DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
[DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, int dx, int dy, uint dwData, System.UIntPtr dwExtraInfo);
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern int GetSystemMetrics(int nIndex);
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
'@

$aware = [K.M]::SetProcessDPIAware()
Write-Output "SetProcessDPIAware=$aware  screen=$([K.M]::GetSystemMetrics(0))x$([K.M]::GetSystemMetrics(1)) (physical)"

[void][K.M]::SetCursorPos($X, $Y)
Start-Sleep -Milliseconds 150
$p = New-Object K.M+POINT
[void][K.M]::GetCursorPos([ref]$p)
Write-Output "cursor now at $($p.X),$($p.Y) (requested $X,$Y)"
if ($NoClick) {
    Write-Output 'hover only (no click)'
    Start-Sleep -Milliseconds $WaitMs
    exit 0
}
Start-Sleep -Milliseconds 150
[K.M]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
Start-Sleep -Milliseconds 60
[K.M]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
Write-Output "clicked at $X,$Y"
Start-Sleep -Milliseconds $WaitMs
