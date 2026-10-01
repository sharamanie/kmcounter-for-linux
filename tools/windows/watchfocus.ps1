param([string]$ExeDir = 'D:\kmcounter', [int]$Seconds = 180, [int]$Interval = 3)

Add-Type -Namespace F -Name W -MemberDefinition @'
[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
'@

$marker = Join-Path $ExeDir 'datadir.txt'
$dir = (Get-Content $marker -Encoding UTF8 | Where-Object { $_.Trim() -ne '' -and -not $_.Trim().StartsWith('#') } | Select-Object -First 1).Trim()
$path = Join-Path $dir 'stats.json'
$today = (Get-Date).ToString('yyyyMMdd')
$hour = (Get-Date).Hour
Write-Host "sampling foreground window + keystrokes (today=$today) for $Seconds s ..."

$prev = $null
$end = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $end) {
    $fg = [F.W]::GetForegroundWindow()
    $t = New-Object System.Text.StringBuilder 256
    [void][F.W]::GetWindowTextW($fg, $t, 256)
    $c = New-Object System.Text.StringBuilder 256
    [void][F.W]::GetClassName($fg, $c, 256)
    $title = $t.ToString(); $cls = $c.ToString()
    $isKmc = $title.StartsWith('KMCounter-rs')
    try {
        $j = Get-Content $path -Raw -Encoding UTF8 | ConvertFrom-Json
        $ks = [int64]$j.days.$today.keystrokes
        $hourKs = 0
        if ($j.days.$today.hours.$hour) { $hourKs = [int64]$j.days.$today.hours.$hour.keystrokes }
        $mtime = (Get-Item $path).LastWriteTime.ToString('HH:mm:ss')
        $delta = ''
        if ($null -ne $prev -and $ks -ne $prev) { $delta = '  <- +' + ($ks - $prev) }
        $prev = $ks
        Write-Host ('{0}  fg_is_KMC={1}  fg_class={2}  today_key={3} hour{4}={5} (file {6}){7}' -f `
            (Get-Date).ToString('HH:mm:ss'), $isKmc, $cls, $ks, $hour, $hourKs, $mtime, $delta)
    } catch { Write-Host ('read failed: ' + $_.Exception.Message) }
    Start-Sleep -Seconds $Interval
}
Write-Host 'done'
