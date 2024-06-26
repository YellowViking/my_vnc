$shared = "C:\Shared"
$watcher = New-Object System.IO.FileSystemWatcher
# if shared is changed
$watcher.Path = $shared
$watcher.Filter = "my-vnc.exe"
$watcher.IncludeSubdirectories = $false
$watcher.EnableRaisingEvents = $true
$watcher.NotifyFilter = [System.IO.NotifyFilters]::LastWrite
Write-Host "Watching $shared for changes..."
$event = (New-Guid).Guid
Register-ObjectEvent -InputObject $watcher -EventName Changed -SourceIdentifier $event
try
{
    while ($true)
    {
        Wait-Event -SourceIdentifier $event
        # remove event
        Remove-Event -SourceIdentifier $event
        $path = "$shared\my-vnc.exe"
        Write-Host "File $path has been changed"
        # kill my-vnc.exe
        Stop-Process -Name "my-vnc" -Force
        # copy my-vnc.exe to Desktop
        Copy-Item -Path $path -Destination "C:\Users\User\Desktop"
        # start my-vnc.exe
        # set environment variable
        $env:RUST_BACKTRACE = 1
        $env:RUST_LOG = "INFO"
        Start-Process -FilePath "C:\Users\User\Desktop\my-vnc.exe" -ArgumentList "--host 0.0.0.0"
        Write-Host "File $path has been copied to Desktop and started"
        $time = Get-Date
        Write-Host "Waiting for changes... $time`n"
    }
}
finally
{
    Write-Host "Unregistering event..."
    Unregister-Event -SourceIdentifier $event
    $watcher.Dispose()
}
