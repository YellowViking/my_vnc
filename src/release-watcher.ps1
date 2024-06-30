$shared = "C:\Shared\"
try
{
    while ($true)
    {
        $path = "$shared\zip.zip"
        # read file stamp
        $stamp = (Get-Item $path).LastWriteTime
        # compare file stamp
        if ($stamp -eq $last_write_time)
        {
            # wait for 1 second
            Write-Debug "File $path has not been changed $stamp"
            Start-Sleep -Milliseconds 500
            continue
        }
        # update file stamp
        $last_write_time = $stamp
        Write-Host "File $path has been changed"
        # kill my-vnc.exe
        Stop-Process -Name "my-vnc" -Force
        # start my-vnc.exe
        Expand-Archive -Path $path -DestinationPath C:\Users\User\Desktop -Force
        # set environment variable
        $env:RUST_BACKTRACE = 1
        $env:RUST_LOG = "INFO"
        Invoke-Expression "C:\Users\User\Desktop\my-vnc.exe --host fox-pc --port 80 --use-tunnel"
        Write-Host "File $path has been copied to Desktop and started"
    }
}
finally
{
    Write-Host "Script has been stopped"
}
