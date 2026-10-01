param([int]$Seconds = 60, [int]$Interval = 8, [string]$ExeDir = 'D:\kmcounter')

# Data dir comes from datadir.txt (ASCII script, Chinese path read from the file itself)
$marker = Join-Path $ExeDir 'datadir.txt'
$dataDir = (Get-Content $marker -Encoding UTF8 | Where-Object { $_.Trim() -ne '' -and -not $_.Trim().StartsWith('#') } | Select-Object -First 1).Trim()
$path = Join-Path $dataDir 'stats.json'
$today = (Get-Date).ToString('yyyyMMdd')
$hour = (Get-Date).Hour
Write-Host "stats file: $path   today=$today hour=$hour"

$prev = $null
$end = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $end) {
    try {
        $j = Get-Content $path -Raw -Encoding UTF8 | ConvertFrom-Json
        $d = $j.days.$today
        $dayKs = 0; $sumKeys = 0; $hourKs = 0
        if ($d) {
            $dayKs = [int64]$d.keystrokes
            if ($d.keys) { foreach ($v in $d.keys) { $sumKeys += [int64]$v } }
            if ($d.hours -and $d.hours.$hour) { $hourKs = [int64]$d.hours.$hour.keystrokes }
        }
        $line = ('{0}  today_keystrokes={1} (keys sum={2})  hour{3}={4}  total_hour{3}={5}  total={6}' -f `
            (Get-Date).ToString('HH:mm:ss'), $dayKs, $sumKeys, $hour, $hourKs, $j.total.hour.$hour.keystrokes, $j.total.keystrokes)
        if ($null -ne $prev) {
            $delta = $dayKs - $prev
            if ($delta -ne 0) { $line += "   <- +$delta" }
        }
        $prev = $dayKs
        Write-Host $line
    } catch { Write-Host ('read failed: ' + $_.Exception.Message) }
    Start-Sleep -Seconds $Interval
}
