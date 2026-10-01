param([Parameter(Mandatory=$true)][int]$TargetPid, [int]$Rounds = 2)

Add-Type -Namespace K -Name C -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, System.Text.StringBuilder lpClassName, int nMaxCount);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, System.Text.StringBuilder lpString, int nMaxCount);
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
[DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, int dx, int dy, uint dwData, System.UIntPtr dwExtraInfo);
'@

[void][K.C]::SetProcessDPIAware()

function Find-Window([int]$ownerPid, [string]$titlePrefix, [string]$classLike) {
    $script:found = [IntPtr]::Zero
    $cb = [K.C+EnumWindowsProc]{
        param($hWnd, $lParam)
        $owner = 0
        [void][K.C]::GetWindowThreadProcessId($hWnd, [ref]$owner)
        if ($owner -eq $ownerPid) {
            $cn = New-Object System.Text.StringBuilder 256
            [void][K.C]::GetClassName($hWnd, $cn, 256)
            $tt = New-Object System.Text.StringBuilder 512
            [void][K.C]::GetWindowTextW($hWnd, $tt, 512)
            if ($titlePrefix -and $tt.ToString().StartsWith($titlePrefix)) { $script:found = $hWnd; return $false }
            if ($classLike -and $cn.ToString() -like $classLike) { $script:found = $hWnd; return $false }
        }
        return $true
    }
    [void][K.C]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:found
}

function Click-At([int]$x, [int]$y) {
    [void][K.C]::SetCursorPos($x, $y)
    Start-Sleep -Milliseconds 150
    [K.C]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 60
    [K.C]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
}

$CHEVRON_X = 2124; $CHEVRON_Y = 1564
$ICON_X = 2010;    $ICON_Y = 1436

function FlyoutVisible {
    $fly = Find-Window -ownerPid 9400 -titlePrefix '' -classLike '*OverflowXamlIsland*'
    if ($fly -eq [IntPtr]::Zero) { return $false }
    return [K.C]::IsWindowVisible($fly)
}

function Report([string]$tag) {
    $main = Find-Window -ownerPid $TargetPid -titlePrefix 'KMCounter-rs' -classLike ''
    if ($main -eq [IntPtr]::Zero) { Write-Host "[$tag] MAIN not found"; return }
    $vis = [K.C]::IsWindowVisible($main)
    $min = [K.C]::IsIconic($main)
    $fg = [K.C]::GetForegroundWindow()
    Write-Host ("[$tag] main visible={0} minimized={1} hwnd=0x{2:X} foreground=0x{3:X}{4}" -f $vis, $min, [int64]$main, [int64]$fg, $(if ([int64]$fg -eq [int64]$main) { ' (main IS foreground)' } else { '' }))
}

for ($i = 1; $i -le $Rounds; $i++) {
    Write-Host "===== round $i ====="
    $main = Find-Window -ownerPid $TargetPid -titlePrefix 'KMCounter-rs' -classLike ''
    if ($main -ne [IntPtr]::Zero -and [K.C]::IsWindowVisible($main)) {
        [void][K.C]::PostMessageW($main, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)  # WM_CLOSE = 点 ×
        Start-Sleep -Milliseconds 900
        Report "after-close"
    } else {
        Write-Host '[skip close: window not visible]'
    }
    if (-not (FlyoutVisible)) {
        Click-At $CHEVRON_X $CHEVRON_Y
        Start-Sleep -Milliseconds 1000
    }
    Write-Host ("[flyout] visible={0}" -f (FlyoutVisible))
    Click-At $ICON_X $ICON_Y
    Start-Sleep -Milliseconds 1200
    Report "after-summon"
}
